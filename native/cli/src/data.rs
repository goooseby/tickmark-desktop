use std::{
    path::{Path, PathBuf},
    time::SystemTime,
};

use serde_json::{Value, json};
use tickmark_capture::{data_dir, store::Store};
use tickmark_report::{Device, ReportData, read, select};

use crate::{cli::Global, errors::CliError, time::Zone};

pub fn resolve_dir(global: &Global) -> PathBuf {
    global.data_dir.clone().unwrap_or_else(data_dir)
}

pub fn open_store(dir: &Path) -> Result<Store, CliError> {
    let database = dir.join("capture.db");
    if !database.is_file() {
        return Err(CliError::unavailable(format!(
            "找不到数据库 {}。请先启动 Tickmark 桌面版，或用 --data-dir 指定数据目录。",
            database.display()
        )));
    }
    Store::existing(dir, false)
        .map_err(|error| CliError::unavailable(format!("无法只读打开数据库：{error}")))
}

pub fn cloud_path(global: &Global, dir: &Path) -> Result<Option<PathBuf>, CliError> {
    match global.cloud.as_deref() {
        None | Some("off") => Ok(None),
        Some("auto") => Ok(tickmark_report::cloud::latest_mirror(dir)),
        Some(path) => {
            let path = PathBuf::from(path);
            if !path.is_file() {
                return Err(CliError::unavailable(format!(
                    "云端镜像不存在：{}",
                    path.display()
                )));
            }
            Ok(Some(path))
        }
    }
}

pub fn load(global: &Global, from_ms: i64, to_ms: i64) -> Result<ReportData, CliError> {
    let dir = resolve_dir(global);
    let store = open_store(&dir)?;
    let mut data = read(&dir, &store, from_ms, to_ms)
        .map_err(|error| CliError::error(format!("读取本机数据失败：{error}")))?;
    drop(store);
    if let Some(path) = cloud_path(global, &dir)? {
        let sources = tickmark_report::cloud::reports(&path, from_ms, to_ms)
            .map_err(|error| CliError::unavailable(format!("无法读取云端镜像：{error}")))?;
        if sources.is_empty() {
            eprintln!(
                "tickmark: 云端镜像中没有可用设备数据（{}）。",
                path.display()
            );
        }
        data.include_cloud(sources);
    }
    Ok(data)
}

pub fn select_devices<'a>(
    data: &'a ReportData,
    selectors: &[String],
) -> Result<Vec<&'a Device>, CliError> {
    select(&data.devices, selectors).map_err(CliError::usage)
}

pub fn range_json(zone: &Zone, from_ms: i64, to_ms: i64) -> Value {
    json!({
        "from": zone.iso(from_ms),
        "to": zone.iso(to_ms),
        "tz": zone.name(),
    })
}

pub fn scope_json(global: &Global, devices: &[&Device]) -> Value {
    scope_ids(global, &devices.iter().map(|device| device.id.clone()).collect::<Vec<_>>())
}

pub fn scope_ids(global: &Global, devices: &[String]) -> Value {
    json!({
        "devices": devices,
        "cloud": global.cloud.as_deref().is_some_and(|value| value != "off"),
    })
}

pub fn generated_at(zone: &Zone) -> String {
    zone.iso(SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64)
}
