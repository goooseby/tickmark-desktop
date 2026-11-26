use std::{path::Path, time::Duration};

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

const DAY_MS: i64 = 86_400_000;

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudDevice {
    pub device_id: String,
    pub name: String,
    pub platform: String,
    pub kind: String,
    pub last_seen_at: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudDeviceSummary {
    #[serde(flatten)]
    pub device: CloudDevice,
    pub first_day_ms: Option<i64>,
    pub last_day_ms: Option<i64>,
    pub shards: i64,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Payload {
    pub zone_id: String,
    pub capture_profile: String,
    pub device_intervals: Vec<DeviceInterval>,
    pub app_intervals: Vec<AppInterval>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceInterval {
    pub start_ms: i64,
    pub end_ms: i64,
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInterval {
    pub start_ms: i64,
    pub end_ms: i64,
    pub app_id: String,
    pub quality: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
}

/// Read the read-only cloud mirror written by the desktop app. Never refreshes.
pub fn reports(path: &Path, start: i64, end: i64) -> Result<Vec<(CloudDevice, Vec<Payload>)>, String> {
    if !path.is_file() {
        return Ok(Vec::new());
    }
    let db = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|error| error.to_string())?;
    db.busy_timeout(Duration::from_secs(3)).map_err(|error| error.to_string())?;
    let tx = db.unchecked_transaction().map_err(|error| error.to_string())?;
    let mut query = tx
        .prepare("SELECT id,name,platform,kind,seen FROM devices ORDER BY id")
        .map_err(|error| error.to_string())?;
    let devices = query
        .query_map([], |row| {
            Ok(CloudDevice {
                device_id: row.get(0)?,
                name: row.get(1)?,
                platform: row.get(2)?,
                kind: row.get(3)?,
                last_seen_at: row.get(4)?,
            })
        })
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    let mut shards = tx
        .prepare("SELECT payload FROM shards WHERE device=? AND day>=? AND day<? AND payload IS NOT NULL ORDER BY day")
        .map_err(|error| error.to_string())?;
    devices
        .into_iter()
        .map(|device| {
            let rows = shards
                .query_map(params![device.device_id, start.div_euclid(DAY_MS) * DAY_MS, end], |row| {
                    row.get::<_, String>(0)
                })
                .map_err(|error| error.to_string())?;
            let payloads = rows
                .map(|row| {
                    let text = row.map_err(|error| error.to_string())?;
                    serde_json::from_str(&text).map_err(|error| error.to_string())
                })
                .collect::<Result<Vec<Payload>, String>>()?;
            Ok((device, payloads))
        })
        .collect()
}

/// Pick the most recently written mirror under `accounts-v1`.
pub fn latest_mirror(root: &Path) -> Option<std::path::PathBuf> {
    let mut best: Option<(std::time::SystemTime, std::path::PathBuf)> = None;
    let accounts = root.join("accounts-v1");
    for vault in std::fs::read_dir(accounts).ok()?.flatten() {
        let Ok(files) = std::fs::read_dir(vault.path()) else { continue };
        for file in files.flatten() {
            let path = file.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("sqlite3") {
                continue;
            }
            let Ok(modified) = file.metadata().and_then(|meta| meta.modified()) else { continue };
            if best.as_ref().is_none_or(|(when, _)| modified > *when) {
                best = Some((modified, path));
            }
        }
    }
    best.map(|(_, path)| path)
}

/// Device list with shard coverage, without parsing every payload.
pub fn device_summaries(path: &Path) -> Result<Vec<CloudDeviceSummary>, String> {
    if !path.is_file() {
        return Ok(Vec::new());
    }
    let db = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|error| error.to_string())?;
    db.busy_timeout(Duration::from_secs(3)).map_err(|error| error.to_string())?;
    let mut query = db
        .prepare(
            "SELECT d.id, d.name, d.platform, d.kind, d.seen,
                    MIN(s.day), MAX(s.day), COUNT(s.day)
             FROM devices d
             LEFT JOIN shards s ON s.device = d.id AND s.payload IS NOT NULL
             GROUP BY d.id ORDER BY d.id",
        )
        .map_err(|error| error.to_string())?;
    query
        .query_map([], |row| {
            Ok(CloudDeviceSummary {
                device: CloudDevice {
                    device_id: row.get(0)?,
                    name: row.get(1)?,
                    platform: row.get(2)?,
                    kind: row.get(3)?,
                    last_seen_at: row.get(4)?,
                },
                first_day_ms: row.get(5)?,
                last_day_ms: row.get(6)?,
                shards: row.get(7)?,
            })
        })
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())
}
