pub mod engine;
pub mod preferences;
pub mod store;

#[cfg(windows)]
pub mod windows;

pub fn data_dir() -> std::path::PathBuf {
    if let Some(dir) = std::env::var_os("TICKMARK_DATA_DIR") {
        return dir.into();
    }
    std::env::var_os("LOCALAPPDATA")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("TickmarkDesktop")
}
