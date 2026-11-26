use super::{tick, wide, window_identity};
use serde::{Deserialize, Serialize};
use std::{io::{self, Read, Write}, mem::size_of, path::Path, ptr::null_mut};
use windows_sys::{core::BOOL, Win32::{
    Foundation::{HWND, LPARAM, WPARAM},
    System::DataExchange::COPYDATASTRUCT,
    UI::WindowsAndMessaging::{
        EnumWindows, FindWindowW, GetClassNameW, GetForegroundWindow, GetWindowThreadProcessId,
        IsWindowVisible, SMTO_ABORTIFHUNG, SMTO_BLOCK, SendMessageTimeoutW, WM_APP, WM_COPYDATA,
    },
}};

pub(super) const QUERY_STATUS: u32 = WM_APP + 2;
const MESSAGE_ID: usize = 0x544B_4544;
const MAX_MESSAGE: usize = 512;
const LEASE_MS: i64 = 5_000;
pub(super) const ALIGN_MS: i64 = 500;

pub(super) struct Browser {
    last_contact: Option<i64>,
    page: Option<(HWND, String, i64)>,
    video: Option<(HWND, i64)>,
}

impl Browser {
    pub(super) fn new() -> Self {
        Self { last_contact: None, page: None, video: None }
    }

    pub(super) fn connected(&self) -> bool {
        self.last_contact.is_some_and(|last| tick() - last <= LEASE_MS)
    }

    pub(super) fn clear_page(&mut self) {
        self.page = None;
        self.video = None;
    }

    pub(super) fn page(&self, window: HWND) -> Option<&str> {
        self.page.as_ref().filter(|(owner, _, until)| *owner == window && tick() <= *until)
            .map(|(_, host, _)| host.as_str())
    }

    pub(super) fn video_playing(&self, window: HWND) -> bool {
        self.video.as_ref().is_some_and(|(owner, until)| *owner == window && tick() <= *until)
    }

    pub(super) fn accept(&mut self, window: HWND, report: Report, now: i64) {
        self.last_contact = Some(now);
        if !report.focused {
            return;
        }
        self.page = report.host.filter(|_| !window.is_null())
            .map(|host| (window, host, now + LEASE_MS));
        self.video = report.video_playing.then_some((window, now + LEASE_MS));
    }
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Report {
    focused: bool,
    host: Option<String>,
    #[serde(default)]
    video_playing: bool,
}

fn parse_report(bytes: &[u8]) -> Option<Report> {
    let report: Report = serde_json::from_slice(bytes).ok()?;
    if let Some(host) = &report.host
        && (!report.focused || !valid_host(host))
    {
        return None;
    }
    if report.video_playing && !report.focused {
        return None;
    }
    Some(report)
}

fn valid_host(host: &str) -> bool {
    host.len() <= 253 && host.split('.').all(|label| {
        (1..=63).contains(&label.len()) && !label.starts_with('-') && !label.ends_with('-')
            && label.bytes().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    })
}

pub(super) unsafe fn decode(data: LPARAM) -> Option<Report> {
    let message = unsafe { (data as *const COPYDATASTRUCT).as_ref()? };
    let size = message.cbData as usize;
    if message.dwData != MESSAGE_ID || size == 0 || size > MAX_MESSAGE || message.lpData.is_null() {
        return None;
    }
    parse_report(unsafe { std::slice::from_raw_parts(message.lpData.cast(), size) })
}

fn collector() -> HWND {
    unsafe {
        FindWindowW(wide(super::CAPTURE_CLASS).as_ptr(),
            super::event_name(&crate::data_dir()).as_ptr())
    }
}

pub fn connected() -> bool {
    request(QUERY_STATUS, 0, 0)
}

fn request(message: u32, w: WPARAM, l: LPARAM) -> bool {
    let collector = collector();
    if collector.is_null() {
        return false;
    }
    let mut result = 0;
    let sent = unsafe {
        SendMessageTimeoutW(collector, message, w, l, SMTO_ABORTIFHUNG | SMTO_BLOCK, 300, &mut result)
    };
    sent != 0 && result != 0
}

fn publish(mut report: Report) -> io::Result<bool> {
    let window = if report.focused { single_foreground_edge_window() } else { None };
    if window.is_none() {
        report.host = None;
        report.video_playing = false;
    }
    let mut bytes = serde_json::to_vec(&report)?;
    let data = COPYDATASTRUCT {
        dwData: MESSAGE_ID,
        cbData: bytes.len() as u32,
        lpData: bytes.as_mut_ptr().cast(),
    };
    Ok(request(WM_COPYDATA, window.unwrap_or(null_mut()) as usize, &data as *const _ as LPARAM))
}

fn edge_window(window: HWND) -> bool {
    if window.is_null() || unsafe { IsWindowVisible(window) } == 0 {
        return false;
    }
    let mut class = [0; 64];
    let len = unsafe { GetClassNameW(window, class.as_mut_ptr(), class.len() as i32) };
    if len <= 0 || !class[..len as usize].iter().copied().eq("Chrome_WidgetWin_1".encode_utf16()) {
        return false;
    }
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(window, &mut pid) };
    window_identity::image(pid)
        .and_then(|path| Path::new(&path).file_name().map(|file| file.to_owned()))
        .is_some_and(|file| file.eq_ignore_ascii_case("msedge.exe"))
}

fn single_foreground_edge_window() -> Option<HWND> {
    unsafe extern "system" fn count(window: HWND, state: LPARAM) -> BOOL {
        let (foreground, ambiguous) = unsafe { &mut *(state as *mut (HWND, bool)) };
        if window != *foreground && edge_window(window) {
            *ambiguous = true;
            return 0;
        }
        1
    }

    let foreground = unsafe { GetForegroundWindow() };
    if !edge_window(foreground) {
        return None;
    }
    let mut state = (foreground, false);
    let enumerated = unsafe { EnumWindows(Some(count), &mut state as *mut _ as LPARAM) };
    (enumerated != 0 && !state.1 && unsafe { GetForegroundWindow() } == foreground)
        .then_some(foreground)
}

pub fn run_host() -> io::Result<()> {
    std::thread::spawn(|| loop {
        std::thread::sleep(std::time::Duration::from_secs(2));
        let _ = publish(Report { focused: false, host: None, video_playing: false });
    });
    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    let mut length = [0; size_of::<u32>()];
    loop {
        match input.read_exact(&mut length) {
            Ok(()) => (),
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(()),
            Err(error) => return Err(error),
        }
        let size = u32::from_ne_bytes(length) as usize;
        if size == 0 || size > MAX_MESSAGE {
            return Err(io::ErrorKind::InvalidData.into());
        }
        let mut bytes = [0; MAX_MESSAGE];
        input.read_exact(&mut bytes[..size])?;
        let connected = if let Some(report) = parse_report(&bytes[..size]) {
            publish(report)?
        } else {
            false
        };
        let response: &[u8] = if connected { br#"{"connected":true}"# } else { br#"{"connected":false}"# };
        output.write_all(&(response.len() as u32).to_ne_bytes())?;
        output.write_all(response)?;
        output.flush()?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bridge_accepts_only_plain_domains_from_focused_reports() {
        for host in ["example.org", "xn--fiqs8s.example", "localhost"] {
            assert!(valid_host(host));
        }
        for host in ["https://example.org", "example.org/private", "login@example.org", "example.org?x", "a..b", ""] {
            assert!(!valid_host(host));
        }
        assert!(parse_report(br#"{"focused":false,"host":"example.org"}"#).is_none());
        assert!(parse_report(br#"{"focused":true,"host":null}"#).is_some());
        assert!(parse_report(br#"{"focused":true,"host":"example.org","videoPlaying":true}"#)
            .is_some_and(|report| report.video_playing));
        assert!(parse_report(br#"{"focused":false,"host":null,"videoPlaying":true}"#).is_none());
    }
}
