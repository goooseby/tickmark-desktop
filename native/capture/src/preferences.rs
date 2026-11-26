use serde::Serialize;
use std::collections::HashMap;

#[derive(Default)]
pub struct Preferences {
    pub documents: HashMap<String, bool>,
}

#[derive(Serialize)]
pub struct DocumentApp {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub eligible: bool,
}

pub(crate) fn default_documents(name: &str) -> bool {
    let name = name.to_lowercase();
    ["word", "excel", "powerpoint", "visual studio code", "adobe acrobat", "sumatrapdf"]
        .iter().any(|item| name == *item || name.contains(&format!(" {item}")) || name.starts_with(&format!("{item} ")))
}

pub(crate) fn browser_name(name: &str) -> bool {
    let name = name.to_lowercase();
    ["chrome", "edge", "firefox", "brave", "opera", "vivaldi", "chromium"]
        .iter().any(|browser| name.split_whitespace().any(|word| word == *browser))
}