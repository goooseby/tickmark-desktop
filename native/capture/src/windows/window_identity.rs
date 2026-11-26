use crate::engine::Target;
use sha2::{Digest, Sha256};
use std::{cell::RefCell, collections::HashMap, os::windows::ffi::OsStringExt, path::Path, ptr::null_mut};
use windows_sys::Win32::{
    Foundation::{CloseHandle, HWND},
    Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW},
    System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW},
    UI::WindowsAndMessaging::{GetClassNameW, GetWindowTextW, GetWindowThreadProcessId},
};

thread_local! { static PRODUCTS: RefCell<HashMap<String, Option<String>>> = RefCell::new(HashMap::new()); }

pub(super) fn identify(window: HWND, overrides: &HashMap<String, bool>) -> (Option<Target>, u32, bool) {
    if window.is_null() {
        return (None, 0, false);
    }
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(window, &mut pid) };
    let mut class = [0u16; 128];
    let length = unsafe { GetClassNameW(window, class.as_mut_ptr(), class.len() as i32) };
    let class = String::from_utf16_lossy(&class[..length.max(0) as usize]);
    let transient = matches!(class.as_str(), "MultitaskingViewFrame" | "TaskSwitcherWnd" | "ForegroundStaging");
    let path = image(pid);
    let file = path.as_deref().and_then(|path| Path::new(path).file_name()).map(|file| file.to_string_lossy());
    let executable = file.as_ref().map(|file| file.to_ascii_lowercase());
    let shell = matches!(executable.as_deref(), Some("searchhost.exe" | "searchapp.exe" | "startmenuexperiencehost.exe" | "shellexperiencehost.exe" | "textinputhost.exe"));
    if transient || shell || matches!(class.as_str(), "Shell_TrayWnd" | "Shell_SecondaryTrayWnd" | "Progman" | "WorkerW") {
        return (Some(Target { id: "windows:desktop".into(), name: "系统桌面操作".into(), page: None, transient: transient || shell }), pid, false);
    }
    let (Some(path), Some(file), Some(executable)) = (path.as_deref(), file, executable) else { return (None, 0, false) };
    let (label, suffixes): (Option<&str>, &[&str]) = match executable.as_str() {
        "winword.exe" => (Some("Microsoft Word"), &[" - Word", " - Microsoft Word"]),
        "excel.exe" => (Some("Microsoft Excel"), &[" - Excel", " - Microsoft Excel"]),
        "powerpnt.exe" => (Some("Microsoft PowerPoint"), &[" - PowerPoint", " - Microsoft PowerPoint"]),
        "code.exe" => (Some("Visual Studio Code"), &[" - Visual Studio Code"]),
        "acrord32.exe" | "acrobat.exe" => (Some("Adobe Acrobat"), &[" - Adobe Acrobat", " - Adobe Acrobat Reader"]),
        "sumatrapdf.exe" => (Some("SumatraPDF"), &[" - SumatraPDF"]),
        "chrome.exe" => (Some("Google Chrome"), &[]),
        "msedge.exe" => (Some("Microsoft Edge"), &[]),
        _ => (None, &[]),
    };
    let label = if executable == "msedge.exe" { label.map(str::to_owned) } else { product_name(path).or_else(|| label.map(str::to_owned)) }
        .unwrap_or_else(|| Path::new(file.as_ref()).file_stem().map(|name| name.to_string_lossy().into_owned()).unwrap_or_else(|| file.into_owned()));
    let id = format!("windows:{:X}", Sha256::digest(path.to_uppercase().as_bytes()));
    let browser = crate::preferences::browser_name(&label) ||
        crate::preferences::browser_name(executable.trim_end_matches(".exe"));
    let explicit = overrides.get(&id).copied();
    let page = if browser || explicit == Some(false) || explicit.is_none() && suffixes.is_empty() { None }
        else { title(window, suffixes, &label, explicit == Some(true)) };
    let presentation = executable == "powerpnt.exe" && class.eq_ignore_ascii_case("screenClass");
    (Some(Target { id, name: label, page, transient: false }), pid, presentation)
}

pub(super) fn image(pid: u32) -> Option<String> {
    if pid == 0 { return None; }
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if process == null_mut() { return None; }
    let mut path = [0u16; 1024];
    let mut length = path.len() as u32;
    let obtained = unsafe { QueryFullProcessImageNameW(process, 0, path.as_mut_ptr(), &mut length) };
    unsafe { CloseHandle(process) };
    (obtained != 0 && length > 0).then(|| std::ffi::OsString::from_wide(&path[..length as usize]).to_string_lossy().into_owned())
}

fn title(window: HWND, suffixes: &[&str], label: &str, explicit: bool) -> Option<String> {
    let mut title = [0u16; 1024];
    let length = unsafe { GetWindowTextW(window, title.as_mut_ptr(), title.len() as i32) };
    if length <= 0 || length as usize >= title.len() - 1 { return None; }
    let title = String::from_utf16_lossy(&title[..length as usize]);
    let fallback = format!(" - {label}");
    let clean = if let Some(suffix) = suffixes.iter().find(|suffix| title.ends_with(**suffix)) {
        title.strip_suffix(suffix)?
    } else if explicit {
        title.strip_suffix(&fallback).unwrap_or(&title)
    } else { return None };
    let clean = clean.trim().trim_start_matches(['*', '●', ' ']).trim();
    if clean.contains("://") || clean.chars().any(char::is_control) { return None; }
    let clean = clean.rsplit(['/', '\\']).next()?.trim();
    if clean.is_empty() || clean == label || clean.chars().count() > 160 || clean.contains([':', '@'])
        || matches!(clean, "Welcome" | "Home" | "Visual Studio Code")
    { return None; }
    Some(clean.to_owned())
}


fn product_name(path: &str) -> Option<String> {
    PRODUCTS.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some(label) = cache.get(path) { return label.clone(); }
        if cache.len() >= 256 { cache.clear(); }
        let label = read_product_name(path);
        cache.insert(path.to_owned(), label.clone());
        label
    })
}

fn read_product_name(path: &str) -> Option<String> {
    let target = super::wide(path);
    let len = unsafe { GetFileVersionInfoSizeW(target.as_ptr(), null_mut()) };
    if len == 0 || len > 1_048_576 { return None; }
    let mut data = vec![0u32; (len as usize).div_ceil(4)];
    if unsafe { GetFileVersionInfoW(target.as_ptr(), 0, len, data.as_mut_ptr().cast()) } == 0 { return None; }
    let lookup = |key: &str| {
        let path = super::wide(key);
        let mut address = null_mut();
        let mut length = 0;
        let found = unsafe { VerQueryValueW(data.as_ptr().cast(), path.as_ptr(), &mut address, &mut length) };
        (found != 0 && !address.is_null() && length > 0).then_some((address.cast::<u16>(), length as usize))
    };
    let (lang, code) = lookup(r"\VarFileInfo\Translation").filter(|(_, count)| *count >= 4)
        .map(|(values, _)| unsafe { (*values, *values.add(1)) }).unwrap_or((0x0409, 0x04B0));
    for property in ["FileDescription", "ProductName"] {
        let key = format!(r"\StringFileInfo\{lang:04x}{code:04x}\{property}");
        if let Some((value, count)) = lookup(&key).filter(|(_, count)| *count <= 256) {
            let units = unsafe { std::slice::from_raw_parts(value, count) };
            let text = String::from_utf16_lossy(units).trim_matches(char::from(0)).trim().to_owned();
            if !text.is_empty() && !text.chars().any(char::is_control) {
                return Some(text);
            }
        }
    }
    None
}
