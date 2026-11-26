use serde::{Deserialize, Serialize};
use std::{collections::{BTreeMap, HashMap}, fs, io::{self, Read, Write}, path::Path, sync::Mutex};
use tickmark_capture::store::{Interval, Store};

const LOCAL_ID: &str = "local-windows";
const MAX_SNAPSHOT_BYTES: u64 = 16 * 1024 * 1024;
const MAX_TIMESTAMP_MS: i64 = 8_640_000_000_000_000;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportData {
    devices: Vec<Device>,
    rejected_sources: usize,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Device {
    id: String,
    name: String,
    kind: String,
    platform: String,
    #[serde(skip_deserializing)]
    local: bool,
    #[serde(skip_deserializing)]
    cloud: bool,
    revision: Option<u64>,
    updated_at_ms: Option<i64>,
    rows: Vec<Row>,
    app_rows: Option<Vec<Row>>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Row {
    start_ms: i64,
    end_ms: i64,
    state: i64,
    #[serde(default)]
    playback: bool,
    app_id: Option<String>,
    app_name: Option<String>,
    shared_app_id: Option<String>,
    page: Option<String>,
    page_kind: Option<PageKind>,
}

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
enum PageKind { Site, Document }

impl From<Interval> for Row {
    fn from(row: Interval) -> Self {
        Self {
            start_ms: row.start_ms, end_ms: row.end_ms, state: row.state, playback: row.media_exempt,
            app_id: row.app_id, app_name: row.app_name, shared_app_id: None, page: row.page,
            page_kind: row.page_kind.map(|kind| if kind == "site" { PageKind::Site } else { PageKind::Document }),
        }
    }
}

pub fn read(dir: &Path, store: &Store, start: i64, end: i64) -> Result<ReportData, Box<dyn std::error::Error>> {
    let local = Device {
        id: LOCAL_ID.into(), name: "本机电脑".into(), kind: "computer".into(), platform: "Windows".into(),
        local: true, cloud: false, revision: None, updated_at_ms: None,
        rows: store.range(start, end)?.into_iter().map(Row::from).collect(), app_rows: None,
    };
    let mut devices = vec![local];
    let mut rejected_sources = 0;
    let entries = match fs::read_dir(dir.join("report-devices")) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(ReportData { devices, rejected_sources }),
        Err(error) => return Err(error.into()),
    };
    let mut imported = BTreeMap::new();
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") || !entry.file_type()?.is_file() {
            continue;
        }
        match snapshot(&path, start, end) {
            Some(device) => { imported.insert(device.id.clone(), device); }
            None => rejected_sources += 1,
        }
    }
    devices.extend(imported.into_values());
    Ok(ReportData { devices, rejected_sources })
}

impl ReportData {
    pub fn include_cloud(&mut self, sources: Vec<(crate::sync::CloudDevice, Vec<crate::sync::Payload>)>) {
        for (device, shards) in sources {
            let mut rows = Vec::new();
            let mut app_rows = Vec::new();
            for shard in shards {
                for item in shard.device_intervals {
                    let state = match item.state.as_str() {
                        "active" => 0, "idle" => 1, "offline" => 2, _ => 3,
                    };
                    rows.push(Row {
                        start_ms: item.start_ms, end_ms: item.end_ms, state, playback: false,
                        app_id: None, app_name: None, shared_app_id: None, page: None, page_kind: None,
                    });
                }
                for item in shard.app_intervals {
                    app_rows.push(Row {
                        start_ms: item.start_ms, end_ms: item.end_ms, state: 0, playback: false,
                        app_name: Some(item.display_name.unwrap_or_else(|| item.app_id.clone())),
                        app_id: Some(item.app_id), shared_app_id: None,
                        page_kind: item.domain.as_ref().map(|_| PageKind::Site), page: item.domain,
                    });
                }
            }
            rows.sort_by_key(|row| row.start_ms);
            app_rows.sort_by_key(|row| row.start_ms);
            self.devices.retain(|prior| prior.local || prior.id != device.device_id);
            self.devices.push(Device {
                id: device.device_id, name: device.name, kind: device.kind, platform: device.platform,
                local: false, cloud: true, revision: None, updated_at_ms: Some(device.last_seen_at),
                rows, app_rows: Some(app_rows),
            });
        }
    }
}

fn snapshot(path: &Path, start: i64, end: i64) -> Option<Device> {
    let mut source = fs::File::open(path).ok()?.take(MAX_SNAPSHOT_BYTES + 1);
    let mut bytes = Vec::new();
    source.read_to_end(&mut bytes).ok()?;
    if bytes.len() as u64 > MAX_SNAPSHOT_BYTES { return None; }
    let mut device: Device = serde_json::from_slice(&bytes).ok()?;
    if !valid_id(&device.id) || device.id == LOCAL_ID || path.file_stem()?.to_str()? != device.id ||
        [&device.name, &device.kind, &device.platform].into_iter().any(|text| text.trim().is_empty() || text.chars().count() > 96 || text.chars().any(char::is_control)) {
        return None;
    }
    validate(&mut device).ok()?;
    device.rows.retain(|row| row.start_ms < end && row.end_ms > start);
    if let Some(apps) = &mut device.app_rows {
        apps.retain(|row| row.start_ms < end && row.end_ms > start);
    }
    Some(device)
}

fn validate(device: &mut Device) -> Result<(), String> {
    if !valid_id(&device.id) || device.id == LOCAL_ID ||
        [&device.name, &device.kind, &device.platform].into_iter().any(|text|
            text.trim().is_empty() || text.chars().count() > 96 || text.chars().any(char::is_control)) ||
        device.updated_at_ms.is_some_and(|stamp| !(0..=MAX_TIMESTAMP_MS).contains(&stamp)) ||
        device.revision.is_some_and(|revision| revision > (1u64 << 53) - 1) {
        return Err("设备资料或修订号无效。".into());
    }
    let valid_row = |row: &Row| row.start_ms >= 0 && row.end_ms <= MAX_TIMESTAMP_MS
        && row.end_ms > row.start_ms && (0..=3).contains(&row.state);
    device.rows.sort_unstable_by_key(|row| row.start_ms);
    if !device.rows.iter().all(valid_row)
        || device.rows.windows(2).any(|pair| pair[0].end_ms > pair[1].start_ms) {
        return Err("设备区间无效或相互重叠。".into());
    }
    if let Some(apps) = &mut device.app_rows {
        apps.sort_unstable_by_key(|row| row.start_ms);
        let mut last_by_app = HashMap::new();
        for row in apps {
            let Some(id) = row.app_id.as_deref().filter(|id| !id.trim().is_empty()) else {
                return Err("应用会话缺少应用标识。".into());
            };
            if !valid_row(row) || row.state != 0 || last_by_app.insert(id, row.end_ms).is_some_and(|end| end > row.start_ms) {
                return Err("同一应用的会话无效或相互重叠。".into());
            }
        }
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Transfer {
    #[serde(rename = "type")]
    format: String,
    version: u32,
    from_ms: i64,
    until_ms: i64,
    device: Device,
}

pub fn import(dir: &Path, content: &str) -> Result<String, String> {
    static WRITER: Mutex<()> = Mutex::new(());
    let _writer = WRITER.lock().map_err(|error| error.to_string())?;
    if content.len() as u64 > MAX_SNAPSHOT_BYTES {
        return Err("设备报告超过 16 MiB，请缩小导出的日期范围。".into());
    }
    let mut transfer: Transfer = serde_json::from_str(content.trim_start_matches('\u{feff}'))
        .map_err(|_| "请选择 Tickmark 设备区间报告（JSON）。".to_owned())?;
    if transfer.format != "tickmark.device-intervals" || transfer.version != 1 {
        return Err("该报告的类型或版本不受支持。".into());
    }
    let device = &mut transfer.device;
    validate(device)?;
    let revision = device.revision.ok_or("设备报告缺少修订号。")?;
    let from = transfer.from_ms;
    let until = transfer.until_ms;
    let in_range = |row: &Row| row.start_ms >= from && row.end_ms <= until;
    if from < 0 || until > MAX_TIMESTAMP_MS || until <= from
        || !device.rows.iter().chain(device.app_rows.iter().flatten()).all(in_range) {
        return Err("报告区间超出了声明的时间范围。".into());
    }
    let output = dir.join("report-devices");
    fs::create_dir_all(&output).map_err(io_message)?;
    let target = output.join(format!("{}.json", device.id));
    if target.exists() {
        let previous = snapshot(&target, i64::MIN, i64::MAX).ok_or("已有设备镜像损坏，请检查数据目录。")?;
        if previous.revision.is_some_and(|last| revision <= last) {
            return Err("该设备已有相同或更新的报告，无需重复导入。".into());
        }
        if previous.app_rows.is_some() || device.app_rows.is_some() {
            let old_apps = previous.app_rows.unwrap_or_else(|| app_intervals(&previous.rows));
            let new_apps = device.app_rows.get_or_insert_with(|| app_intervals(&device.rows));
            keep_outside(new_apps, old_apps, from, until);
        }
        keep_outside(&mut device.rows, previous.rows, from, until);
        validate(device)?;
    }
    let bytes = serde_json::to_vec(device).map_err(|error| error.to_string())?;
    if bytes.len() as u64 > MAX_SNAPSHOT_BYTES {
        return Err("合并后设备历史超过本地镜像容量。".into());
    }
    let temp = target.with_extension("importing");
    let saved = (|| -> io::Result<()> {
        let mut file = fs::File::create(&temp)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temp, &target)?;
        Ok(())
    })();
    if let Err(error) = saved {
        let _ = fs::remove_file(temp);
        return Err(io_message(error));
    }
    Ok(device.name.clone())
}

fn app_intervals(rows: &[Row]) -> Vec<Row> {
    rows.iter().filter(|row| row.state == 0 && row.app_id.is_some()).cloned().collect()
}

fn keep_outside(destination: &mut Vec<Row>, previous: Vec<Row>, from: i64, until: i64) {
    for row in previous {
        if row.start_ms < from {
            destination.push(Row { end_ms: row.end_ms.min(from), ..row.clone() });
        }
        if row.end_ms > until {
            destination.push(Row { start_ms: row.start_ms.max(until), ..row });
        }
    }
}

fn io_message(error: io::Error) -> String {
    format!("无法更新设备镜像（{error}）")
}

fn valid_id(id: &str) -> bool {
    (1..=80).contains(&id.len()) &&
        id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn import_replaces_only_its_range_and_preserves_split_screen_app_times() {
        let dir = std::env::temp_dir().join(format!("tickmark-import-test-{}-{}", std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        let file = |revision, from, until| json!({
            "type": "tickmark.device-intervals", "version": 1, "fromMs": from, "untilMs": until,
            "device": {"id": "android-a", "name": "Android 平板", "kind": "tablet", "platform": "Android",
                "revision": revision,
                "rows": [{"startMs": from, "endMs": until, "state": 0}],
                "appRows": [{"startMs": from, "endMs": until, "state": 0, "appId": "reader"},
                            {"startMs": from, "endMs": until, "state": 0, "appId": "notes"}]}
        }).to_string();
        assert!(import(&dir, &file(2, 0, 20)).is_ok());
        assert!(import(&dir, &file(3, 5, 10)).is_ok());
        assert!(import(&dir, &file(2, 0, 30)).is_err());
        let path = dir.join("report-devices/android-a.json");
        let saved = snapshot(&path, 0, 21).unwrap();
        assert_eq!(saved.rows.into_iter().map(|row| (row.start_ms, row.end_ms)).collect::<Vec<_>>(),
            [(0, 5), (5, 10), (10, 20)]);
        assert_eq!(saved.app_rows.unwrap().len(), 6);
        fs::remove_file(&path).unwrap();
        fs::remove_dir(path.parent().unwrap()).unwrap();
        fs::remove_dir(dir).unwrap();
    }

    #[test]
    fn reads_three_devices_and_rejects_conflicting_intervals_without_losing_local_data() {
        let dir = std::env::temp_dir().join(format!("tickmark-source-test-{}-{}", std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        let store = Store::open(&dir).unwrap();
        let snapshots = dir.join("report-devices");
        fs::create_dir_all(&snapshots).unwrap();
        for (id, kind, rows) in [
            ("phone", "phone", json!([{"startMs": 5, "endMs": 8, "state": 0, "appId": "reader"}])),
            ("tablet", "tablet", json!([{"startMs": 20, "endMs": 30, "state": 1}, {"startMs": 3, "endMs": 7, "state": 2}])),
            ("bad", "other", json!([{"startMs": 4, "endMs": 6, "state": 0}, {"startMs": 5, "endMs": 9, "state": 1}])),
        ] {
            fs::write(snapshots.join(format!("{id}.json")), json!({
                "id": id, "name": "Test device", "kind": kind, "platform": "Test",
                "local": true, "rows": rows
            }).to_string()).unwrap();
        }
        let result = read(&dir, &store, 0, 10).unwrap();
        let ids: Vec<_> = result.devices.iter().map(|device| device.id.as_str()).collect();
        assert_eq!(ids, [LOCAL_ID, "phone", "tablet"]);
        assert!(result.devices[0].local);
        assert!(!result.devices[1].local);
        assert_eq!(result.devices[2].rows.len(), 1);
        assert_eq!(result.devices[2].rows[0].state, 2);
        assert_eq!(result.rejected_sources, 1);
        drop(store);
        for entry in fs::read_dir(&snapshots).unwrap() { fs::remove_file(entry.unwrap().path()).unwrap(); }
        fs::remove_dir(snapshots).unwrap();
        for entry in fs::read_dir(&dir).unwrap() { fs::remove_file(entry.unwrap().path()).unwrap(); }
        fs::remove_dir(dir).unwrap();
    }
}
