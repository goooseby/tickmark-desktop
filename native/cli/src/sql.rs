use std::time::{Duration, Instant};

use rusqlite::{Connection, types::ValueRef};
use serde_json::{Value, json};

use tickmark_report::metrics;

use crate::{
    cli::{Global, SqlArgs},
    data,
    errors::CliError,
    output::{self, Format},
    time::{self, Zone},
};

const DEFAULT_LIMIT: usize = 1_000;
const TIMEOUT: Duration = Duration::from_secs(10);

fn validate(sql: &str) -> Result<(), CliError> {
    let trimmed = sql.trim().trim_end_matches(';').trim();
    if trimmed.is_empty() {
        return Err(CliError::usage("SQL 不能为空。"));
    }
    if trimmed.contains(';') {
        return Err(CliError::rejected("只允许执行一条语句。"));
    }
    let first = trimmed
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    if first != "select" && first != "with" {
        return Err(CliError::rejected("只允许只读查询：语句需以 SELECT 或 WITH 开头。"));
    }
    let words: Vec<String> = trimmed
        .split(|ch: char| !ch.is_alphanumeric() && ch != '_')
        .filter(|word| !word.is_empty())
        .map(|word| word.to_ascii_lowercase())
        .collect();
    for banned in [
        "attach", "detach", "pragma", "insert", "update", "delete", "drop", "create", "alter",
        "replace", "vacuum", "reindex", "begin", "commit", "rollback",
    ] {
        if words.iter().any(|word| word == banned) {
            return Err(CliError::rejected(format!(
                "SQL 包含不允许的关键字「{banned}」，这里只接受只读查询。"
            )));
        }
    }
    Ok(())
}

fn has_limit(sql: &str) -> bool {
    sql.split(|ch: char| !ch.is_alphanumeric() && ch != '_')
        .any(|word| word.eq_ignore_ascii_case("limit"))
}

fn value_ref_to_json(value: ValueRef<'_>) -> Value {
    match value {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(number) => json!(number),
        ValueRef::Real(number) => json!(number),
        ValueRef::Text(text) => json!(String::from_utf8_lossy(text)),
        ValueRef::Blob(bytes) => json!(format!("<{} 字节>", bytes.len())),
    }
}

fn cell(value: &Value) -> String {
    match value {
        Value::Null => "—".into(),
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

fn populate(connection: &Connection, data: &tickmark_report::ReportData, devices: &[&tickmark_report::Device], from_ms: i64, to_ms: i64) -> Result<(), CliError> {
    connection
        .execute_batch(
            "CREATE TABLE devices (id TEXT PRIMARY KEY, name TEXT, kind TEXT, platform TEXT,
                source TEXT, first_ms INTEGER, last_ms INTEGER);
             CREATE TABLE intervals (device_id TEXT, start_ms INTEGER, end_ms INTEGER, state INTEGER,
                state_key TEXT, app_id TEXT, app_name TEXT, page TEXT, page_kind TEXT, playback INTEGER);
             CREATE TABLE app_sessions (device_id TEXT, app_id TEXT, app_name TEXT,
                start_ms INTEGER, end_ms INTEGER);
             CREATE INDEX intervals_device ON intervals(device_id);
             CREATE INDEX intervals_start ON intervals(start_ms);
             CREATE INDEX sessions_app ON app_sessions(app_id);",
        )
        .map_err(|error| CliError::error(format!("无法建立查询库：{error}")))?;
    for device in devices {
        let first = device.rows.iter().map(|row| row.start_ms).min();
        let last = device.rows.iter().map(|row| row.end_ms).max();
        let source = if device.local {
            "local"
        } else if device.cloud {
            "cloud"
        } else {
            "import"
        };
        connection
            .execute(
                "INSERT INTO devices VALUES (?,?,?,?,?,?,?)",
                rusqlite::params![
                    device.id,
                    device.name,
                    device.kind,
                    device.platform,
                    source,
                    first,
                    last
                ],
            )
            .map_err(|error| CliError::error(error.to_string()))?;
    }
    let spans = metrics::usage_spans(devices, from_ms, to_ms);
    for span in &spans {
        connection
            .execute(
                "INSERT INTO intervals VALUES (?,?,?,?,?,?,?,?,?,?)",
                rusqlite::params![
                    span.device,
                    span.start_ms,
                    span.end_ms,
                    span.state,
                    span.state_key(),
                    span.app_id,
                    span.app_name,
                    span.page,
                    span.page_kind,
                    span.playback as i64
                ],
            )
            .map_err(|error| CliError::error(error.to_string()))?;
    }
    let app_spans = metrics::app_spans(devices, from_ms, to_ms);
    for session in metrics::app_sessions(&app_spans, metrics::SESSION_GAP_MS) {
        connection
            .execute(
                "INSERT INTO app_sessions VALUES (?,?,?,?,?)",
                rusqlite::params![
                    session.device,
                    session.app_id,
                    session.name,
                    session.start_ms,
                    session.end_ms
                ],
            )
            .map_err(|error| CliError::error(error.to_string()))?;
    }
    let _ = data;
    Ok(())
}

pub fn run(global: &Global, zone: &Zone, args: &SqlArgs) -> Result<bool, CliError> {
    let query = match (&args.query, &args.file) {
        (Some(query), None) => query.clone(),
        (None, Some(path)) => std::fs::read_to_string(path)
            .map_err(|error| CliError::error(format!("无法读取 {}：{error}", path.display())))?,
        (Some(_), Some(_)) => {
            return Err(CliError::usage("SQL 与 --file 只能选择一个。"));
        }
        (None, None) => {
            return Err(CliError::usage("请提供一条 SQL，或用 --file 指定文件。"));
        }
    };
    validate(&query)?;
    let query = query.trim().trim_end_matches(';').trim().to_owned();
    let format = output::resolve_format(global.format.as_deref())?;
    let range = time::resolve(&args.range, zone, global.allow_large).map_err(CliError::usage)?;
    let data = data::load(global, range.from_ms, range.to_ms)?;
    let devices = data::select_devices(&data, &args.device)?;
    let connection = Connection::open_in_memory()
        .map_err(|error| CliError::error(format!("无法建立查询库：{error}")))?;
    populate(&connection, &data, &devices, range.from_ms, range.to_ms)?;
    let started = Instant::now();
    connection.progress_handler(10_000, Some(move || started.elapsed() > TIMEOUT));
    let limit = args.limit.unwrap_or(DEFAULT_LIMIT);
    let limited = if has_limit(&query) {
        query.clone()
    } else {
        format!("{query} LIMIT {limit}")
    };
    let mut statement = connection
        .prepare(&limited)
        .map_err(|error| CliError::rejected(format!("SQL 无法执行：{error}")))?;
    let columns: Vec<String> = statement
        .column_names()
        .into_iter()
        .map(str::to_owned)
        .collect();
    let mut rows = statement
        .query([])
        .map_err(|error| CliError::rejected(format!("SQL 无法执行：{error}")))?;
    let mut values: Vec<Value> = Vec::new();
    loop {
        let row = match rows.next() {
            Ok(Some(row)) => row,
            Ok(None) => break,
            Err(error) if error.to_string().to_lowercase().contains("interrupt") => {
                return Err(CliError::rejected(
                    "查询超过 10 秒已中断，请缩小范围或增加筛选条件。",
                ));
            }
            Err(error) => return Err(CliError::rejected(format!("SQL 执行失败：{error}"))),
        };
        let mut object = serde_json::Map::new();
        for (index, column) in columns.iter().enumerate() {
            let value = row
                .get_ref(index)
                .map(value_ref_to_json)
                .unwrap_or(Value::Null);
            object.insert(column.clone(), value);
        }
        values.push(Value::Object(object));
    }
    let empty = values.is_empty();
    match format {
        Format::Json => {
            let value = json!({
                "schemaVersion": 1,
                "command": "sql",
                "generatedAt": data::generated_at(zone),
                "range": data::range_json(zone, range.from_ms, range.to_ms),
                "scope": data::scope_json(global, &devices),
                "columns": columns,
                "rows": values,
            });
            output::emit_json(&value, None)?;
        }
        Format::Table => {
            let cells: Vec<Vec<String>> = values
                .iter()
                .map(|row| {
                    columns
                        .iter()
                        .map(|column| cell(row.get(column).unwrap_or(&Value::Null)))
                        .collect()
                })
                .collect();
            let headers: Vec<&str> = columns.iter().map(String::as_str).collect();
            let numeric: Vec<bool> = (0..columns.len()).map(|_| false).collect();
            output::print_block(&output::table(&headers, &numeric, &cells, global.wide));
        }
        Format::Csv => {
            let cells: Vec<Vec<String>> = values
                .iter()
                .map(|row| {
                    columns
                        .iter()
                        .map(|column| cell(row.get(column).unwrap_or(&Value::Null)))
                        .collect()
                })
                .collect();
            let headers: Vec<&str> = columns.iter().map(String::as_str).collect();
            output::write_text(&output::csv(&headers, &cells, true), None)?;
        }
        Format::Ndjson => {
            let meta = json!({
                "type": "meta",
                "schemaVersion": 1,
                "command": "sql",
                "range": data::range_json(zone, range.from_ms, range.to_ms),
                "columns": columns,
            });
            output::write_text(&output::emit_ndjson(&meta, &values), None)?;
        }
    }
    Ok(empty)
}
