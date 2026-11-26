use serde::{Deserialize, Serialize};
use tickmark_capture::store::Interval;

pub const LOCAL_ID: &str = "local-windows";
pub const MAX_SNAPSHOT_BYTES: u64 = 16 * 1024 * 1024;
pub const MAX_TIMESTAMP_MS: i64 = 8_640_000_000_000_000;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportData {
    pub devices: Vec<Device>,
    pub rejected_sources: usize,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Device {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub platform: String,
    #[serde(skip_deserializing)]
    pub local: bool,
    #[serde(skip_deserializing)]
    pub cloud: bool,
    pub revision: Option<u64>,
    pub updated_at_ms: Option<i64>,
    pub rows: Vec<Row>,
    pub app_rows: Option<Vec<Row>>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Row {
    pub start_ms: i64,
    pub end_ms: i64,
    pub state: i64,
    #[serde(default)]
    pub playback: bool,
    pub app_id: Option<String>,
    pub app_name: Option<String>,
    pub shared_app_id: Option<String>,
    pub page: Option<String>,
    pub page_kind: Option<PageKind>,
}

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PageKind {
    Site,
    Document,
}

impl PageKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Site => "site",
            Self::Document => "document",
        }
    }
}

impl From<Interval> for Row {
    fn from(row: Interval) -> Self {
        Self {
            start_ms: row.start_ms,
            end_ms: row.end_ms,
            state: row.state,
            playback: row.media_exempt,
            app_id: row.app_id,
            app_name: row.app_name,
            shared_app_id: None,
            page: row.page,
            page_kind: row.page_kind.map(|kind| if kind == "site" { PageKind::Site } else { PageKind::Document }),
        }
    }
}

pub fn state_key(state: i64) -> &'static str {
    match state {
        0 => "active",
        1 => "idle",
        2 => "offline",
        _ => "gap",
    }
}

impl Device {
    pub fn app_rows(&self) -> &[Row] {
        self.app_rows.as_deref().unwrap_or(&self.rows)
    }
}
