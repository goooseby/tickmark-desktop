#![cfg_attr(windows, windows_subsystem = "windows")]

use serde::Serialize;
use tauri::Manager;
use std::sync::{Mutex, OnceLock};
use tickmark_capture::{
    data_dir,
    preferences::DocumentApp,
    store::Store,
    windows::tray,
};

mod report_sources;
mod account;
mod account_vault;
mod sync;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DashboardSettings {
    apps: Vec<DocumentApp>,
    database_bytes: u64,
    edge_connected: bool,
}

fn dashboard_store(writable: bool) -> Result<Store, String> {
    static INITIALIZED: OnceLock<()> = OnceLock::new();
    static INITIALIZING: Mutex<()> = Mutex::new(());
    let dir = data_dir();
    if INITIALIZED.get().is_none() {
        let _initializing = INITIALIZING.lock().map_err(error_text)?;
        if INITIALIZED.get().is_none() {
            Store::open(&dir).map_err(error_text)?;
            let _ = INITIALIZED.set(());
        }
    }
    Store::existing(&dir, writable).map_err(error_text)
}

fn error_text(error: impl std::fmt::Display) -> String {
    error.to_string()
}

#[tauri::command]
async fn dashboard_intervals(start: i64, end: i64, account: tauri::State<'_, Result<account::AccountService, account::AccountError>>) -> Result<report_sources::ReportData, String> {
    if start >= end || end.saturating_sub(start) > 32 * 24 * 60 * 60 * 1000 {
        return Err("请选择有效的日期范围（最多一个月）。".into());
    }
    let cache = account.inner().as_ref().ok().and_then(account::AccountService::cache);
    tauri::async_runtime::spawn_blocking(move || {
        let store = dashboard_store(false)?;
        let mut data = report_sources::read(&data_dir(), &store, start, end).map_err(error_text)?;
        if let Some(path) = cache {
            data.include_cloud(sync::reports(&path, start, end).unwrap_or_default());
        }
        Ok(data)
    }).await.map_err(error_text)?
}

#[tauri::command]
async fn dashboard_settings() -> Result<DashboardSettings, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let dir = data_dir();
        let store = dashboard_store(false)?;
        let settings = store.preferences().map_err(error_text)?;
        let database_bytes = ["capture.db", "capture.db-wal", "capture.db-shm"].into_iter()
            .filter_map(|name| std::fs::metadata(dir.join(name)).ok()).map(|entry| entry.len()).sum();
        Ok(DashboardSettings {
            apps: store.document_apps(&settings).map_err(error_text)?,
            database_bytes,
            edge_connected: tickmark_capture::windows::browser::connected(),
        })
    }).await.map_err(error_text)?
}

#[tauri::command]
async fn import_device_report(content: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || report_sources::import(&data_dir(), &content))
        .await.map_err(error_text)?
}

#[tauri::command]
fn set_document_capture(app_id: String, enabled: bool) -> Result<(), String> {
    let store = dashboard_store(true)?;
    if !store.set_document_capture(&app_id, enabled).map_err(error_text)? {
        return Err("这个应用无法通过窗口标题记录文档。".into());
    }
    Ok(())
}

#[tauri::command]
fn collector_running() -> bool {
    tickmark_capture::windows::running()
}

#[tauri::command]
async fn edge_connected() -> Result<bool, String> {
    tauri::async_runtime::spawn_blocking(tickmark_capture::windows::browser::connected)
        .await.map_err(error_text)
}

#[tauri::command]
fn start_collector() -> Result<(), String> {
    tray::start_capture().map_err(error_text)
}

#[tauri::command]
fn stop_collector() -> Result<(), String> {
    tickmark_capture::windows::stop().map_err(error_text)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BrowserSetup {
    registered: bool,
    host_exists: bool,
    extension_dir: String,
}

fn extension_dir() -> std::path::PathBuf {
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../edge-extension");
    let dir = dir.canonicalize().unwrap_or(dir);
    let text = dir.to_string_lossy();
    match text.strip_prefix(r"\\?\UNC\") {
        Some(rest) => std::path::PathBuf::from(format!(r"\\{rest}")),
        None => std::path::PathBuf::from(text.strip_prefix(r"\\?\").unwrap_or(&text)),
    }
}

fn host_executable() -> std::path::PathBuf {
    std::env::current_exe().ok()
        .and_then(|path| path.parent().map(|dir| dir.join("tickmark-browser-host.exe")))
        .unwrap_or_else(|| std::path::PathBuf::from("tickmark-browser-host.exe"))
}

fn registered_manifest() -> Option<std::path::PathBuf> {
    let output = std::process::Command::new("reg")
        .args(["query", r"HKCU\Software\Microsoft\Edge\NativeMessagingHosts\com.tickmark.edge", "/ve"])
        .output().ok()?;
    if !output.status.success() { return None; }
    String::from_utf8_lossy(&output.stdout).lines().find_map(|line|
        line.find("REG_SZ").map(|index| std::path::PathBuf::from(line[index + 6..].trim())))
        .filter(|path| path.is_file())
}

fn host_from_manifest(path: &std::path::Path) -> Option<std::path::PathBuf> {
    let value: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
    value.get("path").and_then(|path| path.as_str()).map(std::path::PathBuf::from)
}

#[tauri::command]
fn browser_setup() -> BrowserSetup {
    let manifest = registered_manifest();
    let host_exists = manifest.as_ref().and_then(|path| host_from_manifest(path))
        .map(|path| path.is_file()).unwrap_or_else(|| host_executable().is_file());
    BrowserSetup {
        registered: manifest.is_some(),
        host_exists,
        extension_dir: extension_dir().display().to_string(),
    }
}

#[tauri::command]
fn register_browser_host() -> Result<(), String> {
    use base64::Engine;
    use sha2::{Digest, Sha256};
    let manifest_path = extension_dir().join("manifest.json");
    let manifest: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&manifest_path).map_err(|error| format!("读取扩展清单失败：{error}"))?
    ).map_err(error_text)?;
    let key = manifest.get("key").and_then(|key| key.as_str()).ok_or("扩展清单缺少 key。")?;
    let der = base64::engine::general_purpose::STANDARD.decode(key).map_err(error_text)?;
    let id: String = Sha256::digest(&der)[..16].iter()
        .flat_map(|byte| [b'a' + (byte >> 4), b'a' + (byte & 15)]).map(char::from).collect();
    let host = host_executable();
    if !host.is_file() { return Err("未找到 tickmark-browser-host.exe，请先构建工作区。".into()); }
    let target = host.with_file_name("com.tickmark.edge.json");
    let bridge = serde_json::json!({
        "name": "com.tickmark.edge",
        "description": "Tickmark Edge foreground page bridge",
        "path": host,
        "type": "stdio",
        "allowed_origins": [format!("chrome-extension://{id}/")]
    });
    std::fs::write(&target, serde_json::to_vec_pretty(&bridge).map_err(error_text)?).map_err(error_text)?;
    let status = std::process::Command::new("reg")
        .args(["add", r"HKCU\Software\Microsoft\Edge\NativeMessagingHosts\com.tickmark.edge", "/ve", "/t", "REG_SZ", "/d"])
        .arg(&target).arg("/f").status().map_err(error_text)?;
    if !status.success() { return Err("写入本机通信注册表失败。".into()); }
    Ok(())
}

#[tauri::command]
fn open_extension_folder() -> Result<(), String> {
    let dir = extension_dir();
    if !dir.is_dir() { return Err("未找到扩展目录，请重新构建应用。".into()); }
    std::process::Command::new("explorer.exe").arg(&dir).spawn().map(|_| ()).map_err(error_text)
}

#[tauri::command]
fn open_edge_extensions() -> Result<(), String> {
    std::process::Command::new("cmd")
        .args(["/C", "start", "", "msedge.exe", "edge://extensions"])
        .spawn().map(|_| ()).map_err(error_text)
}

fn main() {
    let _instance = match tray::claim_dashboard() {
        Ok(Some(instance)) => instance,
        Ok(None) => { tray::activate_existing_dashboard(); return; }
        Err(error) => { eprintln!("Could not claim Tickmark window: {error}"); return; }
    };
    tauri::Builder::default()
        .manage(account::AccountService::new(data_dir()))
        .setup(|app| {
            let main = app.get_webview_window("main").ok_or("missing dashboard window")?;
            tray::mark_dashboard(main.hwnd()?.0)?;
            if let Err(error) = tray::ensure_tray() {
                let dir = data_dir();
                let _ = std::fs::create_dir_all(&dir);
                let _ = std::fs::write(dir.join("tray-error.log"), error.to_string());
            }
            tauri::async_runtime::spawn(sync::run(app.handle().clone()));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![dashboard_intervals, dashboard_settings,
            set_document_capture, collector_running, edge_connected, start_collector, stop_collector, import_device_report,
            browser_setup, register_browser_host, open_extension_folder, open_edge_extensions,
            account::account_status, account::account_challenge, account::account_verify, account::account_logout])
        .run(tauri::generate_context!())
        .expect("unable to start Tickmark");
}
