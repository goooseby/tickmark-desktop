use std::collections::BTreeMap;

use crate::model::{Device, ReportData, Row, state_key};

/// Same threshold as the dashboard: adjacent app rows within 2 s belong to one session.
pub const SESSION_GAP_MS: i64 = 2_000;

#[derive(Clone, Debug)]
pub struct Span {
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
}

impl Span {
    pub fn duration_ms(&self) -> i64 {
        (self.end_ms - self.start_ms).max(0)
    }

    pub fn state_key(&self) -> &'static str {
        state_key(self.state)
    }
}

fn clip(rows: &[Row], device: &Device, start: i64, end: i64) -> Vec<Span> {
    rows.iter()
        .filter(|row| row.start_ms < end && row.end_ms > start)
        .map(|row| Span {
            device: device.id.clone(),
            device_name: device.name.clone(),
            start_ms: row.start_ms.max(start),
            end_ms: row.end_ms.min(end),
            state: row.state,
            app_id: row.app_id.clone(),
            app_name: row.app_name.clone(),
            page: row.page.clone(),
            page_kind: row.page_kind.map(|kind| kind.as_str().to_owned()),
            playback: row.playback,
        })
        .filter(|span| span.end_ms > span.start_ms)
        .collect()
}

/// Device-level usage rows: what the dashboard feeds to `usageMetrics`.
pub fn usage_spans(devices: &[&Device], start: i64, end: i64) -> Vec<Span> {
    devices.iter().flat_map(|device| clip(&device.rows, device, start, end)).collect()
}

/// App rows: cloud exports keep them separate, local data uses the same table.
pub fn app_spans(devices: &[&Device], start: i64, end: i64) -> Vec<Span> {
    devices
        .iter()
        .flat_map(|device| clip(device.app_rows(), device, start, end))
        .collect()
}

pub fn all_spans(data: &ReportData, start: i64, end: i64) -> Vec<Span> {
    let devices: Vec<&Device> = data.devices.iter().collect();
    usage_spans(&devices, start, end)
}

#[derive(Clone, Debug)]
pub struct PairOverlap {
    pub devices: [String; 2],
    pub ms: i64,
}

#[derive(Clone, Debug)]
pub struct OverlapSpan {
    pub start_ms: i64,
    pub end_ms: i64,
    pub devices: Vec<String>,
}

#[derive(Default)]
pub struct UsageMetrics {
    pub known: bool,
    pub total_ms: i64,
    pub overlap_ms: i64,
    pub device_sum_ms: i64,
    pub by_device: BTreeMap<String, i64>,
    pub concurrency: BTreeMap<usize, i64>,
    pub pairs: Vec<PairOverlap>,
    pub overlap_spans: Vec<OverlapSpan>,
}

impl UsageMetrics {
    pub fn total(&self) -> Option<i64> {
        self.known.then_some(self.total_ms)
    }

    pub fn overlap(&self) -> Option<i64> {
        self.known.then_some(self.overlap_ms)
    }

    pub fn device_sum(&self) -> Option<i64> {
        self.known.then_some(self.device_sum_ms)
    }
}

/// Port of `usageMetrics` in `native/web/report.mjs`.
pub fn usage_metrics(spans: &[Span], include_overlap_spans: bool) -> UsageMetrics {
    let mut metrics = UsageMetrics {
        known: spans.iter().any(|span| span.state != 3),
        ..Default::default()
    };
    let mut events: Vec<(i64, &str, i32)> = spans
        .iter()
        .filter(|span| span.state == 0)
        .flat_map(|span| {
            [
                (span.start_ms, span.device.as_str(), 1),
                (span.end_ms, span.device.as_str(), -1),
            ]
        })
        .collect();
    events.sort_by_key(|event| event.0);
    let mut active: BTreeMap<&str, i32> = BTreeMap::new();
    let mut previous = events.first().map(|event| event.0);
    let mut pairs: BTreeMap<(String, String), i64> = BTreeMap::new();
    let mut index = 0;
    while index < events.len() {
        let time = events[index].0;
        if let Some(last) = previous {
            let span = time - last;
            if span != 0 && !active.is_empty() {
                let ids: Vec<&str> = active.keys().copied().collect();
                metrics.total_ms += span;
                *metrics.concurrency.entry(ids.len()).or_default() += span;
                if ids.len() > 1 {
                    metrics.overlap_ms += span;
                    if include_overlap_spans {
                        let same = metrics.overlap_spans.last().is_some_and(|existing| {
                            existing.end_ms == last
                                && existing.devices.len() == ids.len()
                                && existing.devices.iter().zip(&ids).all(|(a, b)| a == b)
                        });
                        if same {
                            if let Some(existing) = metrics.overlap_spans.last_mut() {
                                existing.end_ms = time;
                            }
                        } else {
                            metrics.overlap_spans.push(OverlapSpan {
                                start_ms: last,
                                end_ms: time,
                                devices: ids.iter().map(|id| (*id).to_owned()).collect(),
                            });
                        }
                    }
                }
                for id in &ids {
                    *metrics.by_device.entry((*id).to_owned()).or_default() += span;
                }
                for outer in 0..ids.len() {
                    for inner in outer + 1..ids.len() {
                        *pairs
                            .entry((ids[outer].to_owned(), ids[inner].to_owned()))
                            .or_default() += span;
                    }
                }
            }
        }
        while index < events.len() && events[index].0 == time {
            let (_, device, change) = events[index];
            index += 1;
            let count = active.get(device).copied().unwrap_or(0) + change;
            if count != 0 {
                active.insert(device, count);
            } else {
                active.remove(device);
            }
        }
        previous = Some(time);
    }
    metrics.device_sum_ms = metrics.by_device.values().sum();
    metrics.pairs = pairs
        .into_iter()
        .map(|((first, second), ms)| PairOverlap { devices: [first, second], ms })
        .collect();
    metrics.pairs.sort_by(|left, right| right.ms.cmp(&left.ms));
    metrics
}

#[derive(Default, Clone, Copy)]
pub struct Coverage {
    pub active_ms: i64,
    pub idle_ms: i64,
    pub offline_ms: i64,
    pub gap_ms: i64,
    pub unobserved_ms: i64,
    pub known_ms: i64,
    pub elapsed_ms: i64,
}

pub fn coverage(spans: &[Span], start: i64, end: i64) -> Coverage {
    let mut result = Coverage { elapsed_ms: (end - start).max(0), ..Default::default() };
    for span in spans {
        match span.state {
            0 => result.active_ms += span.duration_ms(),
            1 => result.idle_ms += span.duration_ms(),
            2 => result.offline_ms += span.duration_ms(),
            _ => result.gap_ms += span.duration_ms(),
        }
    }
    let observed = result.active_ms + result.idle_ms + result.offline_ms + result.gap_ms;
    result.unobserved_ms = (result.elapsed_ms - observed).max(0);
    result.known_ms = result.active_ms + result.idle_ms + result.offline_ms;
    result
}

#[derive(Clone, Debug)]
pub struct PageAggregate {
    pub name: String,
    pub kind: String,
    pub duration_ms: i64,
}

#[derive(Clone, Debug)]
pub struct AppAggregate {
    pub device: String,
    pub device_name: String,
    pub app_id: String,
    pub name: String,
    pub duration_ms: i64,
    pub pages: Vec<PageAggregate>,
}

#[derive(Default)]
struct AppAccumulator {
    name: String,
    device_name: String,
    duration_ms: i64,
    pages: BTreeMap<(String, String), i64>,
}

/// Port of `aggregateApps`: grouped by device + app, pages summed inside.
pub fn aggregate_apps(spans: &[Span]) -> Vec<AppAggregate> {
    let mut apps: BTreeMap<(String, String), AppAccumulator> = BTreeMap::new();
    for span in spans {
        if span.state != 0 {
            continue;
        }
        let Some(app_id) = span.app_id.as_deref().filter(|id| !id.trim().is_empty()) else {
            continue;
        };
        let entry = apps
            .entry((span.device.clone(), app_id.to_owned()))
            .or_insert_with(|| AppAccumulator {
                name: span.app_name.clone().unwrap_or_else(|| app_id.to_owned()),
                device_name: span.device_name.clone(),
                ..Default::default()
            });
        let duration = span.duration_ms();
        entry.duration_ms += duration;
        if let Some(page) = span.page.as_ref().filter(|_| duration > 0) {
            let kind = if span.page_kind.as_deref() == Some("site") { "site" } else { "document" };
            *entry.pages.entry((kind.to_owned(), page.clone())).or_default() += duration;
        }
    }
    let mut result: Vec<AppAggregate> = apps
        .into_iter()
        .map(|((device, app_id), accumulator)| {
            let mut pages: Vec<PageAggregate> = accumulator
                .pages
                .into_iter()
                .map(|((kind, name), duration_ms)| PageAggregate { name, kind, duration_ms })
                .collect();
            pages.sort_by(|left, right| right.duration_ms.cmp(&left.duration_ms));
            let identified: i64 = pages.iter().map(|page| page.duration_ms).sum();
            let remainder = accumulator.duration_ms - identified;
            if !pages.is_empty() && remainder > 0 {
                pages.push(PageAggregate {
                    name: "页面未识别".into(),
                    kind: "unknown".into(),
                    duration_ms: remainder,
                });
            }
            AppAggregate {
                device,
                device_name: accumulator.device_name,
                app_id,
                name: accumulator.name,
                duration_ms: accumulator.duration_ms,
                pages,
            }
        })
        .collect();
    result.sort_by(|left, right| {
        right.duration_ms.cmp(&left.duration_ms)
            .then_with(|| left.name.cmp(&right.name))
    });
    result
}

#[derive(Clone, Debug)]
pub struct AppSession {
    pub device: String,
    pub device_name: String,
    pub app_id: String,
    pub name: String,
    pub start_ms: i64,
    pub end_ms: i64,
    pub spans: usize,
}

impl AppSession {
    pub fn duration_ms(&self) -> i64 {
        (self.end_ms - self.start_ms).max(0)
    }
}

/// Merge adjacent app rows per device + app within `gap_ms`.
pub fn app_sessions(spans: &[Span], gap_ms: i64) -> Vec<AppSession> {
    let mut tracks: BTreeMap<(&str, &str), Vec<&Span>> = BTreeMap::new();
    for span in spans {
        if span.state != 0 {
            continue;
        }
        let Some(app_id) = span.app_id.as_deref().filter(|id| !id.trim().is_empty()) else {
            continue;
        };
        tracks.entry((span.device.as_str(), app_id)).or_default().push(span);
    }
    let mut sessions = Vec::new();
    for ((device, app_id), mut track) in tracks {
        track.sort_by_key(|span| (span.start_ms, span.end_ms));
        let mut current: Option<AppSession> = None;
        for span in track {
            match current.as_mut() {
                Some(session) if span.start_ms <= session.end_ms + gap_ms => {
                    session.end_ms = session.end_ms.max(span.end_ms);
                    session.spans += 1;
                    if let Some(name) = span.app_name.as_ref().filter(|name| !name.trim().is_empty()) {
                        session.name = name.clone();
                    }
                }
                _ => {
                    if let Some(session) = current.take() {
                        sessions.push(session);
                    }
                    current = Some(AppSession {
                        device: device.to_owned(),
                        device_name: span.device_name.clone(),
                        app_id: app_id.to_owned(),
                        name: span.app_name.clone().unwrap_or_else(|| app_id.to_owned()),
                        start_ms: span.start_ms,
                        end_ms: span.end_ms,
                        spans: 1,
                    });
                }
            }
        }
        if let Some(session) = current {
            sessions.push(session);
        }
    }
    sessions.sort_by_key(|session| (session.start_ms, session.end_ms));
    sessions
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Interval {
    pub start_ms: i64,
    pub end_ms: i64,
}

/// Union of active ranges, sorted and merged.
pub fn merge_ranges(mut ranges: Vec<(i64, i64)>) -> Vec<Interval> {
    ranges.sort_by_key(|(start, end)| (*start, *end));
    let mut merged: Vec<Interval> = Vec::new();
    for (start, end) in ranges {
        if end <= start {
            continue;
        }
        match merged.last_mut() {
            Some(last) if start <= last.end_ms => last.end_ms = last.end_ms.max(end),
            _ => merged.push(Interval { start_ms: start, end_ms: end }),
        }
    }
    merged
}

pub fn active_ranges(spans: &[Span]) -> Vec<Interval> {
    merge_ranges(
        spans
            .iter()
            .filter(|span| span.state == 0)
            .map(|span| (span.start_ms, span.end_ms))
            .collect(),
    )
}

/// Complement of `active` inside `[start, end)`.
pub fn quiet_ranges(active: &[Interval], start: i64, end: i64) -> Vec<Interval> {
    let mut quiet = Vec::new();
    let mut cursor = start;
    for span in active {
        if span.start_ms > cursor {
            quiet.push(Interval { start_ms: cursor, end_ms: span.start_ms.min(end) });
        }
        cursor = cursor.max(span.end_ms);
        if cursor >= end {
            break;
        }
    }
    if cursor < end {
        quiet.push(Interval { start_ms: cursor, end_ms: end });
    }
    quiet.retain(|span| span.end_ms > span.start_ms);
    quiet
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(start_ms: i64, end_ms: i64, state: i64, device: &str, app: Option<&str>) -> Span {
        Span {
            device: device.into(),
            device_name: device.into(),
            start_ms,
            end_ms,
            state,
            app_id: app.map(str::to_owned),
            app_name: app.map(str::to_owned),
            page: None,
            page_kind: None,
            playback: false,
        }
    }

    #[test]
    fn three_device_concurrency_deduplicates_total_and_overlap() {
        let rows = [
            span(0, 10, 0, "desk", Some("editor")),
            span(3, 9, 0, "phone", Some("chat")),
            span(5, 11, 0, "tablet", Some("pdf")),
        ];
        let result = usage_metrics(&rows, false);
        assert_eq!(result.total(), Some(11));
        assert_eq!(result.overlap(), Some(7));
        assert_eq!(result.device_sum(), Some(22));
        assert_eq!(result.concurrency.get(&1), Some(&4));
        assert_eq!(result.concurrency.get(&2), Some(&3));
        assert_eq!(result.concurrency.get(&3), Some(&4));
        assert_eq!(result.pairs.len(), 3);
        assert_eq!(result.pairs[0].devices, ["desk".to_owned(), "phone".to_owned()]);
        assert_eq!(result.pairs[0].ms, 6);
    }

    #[test]
    fn no_known_rows_reports_none_instead_of_zero() {
        let rows = [span(0, 10, 3, "desk", None)];
        let result = usage_metrics(&rows, false);
        assert_eq!(result.total(), None);
        assert_eq!(result.overlap(), None);
    }

    #[test]
    fn sessions_merge_two_second_flicker_only() {
        let rows = [
            span(0, 1000, 0, "desk", Some("a")),
            span(1000, 3000, 0, "desk", Some("b")),
            span(5000, 6000, 0, "desk", Some("a")),
            span(20000, 21000, 0, "desk", Some("a")),
        ];
        let sessions = app_sessions(&rows, SESSION_GAP_MS);
        assert_eq!(sessions.len(), 4);
        assert_eq!(sessions[0].name, "a");
        assert_eq!(sessions[0].end_ms, 1000);
        assert_eq!(sessions[2].name, "a");
        assert_eq!(sessions[2].start_ms, 5000);
    }
}
