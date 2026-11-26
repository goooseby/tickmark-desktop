use std::collections::{BTreeMap, BTreeSet, HashSet};

use chrono::Datelike;
use serde_json::{Value, json};
use tickmark_capture::store::Store;

use tickmark_report::{
    Device,
    metrics::{
        self, AppSession, Coverage, PageAggregate, Span,
    },
    model::MAX_TIMESTAMP_MS,
    timeline::{self, TimelineItem},
};

use crate::{
    cli::{
        AggregateArgs, AppsArgs, ExportArgs, Global, IntervalsArgs, OverlapArgs, ReportArgs,
        TimelineArgs,
    },
    data,
    errors::CliError,
    output::{self, Format},
    time::{self, ResolvedRange, Zone},
};

fn parse_states(values: &[String]) -> Result<Option<HashSet<i64>>, CliError> {
    if values.is_empty() {
        return Ok(None);
    }
    let mut states = HashSet::new();
    for value in values {
        for part in value.split(',') {
            match part.trim() {
                "" => {}
                "all" => return Ok(None),
                "active" => {
                    states.insert(0);
                }
                "idle" => {
                    states.insert(1);
                }
                "offline" => {
                    states.insert(2);
                }
                "gap" => {
                    states.insert(3);
                }
                other => {
                    return Err(CliError::usage(format!(
                        "无法识别的状态「{other}」，可用 active、idle、offline、gap、all。"
                    )));
                }
            }
        }
    }
    if states.is_empty() {
        Ok(None)
    } else {
        Ok(Some(states))
    }
}

fn app_matches(span: &Span, filters: &[String]) -> bool {
    if filters.is_empty() {
        return true;
    }
    let name = span.app_name.as_deref().unwrap_or_default();
    let id = span.app_id.as_deref().unwrap_or_default();
    filters
        .iter()
        .any(|filter| name.eq_ignore_ascii_case(filter) || id.eq_ignore_ascii_case(filter))
}

fn row_limit(global: &Global, format: Format, table: usize, json: usize) -> usize {
    global.limit.unwrap_or(if format == Format::Table { table } else { json })
}

fn thousands(value: i64) -> String {
    let negative = value < 0;
    let digits = value.unsigned_abs().to_string();
    let mut output = String::new();
    for (index, ch) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            output.push(',');
        }
        output.push(ch);
    }
    if negative {
        output.insert(0, '-');
    }
    output
}

fn percent(value: f64) -> String {
    format!("{:.1}%", value * 100.0)
}

fn state_label(state: i64) -> &'static str {
    match state {
        0 => "正常计时",
        1 => "无操作暂停",
        2 => "锁屏 / 离线",
        _ => "采集缺口",
    }
}

fn collector_running() -> bool {
    #[cfg(windows)]
    {
        tickmark_capture::windows::running()
    }
    #[cfg(not(windows))]
    {
        false
    }
}

fn snapshot_files(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir.join("report-devices")) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("json"))
        .collect()
}

fn last_record(database: &std::path::Path) -> Result<(i64, Option<i64>), CliError> {
    let db = rusqlite::Connection::open_with_flags(
        database,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .map_err(|error| CliError::unavailable(format!("无法只读打开数据库：{error}")))?;
    let _ = db.busy_timeout(std::time::Duration::from_secs(3));
    let result: rusqlite::Result<(i64, Option<i64>)> = db.query_row(
        "SELECT COUNT(*), MAX(end_ms) FROM capture_spans",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    );
    match result {
        Ok(values) => Ok(values),
        Err(error) if error.to_string().contains("no such table") => Ok((0, None)),
        Err(error) => Err(CliError::unavailable(format!("无法读取数据库：{error}"))),
    }
}

pub fn status(global: &Global, zone: &Zone) -> Result<bool, CliError> {
    let dir = data::resolve_dir(global);
    let database = dir.join("capture.db");
    if !database.is_file() {
        return Err(CliError::unavailable(format!(
            "找不到数据库 {}。请先启动 Tickmark 桌面版，或用 --data-dir 指定数据目录。",
            database.display()
        )));
    }
    let mut files = serde_json::Map::new();
    let mut total_bytes = 0u64;
    for name in ["capture.db", "capture.db-wal", "capture.db-shm"] {
        if let Ok(metadata) = std::fs::metadata(dir.join(name)) {
            total_bytes += metadata.len();
            files.insert(name.to_owned(), json!(metadata.len()));
        }
    }
    let (intervals, last_ms) = last_record(&database)?;
    let imported = snapshot_files(&dir).len();
    let mirror = tickmark_report::cloud::latest_mirror(&dir);
    let cloud_devices = match mirror.as_ref() {
        Some(path) => tickmark_report::cloud::device_summaries(path)
            .map_err(CliError::unavailable)?
            .len(),
        None => 0,
    };
    let mirror_modified = mirror.as_ref().and_then(|path| {
        std::fs::metadata(path)
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|stamp| stamp.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|elapsed| elapsed.as_millis() as i64)
    });
    let running = collector_running();
    let format = output::resolve_format(global.format.as_deref())?;

    if format == Format::Json {
        let value = json!({
            "schemaVersion": 1,
            "command": "status",
            "generatedAt": data::generated_at(zone),
            "dataDir": dir.to_string_lossy(),
            "database": { "bytes": total_bytes, "files": files },
            "collector": if running { "running" } else { "stopped" },
            "intervals": intervals,
            "lastRecord": last_ms.map(|value| zone.iso(value)),
            "devices": { "local": 1, "imported": imported, "cloud": cloud_devices },
            "cloudMirror": mirror.as_ref().map(|path| json!({
                "path": path.to_string_lossy(),
                "modified": mirror_modified.map(|value| zone.iso(value)),
            })),
        });
        output::emit_json(&value, None)?;
        return Ok(false);
    }

    let wal = files.get("capture.db-wal").and_then(Value::as_u64).unwrap_or(0);
    let mut lines = vec![
        format!("数据目录  {}", dir.display()),
        format!("数据库    {}（capture.db {}，WAL {}）", output::human_bytes(total_bytes), output::human_bytes(files.get("capture.db").and_then(Value::as_u64).unwrap_or(0)), output::human_bytes(wal)),
        format!("采集器    {}", if running { "运行中" } else { "未运行" }),
        format!("区间      {}", thousands(intervals)),
        format!("最后记录  {}", last_ms.map_or_else(|| "—".to_owned(), |value| zone.display(value))),
    ];
    let cloud = match mirror_modified {
        Some(stamp) => format!("云端 {cloud_devices}（镜像 {}）", zone.display(stamp)),
        None => format!("云端 {cloud_devices}"),
    };
    lines.push(format!("设备      本机 1，导入 {imported}，{cloud}"));
    output::print_block(&(lines.join("\n") + "\n"));
    Ok(false)
}

pub fn devices(global: &Global, zone: &Zone) -> Result<bool, CliError> {
    let dir = data::resolve_dir(global);
    let store: Store = data::open_store(&dir)?;
    let data = tickmark_report::read(&dir, &store, 0, MAX_TIMESTAMP_MS)
        .map_err(|error| CliError::error(format!("读取设备失败：{error}")))?;
    drop(store);
    let cloud = match data::cloud_path(global, &dir)? {
        Some(path) => tickmark_report::cloud::device_summaries(&path)
            .map_err(CliError::unavailable)?,
        None => Vec::new(),
    };

    struct Row {
        id: String,
        name: String,
        kind: String,
        source: &'static str,
        first: Option<i64>,
        last: Option<i64>,
        intervals: Option<usize>,
        platform: String,
        local: bool,
        cloud: bool,
    }
    let mut rows: Vec<Row> = Vec::new();
    for device in &data.devices {
        let first = device.rows.iter().map(|row| row.start_ms).min();
        let last = device.rows.iter().map(|row| row.end_ms).max();
        rows.push(Row {
            id: device.id.clone(),
            name: device.name.clone(),
            kind: device.kind.clone(),
            source: if device.local { "本机" } else { "导入" },
            first,
            last,
            intervals: Some(device.rows.len()),
            platform: device.platform.clone(),
            local: device.local,
            cloud: false,
        });
    }
    for device in &cloud {
        rows.push(Row {
            id: device.device.device_id.clone(),
            name: device.device.name.clone(),
            kind: device.device.kind.clone(),
            source: "云端",
            first: device.first_day_ms,
            last: device.last_day_ms,
            intervals: None,
            platform: device.device.platform.clone(),
            local: false,
            cloud: true,
        });
    }

    let format = output::resolve_format(global.format.as_deref())?;
    if format == Format::Json {
        let devices: Vec<Value> = rows
            .iter()
            .map(|row| {
                json!({
                    "id": row.id,
                    "name": row.name,
                    "kind": row.kind,
                    "platform": row.platform,
                    "source": match row.source { "本机" => "local", "导入" => "import", _ => "cloud" },
                    "local": row.local,
                    "cloud": row.cloud,
                    "firstMs": row.first,
                    "lastMs": row.last,
                    "intervals": row.intervals,
                })
            })
            .collect();
        let value = json!({
            "schemaVersion": 1,
            "command": "devices",
            "generatedAt": data::generated_at(zone),
            "dataDir": dir.to_string_lossy(),
            "devices": devices,
        });
        output::emit_json(&value, None)?;
        return Ok(rows.is_empty());
    }

    let cells: Vec<Vec<String>> = rows
        .iter()
        .map(|row| {
            vec![
                row.id.clone(),
                row.name.clone(),
                row.kind.clone(),
                row.source.to_owned(),
                row.first.map_or_else(|| "—".into(), |value| zone.display(value)),
                row.last.map_or_else(|| "—".into(), |value| zone.display(value)),
                row.intervals.map_or_else(|| "—".into(), |value| thousands(value as i64)),
            ]
        })
        .collect();
    output::print_block(&output::table(
        &["id", "名称", "类型", "来源", "首条记录", "末条记录", "区间数"],
        &[false, false, false, false, false, false, true],
        &cells,
        global.wide,
    ));
    Ok(rows.is_empty())
}

struct Tabular {
    headers: Vec<&'static str>,
    numeric: Vec<bool>,
    cells: Vec<Vec<String>>,
    rows: Vec<Value>,
}

fn interval_table(spans: &[Span], zone: &Zone, no_pages: bool) -> Tabular {
    let mut headers = vec!["设备", "开始", "结束", "时长", "状态", "应用"];
    if !no_pages {
        headers.push("页面");
    }
    let mut numeric = vec![false, false, false, true, false, false];
    if !no_pages {
        numeric.push(false);
    }
    let mut cells = Vec::new();
    let mut rows = Vec::new();
    for span in spans {
        let mut cell = vec![
            span.device_name.clone(),
            zone.display(span.start_ms),
            zone.display(span.end_ms),
            time::duration_text(Some(span.duration_ms())),
            state_label(span.state).to_owned(),
            span.app_name.clone().unwrap_or_else(|| "—".into()),
        ];
        if !no_pages {
            cell.push(span.page.clone().unwrap_or_else(|| "—".into()));
        }
        let value = json!({
            "deviceId": span.device,
            "deviceName": span.device_name,
            "startMs": span.start_ms,
            "endMs": span.end_ms,
            "durationMs": span.duration_ms(),
            "state": span.state,
            "stateKey": span.state_key(),
            "appId": span.app_id,
            "appName": span.app_name,
            "page": span.page,
            "pageKind": span.page_kind,
            "playback": span.playback,
        });
        cells.push(cell);
        rows.push(value);
    }
    Tabular { headers, numeric, cells, rows }
}

struct SessionRow {
    name: String,
    devices: BTreeMap<String, String>,
    sessions: i64,
    duration_ms: i64,
    start_ms: i64,
}

fn session_rows(sessions: &[AppSession], merge_by_name: bool) -> Vec<SessionRow> {
    let mut grouped: BTreeMap<String, SessionRow> = BTreeMap::new();
    for session in sessions {
        let key = if merge_by_name {
            format!("name\u{0}{}", session.name)
        } else {
            format!("app\u{0}{}\u{0}{}", session.device, session.app_id)
        };
        let entry = grouped.entry(key).or_insert_with(|| SessionRow {
            name: session.name.clone(),
            devices: BTreeMap::new(),
            sessions: 0,
            duration_ms: 0,
            start_ms: session.start_ms,
        });
        entry
            .devices
            .insert(session.device.clone(), session.device_name.clone());
        entry.sessions += 1;
        entry.duration_ms += session.duration_ms();
        entry.start_ms = entry.start_ms.min(session.start_ms);
    }
    let mut rows: Vec<SessionRow> = grouped.into_values().collect();
    rows.sort_by(|left, right| {
        right
            .duration_ms
            .cmp(&left.duration_ms)
            .then_with(|| left.name.cmp(&right.name))
    });
    rows
}

fn app_table(rows: &[SessionRow], with_device: bool, zone: &Zone) -> Tabular {
    let total: i64 = rows.iter().map(|row| row.duration_ms).sum();
    let mut headers = vec!["应用"];
    let mut numeric = vec![false];
    if with_device {
        headers.push("设备");
        numeric.push(false);
    }
    headers.extend(["会话数", "用时", "占比"]);
    numeric.extend([true, true, true]);
    let mut cells = Vec::new();
    let mut values = Vec::new();
    for row in rows {
        let mut cell = vec![row.name.clone()];
        if with_device {
            cell.push(
                row.devices
                    .values()
                    .map(|name| name.as_str())
                    .collect::<Vec<_>>()
                    .join("、"),
            );
        }
        cell.push(thousands(row.sessions));
        cell.push(time::duration_text(Some(row.duration_ms)));
        cell.push(if total > 0 {
            percent(row.duration_ms as f64 / total as f64)
        } else {
            "—".into()
        });
        cells.push(cell);
        values.push(json!({
            "name": row.name,
            "devices": row.devices.keys().collect::<Vec<_>>(),
            "sessions": row.sessions,
            "durationMs": row.duration_ms,
            "share": if total > 0 { (row.duration_ms as f64 / total as f64 * 1000.0).round() / 1000.0 } else { 0.0 },
            "firstMs": row.start_ms,
            "first": zone.iso(row.start_ms),
        }));
    }
    Tabular { headers, numeric, cells, rows: values }
}

pub fn query_intervals(
    global: &Global,
    zone: &Zone,
    args: &IntervalsArgs,
) -> Result<bool, CliError> {
    let format = output::resolve_format(global.format.as_deref())?;
    let range: ResolvedRange = time::resolve(&args.range, zone, global.allow_large).map_err(CliError::usage)?;
    let data = data::load(global, range.from_ms, range.to_ms)?;
    let devices = data::select_devices(&data, &args.device)?;
    let mut spans = metrics::usage_spans(&devices, range.from_ms, range.to_ms);
    let states = parse_states(&args.state)?;
    let minimum = args
        .min_duration
        .as_deref()
        .map(time::parse_duration)
        .transpose()
        .map_err(CliError::usage)?;
    spans.retain(|span| {
        states.as_ref().is_none_or(|set| set.contains(&span.state))
            && app_matches(span, &args.app)
            && (!args.app_unidentified || (span.state == 0 && span.app_id.is_none()))
            && args.page_contains.as_ref().is_none_or(|text| {
                span.page.as_deref().is_some_and(|page| page.contains(text))
            })
            && (!args.playback || span.playback)
            && minimum.is_none_or(|value| span.duration_ms() >= value)
    });
    match args.sort.as_deref().unwrap_or("start") {
        "start" => spans.sort_by_key(|span| (span.start_ms, span.end_ms)),
        "duration" => spans.sort_by(|left, right| {
            right
                .duration_ms()
                .cmp(&left.duration_ms())
                .then_with(|| left.start_ms.cmp(&right.start_ms))
        }),
        other => {
            return Err(CliError::usage(format!(
                "无法识别的排序「{other}」，可用 start 或 duration。"
            )));
        }
    }
    let limit = row_limit(global, format, 200, 1000);
    let total = spans.len();
    if spans.len() > limit {
        spans.truncate(limit);
        if format != Format::Json {
            eprintln!("tickmark: 结果共 {total} 条，已按 --limit {limit} 截断。");
        }
    }
    let table = interval_table(&spans, zone, global.no_pages);

    match format {
        Format::Json => {
            let value = json!({
                "schemaVersion": 1,
                "command": "query.intervals",
                "generatedAt": data::generated_at(zone),
                "range": data::range_json(zone, range.from_ms, range.to_ms),
                "scope": data::scope_json(global, &devices),
                "total": total,
                "rows": table.rows,
            });
            output::emit_json(&value, None)?;
        }
        Format::Table => output::print_block(&output::table(
            &table.headers,
            &table.numeric,
            &table.cells,
            global.wide,
        )),
        Format::Csv => {
            let text = output::csv(&table.headers, &table.cells, true);
            output::write_text(&text, None)?;
        }
        Format::Ndjson => {
            let meta = json!({
                "type": "meta",
                "schemaVersion": 1,
                "command": "query.intervals",
                "range": data::range_json(zone, range.from_ms, range.to_ms),
                "total": total,
            });
            output::write_text(&output::emit_ndjson(&meta, &table.rows), None)?;
        }
    }
    Ok(total == 0)
}

pub fn query_apps(global: &Global, zone: &Zone, args: &AppsArgs) -> Result<bool, CliError> {
    let format = output::resolve_format(global.format.as_deref())?;
    let range = time::resolve(&args.range, zone, global.allow_large).map_err(CliError::usage)?;
    let data = data::load(global, range.from_ms, range.to_ms)?;
    let devices = data::select_devices(&data, &args.device)?;
    let spans: Vec<Span> = metrics::app_spans(&devices, range.from_ms, range.to_ms)
        .into_iter()
        .filter(|span| app_matches(span, &args.app))
        .collect();
    let sessions = metrics::app_sessions(&spans, metrics::SESSION_GAP_MS);
    let merge = match args.merge_apps.as_str() {
        "none" => false,
        "name" => true,
        other => {
            return Err(CliError::usage(format!(
                "无法识别的 --merge-apps「{other}」，可用 none 或 name。"
            )));
        }
    };
    let mut rows = session_rows(&sessions, merge);
    let limit = args
        .top
        .unwrap_or_else(|| row_limit(global, format, 200, 1000));
    if rows.len() > limit {
        rows.truncate(limit);
    }
    let table = app_table(&rows, !merge, zone);
    match format {
        Format::Json => {
            let value = json!({
                "schemaVersion": 1,
                "command": "query.apps",
                "generatedAt": data::generated_at(zone),
                "range": data::range_json(zone, range.from_ms, range.to_ms),
                "scope": data::scope_json(global, &devices),
                "rows": table.rows,
            });
            output::emit_json(&value, None)?;
        }
        Format::Table => output::print_block(&output::table(
            &table.headers,
            &table.numeric,
            &table.cells,
            global.wide,
        )),
        Format::Csv => {
            let text = output::csv(&table.headers, &table.cells, true);
            output::write_text(&text, None)?;
        }
        Format::Ndjson => {
            let meta = json!({
                "type": "meta",
                "schemaVersion": 1,
                "command": "query.apps",
                "range": data::range_json(zone, range.from_ms, range.to_ms),
            });
            output::write_text(&output::emit_ndjson(&meta, &table.rows), None)?;
        }
    }
    Ok(sessions.is_empty())
}

pub struct MergedApp {
    name: String,
    duration_ms: i64,
    devices: BTreeSet<String>,
    pages: Vec<PageAggregate>,
}

fn merge_apps(apps: Vec<metrics::AppAggregate>, by_name: bool) -> Vec<MergedApp> {
    if !by_name {
        return apps
            .into_iter()
            .map(|app| MergedApp {
                name: app.name,
                duration_ms: app.duration_ms,
                devices: std::iter::once(app.device).collect(),
                pages: app.pages,
            })
            .collect();
    }
    let mut merged: BTreeMap<String, (MergedApp, BTreeMap<(String, String), i64>)> = BTreeMap::new();
    for app in apps {
        let entry = merged
            .entry(app.name.clone())
            .or_insert_with(|| {
                (
                    MergedApp {
                        name: app.name.clone(),
                        duration_ms: 0,
                        devices: BTreeSet::new(),
                        pages: Vec::new(),
                    },
                    BTreeMap::new(),
                )
            });
        entry.0.duration_ms += app.duration_ms;
        entry.0.devices.insert(app.device);
        for page in app.pages {
            *entry.1.entry((page.kind, page.name)).or_default() += page.duration_ms;
        }
    }
    let mut rows: Vec<MergedApp> = merged
        .into_values()
        .map(|(mut app, pages)| {
            let mut pages: Vec<PageAggregate> = pages
                .into_iter()
                .map(|((kind, name), duration_ms)| PageAggregate { name, kind, duration_ms })
                .collect();
            pages.sort_by(|left, right| right.duration_ms.cmp(&left.duration_ms));
            app.pages = pages;
            app
        })
        .collect();
    rows.sort_by(|left, right| {
        right
            .duration_ms
            .cmp(&left.duration_ms)
            .then_with(|| left.name.cmp(&right.name))
    });
    rows
}

pub struct ReportResult {
    pub from_ms: i64,
    pub to_ms: i64,
    pub totals: (Option<i64>, Option<i64>, Option<i64>),
    pub devices: Vec<(String, String, Coverage)>,
    pub device_ids: Vec<String>,
    pub apps: Vec<MergedApp>,
    pub app_total: i64,
    pub split: Vec<(String, String, Vec<MergedApp>)>,
    pub device_label: String,
}

fn device_label(devices: &[&Device]) -> String {
    match devices {
        [] => "无设备".into(),
        [only] => only.name.clone(),
        _ => "全部设备".into(),
    }
}

fn build_report(
    global: &Global,
    zone: &Zone,
    args: &ReportArgs,
) -> Result<ReportResult, CliError> {
    let range = time::resolve(&args.range, zone, global.allow_large).map_err(CliError::usage)?;
    let data = data::load(global, range.from_ms, range.to_ms)?;
    let devices = data::select_devices(&data, &args.device)?;
    let usage = metrics::usage_spans(&devices, range.from_ms, range.to_ms);
    let usage_metrics = metrics::usage_metrics(&usage, false);
    let mut per_device = Vec::new();
    for device in &devices {
        let spans = metrics::usage_spans(&[device], range.from_ms, range.to_ms);
        per_device.push((
            device.id.clone(),
            device.name.clone(),
            metrics::coverage(&spans, range.from_ms, range.to_ms),
        ));
    }
    let app_spans = metrics::app_spans(&devices, range.from_ms, range.to_ms);
    let app_aggregates = metrics::aggregate_apps(&app_spans);
    let split = if args.split_device {
        devices
            .iter()
            .map(|device| {
                let owned: Vec<metrics::AppAggregate> = app_aggregates
                    .iter()
                    .filter(|app| app.device == device.id)
                    .cloned()
                    .collect();
                (device.id.clone(), device.name.clone(), merge_apps(owned, args.merge_apps == "name"))
            })
            .collect()
    } else {
        Vec::new()
    };
    let merge = match args.merge_apps.as_str() {
        "name" => true,
        "none" => false,
        other => {
            return Err(CliError::usage(format!(
                "无法识别的 --merge-apps「{other}」，可用 none 或 name。"
            )));
        }
    };
    let apps = merge_apps(app_aggregates, merge);
    let app_total = apps.iter().map(|app| app.duration_ms).sum();
    Ok(ReportResult {
        from_ms: range.from_ms,
        to_ms: range.to_ms,
        totals: (
            usage_metrics.total(),
            usage_metrics.overlap(),
            usage_metrics.device_sum(),
        ),
        devices: per_device,
        device_ids: devices.iter().map(|device| device.id.clone()).collect(),
        apps,
        app_total,
        split,
        device_label: device_label(&devices),
    })
}

fn report_json(
    result: &ReportResult,
    zone: &Zone,
    global: &Global,
    top: usize,
) -> Value {
    let apps: Vec<Value> = result
        .apps
        .iter()
        .take(top)
        .map(|app| {
            json!({
                "name": app.name,
                "durationMs": app.duration_ms,
                "duration": time::duration_text(Some(app.duration_ms)),
                "share": if result.app_total > 0 { (app.duration_ms as f64 / result.app_total as f64 * 1000.0).round() / 1000.0 } else { 0.0 },
                "devices": app.devices,
                "pages": app.pages.iter().map(|page| json!({
                    "name": page.name,
                    "kind": page.kind,
                    "durationMs": page.duration_ms,
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    let per_device: Vec<Value> = result
        .devices
        .iter()
        .map(|(id, name, coverage)| {
            json!({
                "id": id,
                "name": name,
                "activeMs": coverage.active_ms,
                "coverage": {
                    "activeMs": coverage.active_ms,
                    "idleMs": coverage.idle_ms,
                    "offlineMs": coverage.offline_ms,
                    "gapMs": coverage.gap_ms,
                    "unobservedMs": coverage.unobserved_ms,
                    "knownMs": coverage.known_ms,
                    "elapsedMs": coverage.elapsed_ms,
                },
            })
        })
        .collect();
    let mut value = json!({
        "schemaVersion": 1,
        "command": "report",
        "generatedAt": data::generated_at(zone),
        "range": data::range_json(zone, result.from_ms, result.to_ms),
        "scope": data::scope_ids(global, &result.device_ids),
        "totals": {
            "activeMs": result.totals.0,
            "overlapMs": result.totals.1,
            "deviceSumMs": result.totals.2,
        },
        "devices": per_device,
        "apps": apps,
    });
    if !result.split.is_empty() {
        let split: Vec<Value> = result
            .split
            .iter()
            .map(|(id, name, apps)| json!({
                "id": id,
                "name": name,
                "apps": apps.iter().take(top).map(|app| json!({
                    "name": app.name,
                    "durationMs": app.duration_ms,
                    "share": if result.app_total > 0 { (app.duration_ms as f64 / result.app_total as f64 * 1000.0).round() / 1000.0 } else { 0.0 },
                })).collect::<Vec<_>>(),
            }))
            .collect();
        if let Some(object) = value.as_object_mut() {
            object.insert("deviceApps".into(), Value::Array(split));
        }
    }
    value
}

fn report_table(result: &ReportResult, zone: &Zone, top: usize) -> String {
    let mut output = String::new();
    output.push_str(&format!(
        "{} · {}\n",
        zone.display(result.from_ms),
        result.device_label
    ));
    output.push_str(&format!(
        "总计（去重）  {}\n",
        time::duration_text(result.totals.0)
    ));
    output.push_str(&format!(
        "设备合计      {}\n",
        time::duration_text(result.totals.2)
    ));
    output.push_str(&format!(
        "重叠          {}\n\n",
        time::duration_text(result.totals.1)
    ));
    let device_rows: Vec<Vec<String>> = result
        .devices
        .iter()
        .map(|(_, name, coverage)| {
            vec![
                name.clone(),
                time::duration_text(Some(coverage.active_ms)),
                if coverage.elapsed_ms > 0 {
                    percent(coverage.known_ms as f64 / coverage.elapsed_ms as f64)
                } else {
                    "—".into()
                },
            ]
        })
        .collect();
    output.push_str(&output::table(
        &["设备", "用时", "覆盖"],
        &[false, true, true],
        &device_rows,
        true,
    ));
    output.push('\n');
    output.push_str(&format!("应用 Top {}\n", top.min(result.apps.len().max(1))));
    let app_rows: Vec<Vec<String>> = result
        .apps
        .iter()
        .take(top)
        .map(|app| {
            vec![
                app.name.clone(),
                time::duration_text(Some(app.duration_ms)),
                if result.app_total > 0 {
                    percent(app.duration_ms as f64 / result.app_total as f64)
                } else {
                    "—".into()
                },
            ]
        })
        .collect();
    output.push_str(&output::table(
        &["应用", "用时", "占比"],
        &[false, true, true],
        &app_rows,
        true,
    ));
    for (_, name, apps) in &result.split {
        output.push_str(&format!("\n{name} 应用\n"));
        let rows: Vec<Vec<String>> = apps
            .iter()
            .take(top)
            .map(|app| {
                vec![
                    app.name.clone(),
                    time::duration_text(Some(app.duration_ms)),
                    if result.app_total > 0 {
                        percent(app.duration_ms as f64 / result.app_total as f64)
                    } else {
                        "—".into()
                    },
                ]
            })
            .collect();
        output.push_str(&output::table(
            &["应用", "用时", "占比"],
            &[false, true, true],
            &rows,
            true,
        ));
    }
    output
}

pub fn report(global: &Global, zone: &Zone, args: &ReportArgs) -> Result<bool, CliError> {
    let format = output::resolve_format(global.format.as_deref())?;
    let result = build_report(global, zone, args)?;
    if format == Format::Json {
        let value = report_json(&result, zone, global, args.top);
        output::emit_json(&value, None)?;
    } else if format == Format::Table {
        output::print_block(&report_table(&result, zone, args.top));
    } else {
        return Err(CliError::usage(
            "report 只支持 table 与 json；需要 CSV 请用 export --dataset report --format json。",
        ));
    }
    Ok(result.apps.is_empty())
}

struct AggRow {
    values: Vec<(String, String)>,
    label: String,
    duration_ms: i64,
    sessions: i64,
    intervals: i64,
}

fn dimension_label(key: &str) -> &'static str {
    match key {
        "day" => "日期",
        "weekday" => "星期",
        "hour" => "小时",
        "device" => "设备",
        "app" => "应用",
        "state" => "状态",
        "page" => "页面",
        "time" => "时段",
        _ => "分组",
    }
}

fn weekday_label(date: chrono::NaiveDate) -> &'static str {
    match date.weekday().num_days_from_monday() {
        0 => "周一",
        1 => "周二",
        2 => "周三",
        3 => "周四",
        4 => "周五",
        5 => "周六",
        _ => "周日",
    }
}

fn group_values(
    span: &Span,
    keys: &[String],
    zone: &Zone,
    fragment_start: i64,
    bucket_start: Option<i64>,
) -> Vec<(String, String)> {
    keys.iter()
        .map(|key| {
            let value = match key.as_str() {
                "day" => zone.date_of(fragment_start).to_string(),
                "weekday" => weekday_label(zone.date_of(fragment_start)).to_owned(),
                "hour" => format!("{:02}:00", zone.datetime(fragment_start).format("%H")),
                "device" => span.device_name.clone(),
                "app" => span
                    .app_name
                    .clone()
                    .filter(|name| !name.trim().is_empty())
                    .unwrap_or_else(|| "未识别".into()),
                "state" => span.state_key().to_owned(),
                "page" => span
                    .page
                    .clone()
                    .filter(|name| !name.trim().is_empty())
                    .unwrap_or_else(|| "未识别".into()),
                "time" => bucket_start
                    .map(|start| zone.display(start))
                    .unwrap_or_else(|| zone.display(fragment_start)),
                _ => String::new(),
            };
            (key.clone(), value)
        })
        .collect()
}

fn composite(values: &[(String, String)]) -> String {
    values
        .iter()
        .map(|(_, value)| value.as_str())
        .collect::<Vec<_>>()
        .join("\u{1f}")
}

fn composite_label(values: &[(String, String)]) -> String {
    values
        .iter()
        .map(|(_, value)| value.as_str())
        .collect::<Vec<_>>()
        .join(" · ")
}

fn bucket_span(span: &Span, bucket_ms: i64, zone: &Zone) -> Vec<(i64, i64, i64)> {
    let day = zone.day_start(zone.date_of(span.start_ms));
    let mut at = span.start_ms;
    let mut pieces = Vec::new();
    while at < span.end_ms {
        let index = (at - day).div_euclid(bucket_ms);
        let bucket_start = day + index * bucket_ms;
        let next = bucket_start + bucket_ms;
        let end = span.end_ms.min(next.max(at + 1));
        pieces.push((at, end, bucket_start));
        at = end;
    }
    pieces
}

pub fn aggregate(global: &Global, zone: &Zone, args: &AggregateArgs) -> Result<bool, CliError> {
    let format = output::resolve_format(global.format.as_deref())?;
    let range = time::resolve(&args.range, zone, global.allow_large).map_err(CliError::usage)?;
    let mut keys: Vec<String> = args
        .group_by
        .split(',')
        .map(|key| key.trim().to_lowercase())
        .filter(|key| !key.is_empty())
        .collect();
    let bucket_ms = args
        .bucket
        .as_deref()
        .map(time::parse_duration)
        .transpose()
        .map_err(CliError::usage)?;
    if bucket_ms.is_some() && !keys.iter().any(|key| key == "time") {
        keys.push("time".into());
    }
    for key in &keys {
        if !matches!(
            key.as_str(),
            "day" | "weekday" | "hour" | "device" | "app" | "state" | "page" | "time"
        ) {
            return Err(CliError::usage(format!(
                "无法识别的分组「{key}」，可用 day、weekday、hour、device、app、state、page、time。"
            )));
        }
        if key == "time" && bucket_ms.is_none() {
            return Err(CliError::usage("group-by time 需要配合 --bucket，例如 --bucket 15m。"));
        }
    }
    let metric = args.metric.as_str();
    if !matches!(metric, "duration" | "sessions" | "intervals" | "session-avg") {
        return Err(CliError::usage(
            "无法识别的指标，可用 duration、sessions、intervals、session-avg。",
        ));
    }
    if !matches!(args.sort.as_str(), "metric" | "key") {
        return Err(CliError::usage("无法识别的排序，可用 metric 或 key。"));
    }
    let states = parse_states(&[args.state.clone()])?;
    let data = data::load(global, range.from_ms, range.to_ms)?;
    let devices = data::select_devices(&data, &args.device)?;
    let spans: Vec<Span> = metrics::usage_spans(&devices, range.from_ms, range.to_ms)
        .into_iter()
        .filter(|span| states.as_ref().is_none_or(|set| set.contains(&span.state)))
        .collect();

    let mut groups: BTreeMap<String, AggRow> = BTreeMap::new();
    if matches!(metric, "duration" | "intervals") {
        for span in &spans {
            let pieces = match bucket_ms {
                Some(bucket) => bucket_span(span, bucket, zone),
                None => vec![(span.start_ms, span.end_ms, span.start_ms)],
            };
            for (start, end, bucket_start) in pieces {
                let values = group_values(span, &keys, zone, start, Some(bucket_start));
                let key = composite(&values);
                let entry = groups.entry(key).or_insert_with(|| AggRow {
                    label: composite_label(&values),
                    values,
                    duration_ms: 0,
                    sessions: 0,
                    intervals: 0,
                });
                entry.duration_ms += end - start;
                entry.intervals += 1;
            }
        }
    } else {
        let sessions = metrics::app_sessions(&spans, metrics::SESSION_GAP_MS);
        for session in &sessions {
            let pseudo = Span {
                device: session.device.clone(),
                device_name: session.device_name.clone(),
                start_ms: session.start_ms,
                end_ms: session.end_ms,
                state: 0,
                app_id: Some(session.app_id.clone()),
                app_name: Some(session.name.clone()),
                page: None,
                page_kind: None,
                playback: false,
            };
            let pieces = match bucket_ms {
                Some(bucket) => bucket_span(&pseudo, bucket, zone),
                None => vec![(session.start_ms, session.end_ms, session.start_ms)],
            };
            for (start, end, bucket_start) in pieces {
                let values = group_values(&pseudo, &keys, zone, start, Some(bucket_start));
                let key = composite(&values);
                let entry = groups.entry(key).or_insert_with(|| AggRow {
                    label: composite_label(&values),
                    values,
                    duration_ms: 0,
                    sessions: 0,
                    intervals: 0,
                });
                entry.duration_ms += end - start;
                entry.sessions += 1;
                entry.intervals += session.spans as i64;
            }
        }
    }

    let total_duration: i64 = groups.values().map(|row| row.duration_ms).sum();
    let mut rows: Vec<AggRow> = groups.into_values().collect();
    if args.sort == "key" {
        rows.sort_by(|left, right| left.label.cmp(&right.label));
    } else {
        rows.sort_by(|left, right| {
            let left_value = match metric {
                "sessions" => left.sessions,
                "session-avg" => {
                    if left.sessions > 0 { left.duration_ms / left.sessions } else { 0 }
                }
                "intervals" => left.intervals,
                _ => left.duration_ms,
            };
            let right_value = match metric {
                "sessions" => right.sessions,
                "session-avg" => {
                    if right.sessions > 0 { right.duration_ms / right.sessions } else { 0 }
                }
                "intervals" => right.intervals,
                _ => right.duration_ms,
            };
            right_value
                .cmp(&left_value)
                .then_with(|| left.label.cmp(&right.label))
        });
    }
    let limit = args
        .top
        .unwrap_or_else(|| row_limit(global, format, 200, 1000));
    let total_groups = rows.len();
    let truncated = rows.len() > limit;
    if truncated {
        rows.truncate(limit);
    }

    let mut headers: Vec<&'static str> = keys.iter().map(|key| dimension_label(key)).collect();
    let mut numeric = vec![false; keys.len()];
    match metric {
        "sessions" => {
            headers.extend(["会话数", "时长", "占比"]);
            numeric.extend([true, true, true]);
        }
        "intervals" => {
            headers.extend(["区间数", "时长", "占比"]);
            numeric.extend([true, true, true]);
        }
        "session-avg" => {
            headers.extend(["平均时长", "会话数", "占比"]);
            numeric.extend([true, true, true]);
        }
        _ => {
            headers.extend(["时长", "占比"]);
            numeric.extend([true, true]);
        }
    }
    let share = |duration: i64| {
        if total_duration > 0 {
            percent(duration as f64 / total_duration as f64)
        } else {
            "—".into()
        }
    };
    let mut cells = Vec::new();
    let mut values = Vec::new();
    for row in &rows {
        let mut cell: Vec<String> = row.values.iter().map(|(_, value)| value.clone()).collect();
        let object: serde_json::Map<String, Value> = row
            .values
            .iter()
            .map(|(key, value)| (key.clone(), Value::String(value.clone())))
            .collect();
        let mut value = json!({
            "group": object,
            "label": row.label,
            "durationMs": row.duration_ms,
            "duration": time::duration_text(Some(row.duration_ms)),
            "sessions": row.sessions,
            "intervals": row.intervals,
            "share": if total_duration > 0 { (row.duration_ms as f64 / total_duration as f64 * 1000.0).round() / 1000.0 } else { 0.0 },
        });
        match metric {
            "sessions" => {
                cell.push(thousands(row.sessions));
                cell.push(time::duration_text(Some(row.duration_ms)));
                cell.push(share(row.duration_ms));
            }
            "intervals" => {
                cell.push(thousands(row.intervals));
                cell.push(time::duration_text(Some(row.duration_ms)));
                cell.push(share(row.duration_ms));
            }
            "session-avg" => {
                let average = if row.sessions > 0 {
                    row.duration_ms / row.sessions
                } else {
                    0
                };
                cell.push(time::duration_text(Some(average)));
                cell.push(thousands(row.sessions));
                cell.push(share(row.duration_ms));
                if let Some(object) = value.as_object_mut() {
                    object.insert("averageMs".into(), json!(average));
                }
            }
            _ => {
                cell.push(time::duration_text(Some(row.duration_ms)));
                cell.push(share(row.duration_ms));
            }
        }
        cells.push(cell);
        values.push(value);
    }

    if format == Format::Json {
        let value = json!({
            "schemaVersion": 1,
            "command": "aggregate",
            "generatedAt": data::generated_at(zone),
            "range": data::range_json(zone, range.from_ms, range.to_ms),
            "scope": data::scope_json(global, &devices),
            "groupBy": keys,
            "bucket": args.bucket,
            "metric": metric,
            "totalDurationMs": total_duration,
            "rows": values,
        });
        output::emit_json(&value, None)?;
    } else if format == Format::Table {
        if truncated {
            eprintln!(
                "tickmark: 共 {total_groups} 组，已按上限显示 {} 组，可用 --top 或 --limit 调整。",
                limit
            );
        }
        output::print_block(&output::table(&headers, &numeric, &cells, global.wide));
    } else {
        let text = output::csv(&headers, &cells, true);
        output::write_text(&text, None)?;
    }
    Ok(rows.is_empty())
}

pub struct QuietOut {
    pub start_ms: i64,
    pub end_ms: i64,
    pub reason: &'static str,
    pub compressed: bool,
}

pub struct TimelineResult {
    pub from_ms: i64,
    pub to_ms: i64,
    pub device_ids: Vec<String>,
    pub items: Vec<TimelineItem>,
    pub quiet: Vec<QuietOut>,
    pub smooth_ms: i64,
    pub raw: bool,
}

fn quiet_label(span: metrics::Interval, raw: &[Span], zone: &Zone) -> &'static str {
    let rows: Vec<&Span> = raw
        .iter()
        .filter(|row| row.start_ms < span.end_ms && row.end_ms > span.start_ms)
        .collect();
    if rows.is_empty() {
        return "no-record";
    }
    if rows.iter().any(|row| row.state == 0) {
        return "no-activity";
    }
    let night_hour = zone.datetime(span.start_ms).format("%H").to_string();
    let hour: i64 = night_hour.parse().unwrap_or(12);
    if span.end_ms - span.start_ms >= 2 * 3_600_000 && (hour >= 22 || hour < 8) {
        return "sleep";
    }
    if rows.iter().all(|row| row.state == 1) {
        return "idle";
    }
    if rows.iter().any(|row| row.state == 2) {
        return "offline";
    }
    if rows.iter().all(|row| row.state == 3) {
        return "gap";
    }
    "no-activity"
}

fn build_timeline(
    global: &Global,
    zone: &Zone,
    args: &TimelineArgs,
) -> Result<TimelineResult, CliError> {
    let range = time::resolve(&args.range, zone, global.allow_large).map_err(CliError::usage)?;
    let smooth_ms = if args.raw {
        0
    } else {
        time::parse_duration(&args.smooth).map_err(CliError::usage)?
    };
    let data = data::load(global, range.from_ms, range.to_ms)?;
    let devices = data::select_devices(&data, &args.device)?;
    let spans = metrics::usage_spans(&devices, range.from_ms, range.to_ms);
    let mut by_device: BTreeMap<String, Vec<&Span>> = BTreeMap::new();
    for span in &spans {
        by_device.entry(span.device.clone()).or_default().push(span);
    }
    let mut items = Vec::new();
    for mut device_spans in by_device.into_values() {
        device_spans.sort_by_key(|span| (span.start_ms, span.end_ms));
        let mapped: Vec<TimelineItem> = device_spans.iter().map(|span| TimelineItem::from_span(span)).collect();
        let merged = if smooth_ms > 0 {
            timeline::coalesce(&mapped, smooth_ms)
        } else {
            mapped
        };
        items.extend(merged);
    }
    items.sort_by_key(|item| (item.start_ms, item.end_ms, item.device.clone()));
    let active = metrics::merge_ranges(
        items
            .iter()
            .filter(|item| item.state == 0)
            .map(|item| (item.start_ms, item.end_ms))
            .collect(),
    );
    let quiet_ranges = metrics::quiet_ranges(&active, range.from_ms, range.to_ms);
    let now = chrono::Utc::now().timestamp_millis();
    let compressed = if args.raw {
        vec![false; quiet_ranges.len()]
    } else {
        timeline::compressed_quiet(&quiet_ranges)
    };
    let mut quiet = Vec::new();
    for (index, span) in quiet_ranges.iter().enumerate() {
        if !args.raw && span.start_ms < now && span.end_ms > now {
            let first = metrics::Interval { start_ms: span.start_ms, end_ms: now };
            quiet.push(QuietOut {
                start_ms: first.start_ms,
                end_ms: first.end_ms,
                reason: quiet_label(first, &spans, zone),
                compressed: compressed[index],
            });
            quiet.push(QuietOut {
                start_ms: now,
                end_ms: span.end_ms,
                reason: "future",
                compressed: compressed[index],
            });
        } else {
            quiet.push(QuietOut {
                start_ms: span.start_ms,
                end_ms: span.end_ms,
                reason: if span.start_ms >= now {
                    "future"
                } else {
                    quiet_label(*span, &spans, zone)
                },
                compressed: compressed[index],
            });
        }
    }
    Ok(TimelineResult {
        from_ms: range.from_ms,
        to_ms: range.to_ms,
        device_ids: devices.iter().map(|device| device.id.clone()).collect(),
        items,
        quiet,
        smooth_ms,
        raw: args.raw,
    })
}

fn timeline_json(result: &TimelineResult, zone: &Zone, global: &Global) -> Value {
    let items: Vec<Value> = result
        .items
        .iter()
        .map(|item| {
            let mut value = json!({
                "deviceId": item.device,
                "deviceName": item.device_name,
                "startMs": item.start_ms,
                "endMs": item.end_ms,
                "durationMs": item.end_ms - item.start_ms,
                "state": item.state,
                "stateKey": item.state_key(),
                "appId": item.app_id,
                "appName": item.app_name,
                "playback": item.playback,
                "merged": item.merged,
            });
            if !global.no_pages {
                if let Some(object) = value.as_object_mut() {
                    object.insert("page".into(), json!(item.page));
                    object.insert("pageKind".into(), json!(item.page_kind));
                }
            }
            value
        })
        .collect();
    let quiet: Vec<Value> = result
        .quiet
        .iter()
        .map(|span| {
            json!({
                "startMs": span.start_ms,
                "endMs": span.end_ms,
                "reason": span.reason,
                "compressed": span.compressed,
            })
        })
        .collect();
    json!({
        "schemaVersion": 1,
        "command": "timeline",
        "generatedAt": data::generated_at(zone),
        "range": data::range_json(zone, result.from_ms, result.to_ms),
        "scope": data::scope_ids(global, &result.device_ids),
        "smoothingMs": result.smooth_ms,
        "raw": result.raw,
        "items": items,
        "quiet": quiet,
    })
}

pub fn timeline(global: &Global, zone: &Zone, args: &TimelineArgs) -> Result<bool, CliError> {
    let format = output::resolve_format(global.format.as_deref())?;
    let result = build_timeline(global, zone, args)?;
    if format == Format::Json {
        output::emit_json(&timeline_json(&result, zone, global), None)?;
    } else if format == Format::Table {
        let active: i64 = result
            .items
            .iter()
            .filter(|item| item.state == 0)
            .map(|item| item.end_ms - item.start_ms)
            .sum();
        let quiet_ms: i64 = result.quiet.iter().map(|span| span.end_ms - span.start_ms).sum();
        let summary = format!(
            "{} – {}\n活动段    {} 段，{}\n无活动    {} 段，{}\n平滑      {}\n",
            zone.display(result.from_ms),
            zone.display(result.to_ms),
            thousands(result.items.iter().filter(|item| item.state == 0).count() as i64),
            time::duration_text(Some(active)),
            thousands(result.quiet.len() as i64),
            time::duration_text(Some(quiet_ms)),
            if result.raw {
                "关闭".to_owned()
            } else {
                format!("{} 毫秒", result.smooth_ms)
            }
        );
        output::print_block(&summary);
    } else {
        return Err(CliError::usage("timeline 只支持 table 与 json。"));
    }
    Ok(result.items.is_empty())
}

pub struct OverlapResult {
    pub from_ms: i64,
    pub to_ms: i64,
    pub device_ids: Vec<String>,
    pub total: Option<i64>,
    pub overlap: Option<i64>,
    pub pairs: Vec<metrics::PairOverlap>,
    pub spans: Vec<metrics::OverlapSpan>,
}

fn build_overlap(global: &Global, zone: &Zone, args: &OverlapArgs) -> Result<OverlapResult, CliError> {
    let range = time::resolve(&args.range, zone, global.allow_large).map_err(CliError::usage)?;
    let data = data::load(global, range.from_ms, range.to_ms)?;
    let devices = data::select_devices(&data, &args.device)?;
    let spans = metrics::usage_spans(&devices, range.from_ms, range.to_ms);
    let usage = metrics::usage_metrics(&spans, true);
    let total = usage.total();
    let overlap = usage.overlap();
    let mut longest = usage.overlap_spans;
    longest.sort_by(|left, right| {
        (right.end_ms - right.start_ms)
            .cmp(&(left.end_ms - left.start_ms))
            .then_with(|| left.start_ms.cmp(&right.start_ms))
    });
    longest.truncate(args.top);
    Ok(OverlapResult {
        from_ms: range.from_ms,
        to_ms: range.to_ms,
        device_ids: devices.iter().map(|device| device.id.clone()).collect(),
        total,
        overlap,
        pairs: usage.pairs,
        spans: longest,
    })
}

pub fn overlap(global: &Global, zone: &Zone, args: &OverlapArgs) -> Result<bool, CliError> {
    let format = output::resolve_format(global.format.as_deref())?;
    let result = build_overlap(global, zone, args)?;
    if format == Format::Json {
        let pairs: Vec<Value> = result
            .pairs
            .iter()
            .map(|pair| {
                json!({
                    "devices": pair.devices,
                    "durationMs": pair.ms,
                    "duration": time::duration_text(Some(pair.ms)),
                })
            })
            .collect();
        let spans: Vec<Value> = result
            .spans
            .iter()
            .map(|span| {
                json!({
                    "startMs": span.start_ms,
                    "endMs": span.end_ms,
                    "start": zone.iso(span.start_ms),
                    "end": zone.iso(span.end_ms),
                    "durationMs": span.end_ms - span.start_ms,
                    "devices": span.devices,
                })
            })
            .collect();
        let value = json!({
            "schemaVersion": 1,
            "command": "overlap",
            "generatedAt": data::generated_at(zone),
            "range": data::range_json(zone, result.from_ms, result.to_ms),
            "scope": data::scope_ids(global, &result.device_ids),
            "totals": { "activeMs": result.total, "overlapMs": result.overlap },
            "pairs": pairs,
            "longest": spans,
        });
        output::emit_json(&value, None)?;
    } else if format == Format::Table {
        let mut text = format!(
            "重叠总计  {}\n\n",
            time::duration_text(result.overlap)
        );
        let pairs: Vec<Vec<String>> = result
            .pairs
            .iter()
            .map(|pair| {
                vec![
                    pair.devices.join(" + "),
                    time::duration_text(Some(pair.ms)),
                ]
            })
            .collect();
        text.push_str(&output::table(&["设备组合", "重叠"], &[false, true], &pairs, global.wide));
        if !result.spans.is_empty() {
            text.push_str("\n最长并发\n");
            let spans: Vec<Vec<String>> = result
                .spans
                .iter()
                .map(|span| {
                    vec![
                        format!("{}–{}", zone.display(span.start_ms), zone.display(span.end_ms)),
                        time::duration_text(Some(span.end_ms - span.start_ms)),
                        span.devices.join(" + "),
                    ]
                })
                .collect();
            text.push_str(&output::table(
                &["时段", "时长", "设备"],
                &[false, true, false],
                &spans,
                global.wide,
            ));
        }
        output::print_block(&text);
    } else {
        return Err(CliError::usage("overlap 只支持 table 与 json。"));
    }
    Ok(result.pairs.is_empty())
}

pub fn export(global: &Global, zone: &Zone, args: &ExportArgs) -> Result<bool, CliError> {
    let format = match global.format.as_deref() {
        Some(value) => Format::parse(value)?,
        None => match args.out.as_deref() {
            Some("-") | None => output::resolve_format(None)?,
            Some(_) => Format::Csv,
        },
    };
    let out = args.out.as_deref();
    let range = time::resolve(&args.range, zone, global.allow_large).map_err(CliError::usage)?;
    match args.dataset.as_str() {
        "intervals" => {
            let data = data::load(global, range.from_ms, range.to_ms)?;
            let devices = data::select_devices(&data, &args.device)?;
            let spans: Vec<Span> = metrics::usage_spans(&devices, range.from_ms, range.to_ms)
                .into_iter()
                .filter(|span| app_matches(span, &args.app))
                .collect();
            let table = interval_table(&spans, zone, global.no_pages);
            let ids: Vec<String> = devices.iter().map(|device| device.id.clone()).collect();
            emit_table(global, format, &table, out, !args.no_bom, "export.intervals", zone, range.from_ms, range.to_ms, &ids)?;
            Ok(spans.is_empty())
        }
        "apps" => {
            let data = data::load(global, range.from_ms, range.to_ms)?;
            let devices = data::select_devices(&data, &args.device)?;
            let spans: Vec<Span> = metrics::app_spans(&devices, range.from_ms, range.to_ms)
                .into_iter()
                .filter(|span| app_matches(span, &args.app))
                .collect();
            let sessions = metrics::app_sessions(&spans, metrics::SESSION_GAP_MS);
            let rows = session_rows(&sessions, false);
            let table = app_table(&rows, true, zone);
            let ids: Vec<String> = devices.iter().map(|device| device.id.clone()).collect();
            emit_table(global, format, &table, out, !args.no_bom, "export.apps", zone, range.from_ms, range.to_ms, &ids)?;
            Ok(sessions.is_empty())
        }
        "report" | "timeline" => {
            if !matches!(format, Format::Json | Format::Ndjson) {
                return Err(CliError::usage(
                    "report 与 timeline 数据集只支持 json 或 ndjson。",
                ));
            }
            let (value, empty) = if args.dataset == "report" {
                let report_args = ReportArgs {
                    range: args.range.clone(),
                    device: args.device.clone(),
                    top: global.limit.unwrap_or(usize::MAX),
                    merge_apps: "name".into(),
                    split_device: false,
                };
                let result = build_report(global, zone, &report_args)?;
                (
                    report_json(&result, zone, global, usize::MAX),
                    result.apps.is_empty(),
                )
            } else {
                let timeline_args = TimelineArgs {
                    range: args.range.clone(),
                    device: args.device.clone(),
                    raw: false,
                    smooth: "2s".into(),
                };
                let result = build_timeline(global, zone, &timeline_args)?;
                (
                    timeline_json(&result, zone, global),
                    result.items.is_empty(),
                )
            };
            let text = if format == Format::Json {
                let mut text = serde_json::to_string_pretty(&value)
                    .map_err(|error| CliError::error(error.to_string()))?;
                text.push('\n');
                text
            } else {
                let meta = json!({
                    "type": "meta",
                    "schemaVersion": 1,
                    "command": format!("export.{}", args.dataset),
                    "data": value,
                });
                output::emit_ndjson(&meta, &[])
            };
            output::write_text(&text, out)?;
            Ok(empty)
        }
        other => Err(CliError::usage(format!(
            "无法识别的数据集「{other}」，可用 intervals、apps、report、timeline。"
        ))),
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_table(
    global: &Global,
    format: Format,
    table: &Tabular,
    out: Option<&str>,
    bom: bool,
    command: &str,
    zone: &Zone,
    from_ms: i64,
    to_ms: i64,
    device_ids: &[String],
) -> Result<(), CliError> {
    let text = match format {
        Format::Json => {
            let value = json!({
                "schemaVersion": 1,
                "command": command,
                "generatedAt": data::generated_at(zone),
                "range": data::range_json(zone, from_ms, to_ms),
                "scope": data::scope_ids(global, device_ids),
                "rows": table.rows,
            });
            let mut text = serde_json::to_string_pretty(&value)
                .map_err(|error| CliError::error(error.to_string()))?;
            text.push('\n');
            text
        }
        Format::Table => output::table(&table.headers, &table.numeric, &table.cells, global.wide),
        Format::Csv => output::csv(&table.headers, &table.cells, bom),
        Format::Ndjson => {
            let meta = json!({
                "type": "meta",
                "schemaVersion": 1,
                "command": command,
                "range": data::range_json(zone, from_ms, to_ms),
                "scope": data::scope_ids(global, device_ids),
            });
            output::emit_ndjson(&meta, &table.rows)
        }
    };
    output::write_text(&text, out)
}

pub fn import(global: &Global, zone: &Zone, args: &crate::cli::ImportArgs) -> Result<bool, CliError> {
    if args.write && args.dry_run {
        return Err(CliError::usage("--write 与 --dry-run 不能同时使用。"));
    }
    let content = std::fs::read_to_string(&args.file)
        .map_err(|error| CliError::error(format!("无法读取 {}：{error}", args.file.display())))?;
    let dir = data::resolve_dir(global);
    let preview = tickmark_report::preview(&dir, &content).map_err(CliError::rejected)?;
    if let Some(conflict) = &preview.conflict {
        return Err(CliError::rejected(conflict.clone()));
    }
    let format = output::resolve_format(global.format.as_deref())?;
    let written = args.write;
    let name = if written {
        tickmark_report::import(&dir, &content).map_err(CliError::rejected)?
    } else {
        preview.name.clone()
    };
    if format == Format::Json {
        let value = json!({
            "schemaVersion": 1,
            "command": "import",
            "generatedAt": data::generated_at(zone),
            "written": written,
            "deviceId": preview.device_id,
            "name": name,
            "from": zone.iso(preview.from_ms),
            "until": zone.iso(preview.until_ms),
            "intervals": preview.intervals,
            "appIntervals": preview.app_intervals,
            "revision": preview.revision,
            "existing": preview.existing,
        });
        output::emit_json(&value, None)?;
    } else {
        let mut lines = vec![
            format!("设备      {}（{}）", name, preview.device_id),
            format!(
                "范围      {} – {}",
                zone.display(preview.from_ms),
                zone.display(preview.until_ms)
            ),
            format!("区间      {}", thousands(preview.intervals as i64)),
            format!("应用会话  {}", thousands(preview.app_intervals as i64)),
            format!("修订号    {}", preview.revision),
            format!("已有镜像  {}", if preview.existing { "是" } else { "否" }),
            format!(
                "状态      {}",
                if written {
                    "已写入"
                } else {
                    "仅检查，未写入（加 --write 才会保存）"
                }
            ),
        ];
        if !written {
            lines.push("提示      导入会按报告范围替换该设备在相同时间段的旧数据。".into());
        }
        output::print_block(&(lines.join("\n") + "\n"));
    }
    Ok(false)
}
