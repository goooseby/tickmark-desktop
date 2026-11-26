use crate::{
    metrics::{Interval, Span},
    model::state_key,
};

pub const TIMELINE_SMOOTH_GAP_MS: i64 = 2_000;
pub const QUIET_MIN_MS: i64 = 20 * 60 * 1000;
pub const QUIET_MAX_SPANS: usize = 4;

#[derive(Clone, Debug)]
pub struct TimelineItem {
    pub device: String,
    pub device_name: String,
    pub start_ms: i64,
    pub end_ms: i64,
    pub state: i64,
    pub app_id: Option<String>,
    pub app_name: Option<String>,
    pub page: Option<String>,
    pub page_kind: Option<String>,
    pub playback: bool,
    pub unidentified: bool,
    pub merged: usize,
    pub gap_ms: i64,
}

impl TimelineItem {
    pub fn from_span(span: &Span) -> Self {
        Self {
            device: span.device.clone(),
            device_name: span.device_name.clone(),
            start_ms: span.start_ms,
            end_ms: span.end_ms,
            state: span.state,
            app_id: span.app_id.clone(),
            app_name: span.app_name.clone(),
            page: span.page.clone(),
            page_kind: span.page_kind.clone(),
            playback: span.playback,
            unidentified: false,
            merged: 1,
            gap_ms: 0,
        }
    }

    pub fn state_key(&self) -> &'static str {
        state_key(self.state)
    }

    fn identity(&self) -> String {
        format!("{}\u{0}{}", self.device, self.app_id.as_deref().unwrap_or_default())
    }
}

/// Port of `coalesceTimelineItems`: join adjacent runs of the same app or state
/// when the gap is a capture flicker rather than a real switch.
pub fn coalesce(items: &[TimelineItem], max_gap_ms: i64) -> Vec<TimelineItem> {
    let mut merged: Vec<TimelineItem> = Vec::with_capacity(items.len());
    let mut index = 0;
    while index < items.len() {
        let item = items[index].clone();
        if let Some(previous) = merged.last_mut() {
            let previous_id = if previous.state == 0 && !previous.unidentified {
                Some(previous.identity())
            } else {
                None
            };
            let next = items.get(index + 1);
            let next_id = next
                .filter(|next| next.state == 0 && !next.unidentified)
                .map(TimelineItem::identity);
            let flicker = item.state == 0
                && previous_id.is_some()
                && next_id.is_some()
                && previous_id == next_id
                && (item.unidentified || Some(item.identity()) != previous_id)
                && item.end_ms - item.start_ms <= max_gap_ms
                && item.start_ms - previous.end_ms <= max_gap_ms
                && next.is_some_and(|next| next.start_ms - item.end_ms <= max_gap_ms);
            if flicker {
                if let Some(next) = next {
                    previous.end_ms = previous.end_ms.max(next.end_ms);
                    previous.merged += 2;
                }
                index += 2;
                continue;
            }
            let gap = item.start_ms - previous.end_ms;
            let same_run = previous.state == item.state
                && gap >= 0
                && gap <= max_gap_ms
                && (item.state != 0
                    || (!previous.unidentified
                        && !item.unidentified
                        && previous_id == Some(item.identity())));
            if same_run {
                previous.end_ms = previous.end_ms.max(item.end_ms);
                previous.merged += 1;
                previous.gap_ms += gap;
                index += 1;
                continue;
            }
        }
        merged.push(item);
        index += 1;
    }
    merged
}

pub fn quiet_reason(span: Interval, raw: &[Span], now_ms: i64) -> &'static str {
    if span.start_ms >= now_ms {
        return "future";
    }
    let overlapping: Vec<&Span> = raw
        .iter()
        .filter(|row| row.start_ms < span.end_ms && row.end_ms > span.start_ms)
        .collect();
    if overlapping.iter().any(|row| row.state == 2) {
        "offline"
    } else if overlapping.iter().any(|row| row.state == 1) {
        "idle"
    } else if overlapping.iter().any(|row| row.state == 3) {
        "gap"
    } else {
        "no-record"
    }
}

/// The quiet spans the dashboard would compress: longest few, each at least 20 minutes.
pub fn compressed_quiet(quiet: &[Interval]) -> Vec<bool> {
    let mut candidates: Vec<usize> = (0..quiet.len())
        .filter(|&index| quiet[index].end_ms - quiet[index].start_ms >= QUIET_MIN_MS)
        .collect();
    candidates.sort_by_key(|&index| {
        std::cmp::Reverse(quiet[index].end_ms - quiet[index].start_ms)
    });
    candidates.truncate(QUIET_MAX_SPANS);
    let mut flags = vec![false; quiet.len()];
    for index in candidates {
        flags[index] = true;
    }
    flags
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(start_ms: i64, end_ms: i64, app: &str) -> TimelineItem {
        TimelineItem {
            device: "desk".into(),
            device_name: "本机电脑".into(),
            start_ms,
            end_ms,
            state: 0,
            app_id: Some(app.into()),
            app_name: Some(app.into()),
            page: None,
            page_kind: None,
            playback: false,
            unidentified: false,
            merged: 1,
            gap_ms: 0,
        }
    }

    #[test]
    fn merges_same_app_with_short_flicker() {
        let items = [item(0, 10, "a"), item(10, 12, "a"), item(12, 30, "a")];
        let merged = coalesce(&items, TIMELINE_SMOOTH_GAP_MS);
        assert_eq!(merged.len(), 1);
        assert_eq!((merged[0].start_ms, merged[0].end_ms), (0, 30));
    }

    #[test]
    fn keeps_real_switches() {
        let items = [item(0, 10, "a"), item(10, 30, "b")];
        assert_eq!(coalesce(&items, TIMELINE_SMOOTH_GAP_MS).len(), 2);
    }
}
