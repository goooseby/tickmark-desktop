use std::str::FromStr;

use chrono::{
    DateTime, Datelike, Duration, FixedOffset, Local, LocalResult, NaiveDate, NaiveDateTime,
    NaiveTime, Offset, SecondsFormat, TimeZone, Utc,
};
use chrono_tz::Tz;

use crate::cli::RangeArgs;

#[derive(Clone, Copy)]
pub enum Zone {
    Local,
    Named(Tz),
    Fixed(FixedOffset),
}

fn utc() -> FixedOffset {
    FixedOffset::east_opt(0).expect("UTC offset")
}

fn parse_offset(text: &str) -> Option<FixedOffset> {
    let sign = match text.as_bytes().first()? {
        b'+' => 1,
        b'-' => -1,
        _ => return None,
    };
    let digits: String = text[1..].chars().filter(|ch| *ch != ':').collect();
    let (hours, minutes) = match digits.len() {
        2 => (digits.parse::<i32>().ok()?, 0),
        4 => (
            digits[..2].parse::<i32>().ok()?,
            digits[2..].parse::<i32>().ok()?,
        ),
        _ => return None,
    };
    if hours > 23 || minutes > 59 {
        return None;
    }
    FixedOffset::east_opt(sign * (hours * 3600 + minutes * 60))
}

impl Zone {
    pub fn parse(text: &str) -> Result<Self, String> {
        let text = text.trim();
        if text.is_empty() || text.eq_ignore_ascii_case("local") {
            return Ok(Self::Local);
        }
        if let Ok(zone) = Tz::from_str(text) {
            return Ok(Self::Named(zone));
        }
        if let Some(offset) = parse_offset(text) {
            return Ok(Self::Fixed(offset));
        }
        Err(format!(
            "无法识别的时区「{text}」，可用 local、UTC、+08:00 或 Asia/Shanghai。"
        ))
    }

    pub fn name(&self) -> String {
        match self {
            Self::Local => "local".into(),
            Self::Named(zone) => zone.name().to_owned(),
            Self::Fixed(offset) => offset.to_string(),
        }
    }

    pub fn offset_at(&self, ms: i64) -> FixedOffset {
        match self {
            Self::Local => Local
                .timestamp_millis_opt(ms)
                .earliest()
                .map(|value| value.offset().fix())
                .unwrap_or_else(utc),
            Self::Named(zone) => zone
                .timestamp_millis_opt(ms)
                .earliest()
                .map(|value| value.offset().fix())
                .unwrap_or_else(utc),
            Self::Fixed(offset) => *offset,
        }
    }

    pub fn datetime(&self, ms: i64) -> DateTime<FixedOffset> {
        DateTime::from_timestamp_millis(ms)
            .unwrap_or_default()
            .with_timezone(&self.offset_at(ms))
    }

    pub fn iso(&self, ms: i64) -> String {
        self.datetime(ms).to_rfc3339_opts(SecondsFormat::Secs, false)
    }

    pub fn display(&self, ms: i64) -> String {
        self.datetime(ms).format("%Y-%m-%d %H:%M").to_string()
    }

    pub fn date_of(&self, ms: i64) -> NaiveDate {
        self.datetime(ms).date_naive()
    }

    fn wall_ms(&self, naive: NaiveDateTime) -> i64 {
        match self {
            Self::Local => match Local.from_local_datetime(&naive) {
                LocalResult::Single(value) => value.timestamp_millis(),
                LocalResult::Ambiguous(first, _) => first.timestamp_millis(),
                LocalResult::None => Local
                    .from_local_datetime(&(naive + Duration::hours(1)))
                    .earliest()
                    .map(|value| value.timestamp_millis())
                    .unwrap_or_default(),
            },
            Self::Named(zone) => match zone.from_local_datetime(&naive) {
                LocalResult::Single(value) => value.timestamp_millis(),
                LocalResult::Ambiguous(first, _) => first.timestamp_millis(),
                LocalResult::None => zone
                    .from_local_datetime(&(naive + Duration::hours(1)))
                    .earliest()
                    .map(|value| value.timestamp_millis())
                    .unwrap_or_default(),
            },
            Self::Fixed(offset) => offset
                .from_local_datetime(&naive)
                .earliest()
                .map(|value| value.timestamp_millis())
                .unwrap_or_default(),
        }
    }

    pub fn day_start(&self, date: NaiveDate) -> i64 {
        self.wall_ms(date.and_time(NaiveTime::MIN))
    }

    pub fn next_day(&self, date: NaiveDate) -> i64 {
        let next = date.succ_opt().unwrap_or(date);
        self.day_start(next)
    }
}

pub fn parse_date(text: &str, zone: &Zone) -> Result<NaiveDate, String> {
    let now = Utc::now().timestamp_millis();
    match text.trim().to_ascii_lowercase().as_str() {
        "today" => Ok(zone.date_of(now)),
        "yesterday" => zone
            .date_of(now)
            .pred_opt()
            .ok_or_else(|| "日期超出范围。".to_owned()),
        _ => NaiveDate::parse_from_str(text.trim(), "%Y-%m-%d")
            .map_err(|_| format!("无法识别的日期「{text}」，请用 YYYY-MM-DD、today 或 yesterday。")),
    }
}

pub fn parse_duration(text: &str) -> Result<i64, String> {
    let text = text.trim();
    if text == "0" {
        return Ok(0);
    }
    let mut total: i64 = 0;
    let mut number = String::new();
    let mut seen = false;
    for ch in text.chars() {
        if ch.is_ascii_digit() {
            number.push(ch);
            continue;
        }
        let value: i64 = number
            .parse()
            .map_err(|_| format!("无法识别的时长「{text}」，例如 90s、5m、1h30m。"))?;
        let unit = match ch {
            's' | 'S' => 1_000,
            'm' | 'M' => 60_000,
            'h' | 'H' => 3_600_000,
            'd' | 'D' => 86_400_000,
            'w' | 'W' => 604_800_000,
            _ => return Err(format!("无法识别的时长单位「{ch}」，可用 s、m、h、d、w。")),
        };
        total += value.saturating_mul(unit);
        number.clear();
        seen = true;
    }
    if !number.is_empty() || !seen {
        return Err(format!("时长「{text}」缺少单位，例如 90s、5m、1h30m。"));
    }
    Ok(total)
}

pub fn parse_point(text: &str, zone: &Zone) -> Result<i64, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("时间不能为空。".into());
    }
    match text.to_ascii_lowercase().as_str() {
        "now" => return Ok(Utc::now().timestamp_millis()),
        "today" => return Ok(zone.day_start(zone.date_of(Utc::now().timestamp_millis()))),
        "yesterday" => {
            let today = zone.date_of(Utc::now().timestamp_millis());
            let day = today.pred_opt().ok_or_else(|| "日期超出范围。".to_owned())?;
            return Ok(zone.day_start(day));
        }
        _ => {}
    }
    if let Ok(value) = DateTime::parse_from_rfc3339(text) {
        return Ok(value.timestamp_millis());
    }
    if let Ok(date) = NaiveDate::parse_from_str(text, "%Y-%m-%d") {
        return Ok(zone.day_start(date));
    }
    for format in [
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%d %H:%M",
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%dT%H:%M",
    ] {
        if let Ok(value) = NaiveDateTime::parse_from_str(text, format) {
            return Ok(zone.wall_ms(value));
        }
    }
    Err(format!(
        "无法识别的时间「{text}」，可用 2026-11-26、2026-11-26 09:30 或 RFC 3339。"
    ))
}

pub struct ResolvedRange {
    pub from_ms: i64,
    pub to_ms: i64,
}

pub fn resolve(args: &RangeArgs, zone: &Zone, allow_large: bool) -> Result<ResolvedRange, String> {
    let groups = [
        args.from.is_some() || args.to.is_some(),
        args.date.is_some(),
        args.range.is_some(),
        args.last.is_some(),
    ]
    .into_iter()
    .filter(|flag| *flag)
    .count();
    if groups > 1 {
        return Err("--from/--to、--date、--range、--last 只能使用一种。".into());
    }
    let now = Utc::now().timestamp_millis();
    let (from_ms, to_ms) = if args.from.is_some() {
        let from = args.from.as_deref().unwrap_or_default();
        let to = args
            .to
            .as_deref()
            .ok_or_else(|| "使用 --from 时还需要 --to。".to_owned())?;
        (parse_point(from, zone)?, parse_point(to, zone)?)
    } else if let Some(date) = args.date.as_deref() {
        let day = parse_date(date, zone)?;
        (zone.day_start(day), zone.next_day(day))
    } else if let Some(range) = args.range.as_deref() {
        let today = zone.date_of(now);
        match range.trim().to_ascii_lowercase().as_str() {
            "today" => (zone.day_start(today), zone.next_day(today)),
            "yesterday" => {
                let day = today.pred_opt().ok_or_else(|| "日期超出范围。".to_owned())?;
                (zone.day_start(day), zone.next_day(day))
            }
            "week" => {
                let monday = today - Duration::days(today.weekday().num_days_from_monday() as i64);
                (zone.day_start(monday), zone.day_start(monday + Duration::days(7)))
            }
            "month" => {
                let first = NaiveDate::from_ymd_opt(today.year(), today.month(), 1)
                    .ok_or_else(|| "无法计算月份起点。".to_owned())?;
                let next = if today.month() == 12 {
                    NaiveDate::from_ymd_opt(today.year() + 1, 1, 1)
                } else {
                    NaiveDate::from_ymd_opt(today.year(), today.month() + 1, 1)
                }
                .ok_or_else(|| "无法计算月份终点。".to_owned())?;
                (zone.day_start(first), zone.day_start(next))
            }
            other => {
                return Err(format!(
                    "无法识别的范围「{other}」，可用 today、yesterday、week、month。"
                ));
            }
        }
    } else if let Some(last) = args.last.as_deref() {
        let duration = parse_duration(last)?;
        (now - duration, now)
    } else {
        let today = zone.date_of(now);
        (zone.day_start(today), zone.next_day(today))
    };
    if to_ms <= from_ms {
        return Err("结束时间必须晚于开始时间。".into());
    }
    if !allow_large && to_ms - from_ms > 366 * 86_400_000 {
        return Err("范围超过 366 天，确需查询请加 --allow-large。".into());
    }
    Ok(ResolvedRange { from_ms, to_ms })
}

/// Duration text used by tables and JSON `duration` fields, matching the dashboard.
pub fn duration_text(ms: Option<i64>) -> String {
    let Some(ms) = ms else {
        return "—".into();
    };
    if ms > 0 && ms < 1_000 {
        return "<1 秒".into();
    }
    let seconds = ms / 1_000;
    let hours = seconds / 3_600;
    let minutes = (seconds / 60) % 60;
    let mut parts = Vec::new();
    if hours > 0 {
        parts.push(format!("{hours} 小时"));
    }
    if minutes > 0 {
        parts.push(format!("{minutes} 分"));
    }
    if parts.is_empty() {
        parts.push(format!("{} 秒", seconds % 60));
    }
    parts.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_compound_durations() {
        assert_eq!(parse_duration("90s").unwrap(), 90_000);
        assert_eq!(parse_duration("1h30m").unwrap(), 5_400_000);
        assert_eq!(parse_duration("2d").unwrap(), 172_800_000);
        assert!(parse_duration("5").is_err());
        assert!(parse_duration("5x").is_err());
    }

    #[test]
    fn fixed_offset_zone_round_trips() {
        let zone = Zone::parse("+08:00").unwrap();
        let from = parse_point("2026-11-26", &zone).unwrap();
        let to = parse_point("2026-11-27", &zone).unwrap();
        assert_eq!(to - from, 86_400_000);
        assert_eq!(zone.iso(from), "2026-11-26T00:00:00+08:00");
    }
}
