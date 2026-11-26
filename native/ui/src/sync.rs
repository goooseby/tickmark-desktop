use std::{path::Path, time::{Duration, SystemTime, UNIX_EPOCH}};

use chrono::{DateTime, NaiveDate};
use reqwest::StatusCode;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use tauri::Manager;

use tickmark_report::cloud::{AppInterval, CloudDevice, DeviceInterval, Payload};
pub use tickmark_report::cloud::reports;

use crate::{account::{AccountError, AccountService, SyncSession}, dashboard_store, error_text};

const DAY_MS: i64 = 86_400_000;
const MAX_RESPONSE: usize = 9_000_000;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Delta {
    epoch: String,
    changes: Vec<Change>,
    devices: Vec<CloudDevice>,
    next_after: i64,
    has_more: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Change {
    device_id: String,
    day_utc: String,
    revision: i64,
    seq: i64,
    payload_version: i64,
    payload: Option<Payload>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Shard {
    day_utc: String,
    revision: i64,
    payload_version: i64,
    payload: serde_json::Value,
}

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as i64
}

fn open(path: &Path) -> Result<Connection, String> {
    std::fs::create_dir_all(path.parent().ok_or("invalid sync storage")?).map_err(error_text)?;
    let db = Connection::open(path).map_err(error_text)?;
    db.busy_timeout(Duration::from_secs(3)).map_err(error_text)?;
    db.execute_batch("PRAGMA journal_mode=WAL;
        CREATE TABLE IF NOT EXISTS cursor (slot INTEGER PRIMARY KEY, epoch TEXT, seq INTEGER NOT NULL);
        INSERT OR IGNORE INTO cursor VALUES (1, NULL, 0);
        CREATE TABLE IF NOT EXISTS devices (id TEXT PRIMARY KEY, name TEXT NOT NULL, kind TEXT NOT NULL,
            platform TEXT NOT NULL, seen INTEGER NOT NULL);
        CREATE TABLE IF NOT EXISTS shards (device TEXT NOT NULL, day INTEGER NOT NULL, revision INTEGER NOT NULL,
            payload TEXT, PRIMARY KEY(device,day));
        CREATE TABLE IF NOT EXISTS writer (slot INTEGER PRIMARY KEY, start_ms INTEGER NOT NULL, next_day INTEGER NOT NULL);
        CREATE TABLE IF NOT EXISTS uploads (day INTEGER PRIMARY KEY, revision INTEGER NOT NULL, payload TEXT NOT NULL,
            acknowledged INTEGER NOT NULL);").map_err(error_text)?;
    Ok(db)
}

fn save_delta(db: &mut Connection, delta: Delta, own: &str, after: i64, epoch: Option<&str>) -> Result<(), String> {
    if delta.epoch.is_empty() || epoch.is_some_and(|old| old != delta.epoch) || delta.next_after < after ||
        (delta.has_more && delta.next_after <= after) {
        return Err("同步游标无效".into());
    }
    let tx = db.transaction().map_err(error_text)?;
    let mut last = after;
    for item in delta.changes {
        if item.payload_version != 1 || item.revision <= 0 || item.seq <= last || item.seq > delta.next_after {
            return Err("云端分片版本无效".into());
        }
        last = item.seq;
        if item.device_id == own { continue; }
        let day = NaiveDate::parse_from_str(&item.day_utc, "%Y-%m-%d").map_err(error_text)?
            .and_hms_opt(0, 0, 0).ok_or("无效日期")?.and_utc().timestamp_millis();
        let payload = item.payload.map(|data| serde_json::to_string(&data)).transpose().map_err(error_text)?;
        tx.execute("INSERT INTO shards VALUES (?1,?2,?3,?4)
            ON CONFLICT(device,day) DO UPDATE SET revision=excluded.revision,payload=excluded.payload
            WHERE excluded.revision >= shards.revision", params![item.device_id, day, item.revision, payload]).map_err(error_text)?;
    }
    tx.execute("DELETE FROM devices", []).map_err(error_text)?;
    for item in delta.devices.into_iter().filter(|device| device.device_id != own) {
        tx.execute("INSERT INTO devices VALUES (?1,?2,?3,?4,?5)",
            params![item.device_id, item.name, item.kind, item.platform, item.last_seen_at]).map_err(error_text)?;
    }
    tx.execute("UPDATE cursor SET epoch=?1,seq=?2 WHERE slot=1", params![delta.epoch, delta.next_after]).map_err(error_text)?;
    tx.commit().map_err(error_text)
}

async fn receive(account: &AccountService, session: &SyncSession, db: &mut Connection) -> Result<Option<String>, String> {
    for _ in 0..8 {
        let (epoch, after) = db.query_row("SELECT epoch,seq FROM cursor WHERE slot=1", [], |row| {
            Ok((row.get::<_, Option<String>>(0)?, row.get::<_, i64>(1)?))
        }).map_err(error_text)?;
        let mut response = account.changes(session, after, epoch.as_deref()).map_err(|_| "同步服务地址无效")?
            .send().await.map_err(|_| "同步连接失败")?;
        match response.status() {
            StatusCode::UNAUTHORIZED => { account.forget_access(session).await; return Ok(None); }
            StatusCode::GONE => {
                let tx = db.transaction().map_err(error_text)?;
                tx.execute_batch("DELETE FROM devices; DELETE FROM shards; UPDATE cursor SET epoch=NULL,seq=0;
                    UPDATE uploads SET acknowledged=0;").map_err(error_text)?;
                tx.commit().map_err(error_text)?;
                continue;
            }
            code if !code.is_success() => return Err("云端读取失败".into()),
            _ => (),
        }
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| "同步读取失败")? {
            if chunk.len() > MAX_RESPONSE - body.len() { return Err("云端同步回包过大".into()); }
            body.extend_from_slice(&chunk);
        }
        let delta: Delta = serde_json::from_slice(&body).map_err(error_text)?;
        let has_more = delta.has_more;
        let current_epoch = delta.epoch.clone();
        save_delta(db, delta, &session.device_id.to_string(), after, epoch.as_deref())?;
        if !has_more { return Ok(Some(current_epoch)); }
    }
    Ok(None)
}

fn windows_payload(start: i64, end: i64) -> Result<Payload, String> {
    let rows = dashboard_store(false)?.range(start, end).map_err(error_text)?;
    let mut device_intervals = Vec::new();
    let mut app_intervals = Vec::new();
    for row in rows {
        let start_ms = row.start_ms.max(start);
        let end_ms = row.end_ms.min(end);
        if start_ms >= end_ms { continue; }
        let (state, reason) = match row.state {
            0 => ("active", None), 1 => ("idle", None), 2 => ("offline", Some("locked")),
            3 => ("gap", Some("unknown")), _ => continue,
        };
        device_intervals.push(DeviceInterval { start_ms, end_ms, state: state.into(), reason: reason.map(str::to_owned) });
        if let Some(app_id) = row.app_id.filter(|_| row.state == 0) {
            let domain = row.page.filter(|name| row.page_kind == Some("site") && name.contains('.') &&
                name.len() <= 253 && name.split('.').all(|part| !part.is_empty() &&
                    part.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')));
            app_intervals.push(AppInterval { start_ms, end_ms, app_id, quality: "observed".into(),
                display_name: row.app_name.map(|name| name.chars().filter(|ch| !ch.is_control()).take(255).collect::<String>())
                    .filter(|name| !name.trim().is_empty()), domain });
        }
    }
    Ok(Payload { zone_id: "UTC".into(), capture_profile: "windows.foreground.v1".into(), device_intervals, app_intervals })
}

fn stage(db: &Connection, day: i64, since: i64, now: i64) -> Result<(), String> {
    let payload = windows_payload(day.max(since), (day + DAY_MS).min(now))?;
    let stored: Option<(String, i64)> = db.query_row("SELECT payload,revision FROM uploads WHERE day=?1", [day],
        |row| Ok((row.get(0)?, row.get(1)?))).optional().map_err(error_text)?;
    let empty = payload.device_intervals.is_empty() && payload.app_intervals.is_empty();
    if empty && stored.is_none() { return Ok(()); }
    let body = if empty { "null".into() } else { serde_json::to_string(&payload).map_err(error_text)? };
    if stored.as_ref().is_some_and(|(old, _)| old == &body) { return Ok(()); }
    let revision = stored.map_or(1, |(_, old)| old + 1);
    db.execute("INSERT OR REPLACE INTO uploads VALUES (?1,?2,?3,0)", params![day, revision, body]).map_err(error_text)?;
    Ok(())
}

async fn publish(account: &AccountService, session: &SyncSession, epoch: &str, db: &mut Connection) -> Result<(), String> {
    let now = now_ms();
    let today = now.div_euclid(DAY_MS) * DAY_MS;
    db.execute("INSERT OR IGNORE INTO writer VALUES (1,?1,?2)", params![now, today]).map_err(error_text)?;
    let (since, mut next): (i64, i64) = db.query_row("SELECT start_ms,next_day FROM writer", [],
        |row| Ok((row.get(0)?, row.get(1)?))).map_err(error_text)?;
    for _ in 0..4 {
        if next >= today { break; }
        stage(db, next, since, now)?;
        next += DAY_MS;
        db.execute("UPDATE writer SET next_day=?1", [next]).map_err(error_text)?;
    }
    if today - DAY_MS >= since.div_euclid(DAY_MS) * DAY_MS { stage(db, today - DAY_MS, since, now)?; }
    stage(db, today, since, now)?;
    let pending = {
        let mut query = db.prepare("SELECT day,revision,payload FROM uploads WHERE acknowledged=0 ORDER BY day LIMIT 1").map_err(error_text)?;
        query.query_row([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?, row.get::<_, String>(2)?)))
            .optional().map_err(error_text)?
    };
    let Some((day, revision, payload)) = pending else { return Ok(()); };
    let shard = Shard { day_utc: DateTime::from_timestamp_millis(day).ok_or("无效日期")?.date_naive().to_string(),
        revision, payload_version: 1, payload: serde_json::from_str(&payload).map_err(error_text)? };
    let result = account.upload(session, serde_json::json!({"epoch":epoch,"shards":[shard]}))
        .map_err(|_| "同步服务地址无效")?.send().await.map_err(|_| "同步上传失败")?;
    if result.status() == StatusCode::UNAUTHORIZED { account.forget_access(session).await; }
    if result.status().is_success() {
        db.execute("UPDATE uploads SET acknowledged=1 WHERE day=?1 AND revision=?2", params![day, revision]).map_err(error_text)?;
        Ok(())
    } else { Err(format!("云端拒绝上传 ({})", result.status().as_u16())) }
}

pub async fn run(app: tauri::AppHandle) {
    let mut delay = 5;
    loop {
        let managed = app.state::<Result<AccountService, AccountError>>();
        let Ok(account) = managed.inner() else { return; };
        match account.sync_session().await {
            Ok(Some(session)) => {
                let result = async {
                    let mut db = open(&session.cache)?;
                    let epoch = receive(account, &session, &mut db).await?;
                    if let Some(epoch) = epoch {
                        if account.cache().as_ref() == Some(&session.cache) {
                            publish(account, &session, &epoch, &mut db).await?;
                        }
                    }
                    Ok::<(), String>(())
                }.await;
                delay = if result.is_ok() { 5 } else { (delay * 2).min(60) };
            }
            Ok(None) | Err(_) => { delay = 5; }
        }
        tokio::time::sleep(Duration::from_secs(delay)).await;
    }
}
