use std::io::{self, IsTerminal, Write};

use serde_json::Value;
use unicode_width::UnicodeWidthStr;

use crate::errors::CliError;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Table,
    Json,
    Csv,
    Ndjson,
}

impl Format {
    pub fn parse(raw: &str) -> Result<Self, CliError> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "table" => Ok(Self::Table),
            "json" => Ok(Self::Json),
            "csv" => Ok(Self::Csv),
            "ndjson" => Ok(Self::Ndjson),
            other => Err(CliError::usage(format!(
                "无法识别的输出格式「{other}」，可用 table、json、csv、ndjson。"
            ))),
        }
    }
}

/// Default follows the terminal: table for a TTY, JSON for pipes.
pub fn resolve_format(raw: Option<&str>) -> Result<Format, CliError> {
    match raw {
        Some(value) => Format::parse(value),
        None => Ok(if io::stdout().is_terminal() { Format::Table } else { Format::Json }),
    }
}

pub fn write_text(text: &str, out: Option<&str>) -> Result<(), CliError> {
    match out {
        None | Some("-") => {
            let mut stdout = io::stdout().lock();
            stdout.write_all(text.as_bytes())?;
            if !text.ends_with('\n') {
                stdout.write_all(b"\n")?;
            }
            stdout.flush()?;
            Ok(())
        }
        Some(path) => {
            std::fs::write(path, text)
                .map_err(|error| CliError::error(format!("无法写入 {path}：{error}")))
        }
    }
}

pub fn emit_json(value: &Value, out: Option<&str>) -> Result<(), CliError> {
    let mut text = serde_json::to_string_pretty(value)
        .map_err(|error| CliError::error(error.to_string()))?;
    text.push('\n');
    write_text(&text, out)
}

pub fn emit_ndjson(meta: &Value, rows: &[Value]) -> String {
    let mut output = String::new();
    if let Ok(line) = serde_json::to_string(meta) {
        output.push_str(&line);
        output.push('\n');
    }
    for row in rows {
        if let Ok(line) = serde_json::to_string(row) {
            output.push_str(&line);
            output.push('\n');
        }
    }
    output
}

fn display_width(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}

fn truncate(text: &str, cap: usize) -> String {
    if display_width(text) <= cap {
        return text.to_owned();
    }
    let mut result = String::new();
    let mut width = 0;
    for ch in text.chars() {
        let step = display_width(&ch.to_string());
        if width + step + 1 > cap {
            break;
        }
        width += step;
        result.push(ch);
    }
    result.push('…');
    result
}

pub fn table(headers: &[&str], numeric: &[bool], rows: &[Vec<String>], wide: bool) -> String {
    let cap = if wide { usize::MAX } else { 48 };
    let rows: Vec<Vec<String>> = rows
        .iter()
        .map(|row| row.iter().map(|cell| truncate(cell, cap)).collect())
        .collect();
    let mut widths: Vec<usize> = headers.iter().map(|header| display_width(header)).collect();
    for row in &rows {
        for (index, cell) in row.iter().enumerate() {
            if index < widths.len() {
                widths[index] = widths[index].max(display_width(cell));
            }
        }
    }
    let mut output = String::new();
    let line = |cells: Vec<String>, header: bool| {
        let mut text = String::new();
        for (index, cell) in cells.iter().enumerate() {
            if index > 0 {
                text.push_str("  ");
            }
            let padding = widths[index].saturating_sub(display_width(cell));
            if header || !numeric.get(index).copied().unwrap_or(false) {
                text.push_str(cell);
                text.push_str(&" ".repeat(padding));
            } else {
                text.push_str(&" ".repeat(padding));
                text.push_str(cell);
            }
        }
        text.trim_end().to_owned()
    };
    output.push_str(&line(headers.iter().map(|header| (*header).to_owned()).collect(), true));
    output.push('\n');
    for row in &rows {
        output.push_str(&line(row.clone(), false));
        output.push('\n');
    }
    output
}

fn csv_field(text: &str) -> String {
    if text.chars().any(|ch| matches!(ch, ',' | '"' | '\n' | '\r')) {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else {
        text.to_owned()
    }
}

pub fn csv(headers: &[&str], rows: &[Vec<String>], bom: bool) -> String {
    let mut output = String::new();
    if bom {
        output.push('\u{feff}');
    }
    output.push_str(
        &headers
            .iter()
            .map(|header| csv_field(header))
            .collect::<Vec<_>>()
            .join(","),
    );
    output.push('\n');
    for row in rows {
        output.push_str(&row.iter().map(|cell| csv_field(cell)).collect::<Vec<_>>().join(","));
        output.push('\n');
    }
    output
}

pub fn print_block(text: &str) {
    print!("{text}");
    if !text.ends_with('\n') {
        println!();
    }
}

pub fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KiB", "MiB", "GiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}
