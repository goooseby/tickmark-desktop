#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
fn main() {
    let result = if std::env::args_os().nth(1).is_some_and(|arg| arg == "--stop") {
        tickmark_capture::windows::stop().map_err(|error| error.into())
    } else {
        tickmark_capture::windows::run()
    };
    if let Err(error) = result {
        let dir = tickmark_capture::data_dir();
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(dir.join("capture-error.log"), error.to_string());
        std::process::exit(1);
    }
}

#[cfg(not(windows))]
fn main() {
    eprintln!("The collector runs in a Windows user session.");
}
