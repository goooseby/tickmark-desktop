use crate::{engine::Slice, preferences::{self, DocumentApp, Preferences}};
use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior, params};
use serde::Serialize;
use std::{path::Path, time::Duration};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Interval {
    pub start_ms: i64,
    pub end_ms: i64,
    pub state: i64,
    #[serde(rename = "playback")]
    pub media_exempt: bool,
    pub app_id: Option<String>,
    pub app_name: Option<String>,
    pub page: Option<String>,
    pub page_kind: Option<&'static str>,
}

pub struct Store(Connection);

impl Store {
    pub fn open(dir: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        std::fs::create_dir_all(dir)?;
        let mut db = Connection::open(dir.join("capture.db"))?;
        db.busy_timeout(Duration::from_secs(3))?;
        db.execute_batch(
            "PRAGMA journal_mode = WAL;
             CREATE TABLE IF NOT EXISTS capture_spans (
                start_ms INTEGER PRIMARY KEY,
                end_ms INTEGER NOT NULL CHECK (end_ms > start_ms),
                state INTEGER NOT NULL, app_id TEXT, app_name TEXT, page TEXT);
             CREATE INDEX IF NOT EXISTS capture_span_ends ON capture_spans(end_ms);
             CREATE TABLE IF NOT EXISTS document_capture (
                app_id TEXT PRIMARY KEY, enabled INTEGER NOT NULL CHECK(enabled IN (0,1)));",
        )?;
        if !has_playback_column(&db)? {
            let schema = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            if !has_playback_column(&schema)? {
                schema.execute("ALTER TABLE capture_spans ADD COLUMN playback INTEGER NOT NULL DEFAULT 0", [])?;
            }
            schema.commit()?;
        }
        Ok(Self(db))
    }

    pub fn existing(dir: &Path, writable: bool) -> rusqlite::Result<Self> {
        let flags = if writable { OpenFlags::SQLITE_OPEN_READ_WRITE } else { OpenFlags::SQLITE_OPEN_READ_ONLY };
        let db = Connection::open_with_flags(dir.join("capture.db"), flags)?;
        db.busy_timeout(Duration::from_secs(3))?;
        Ok(Self(db))
    }


    pub fn preferences(&self) -> rusqlite::Result<Preferences> {
        let mut statement = self.0.prepare("SELECT app_id, enabled FROM document_capture")?;
        let documents = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(Preferences { documents })
    }

    pub fn document_apps(&self, settings: &Preferences) -> rusqlite::Result<Vec<DocumentApp>> {
        let mut statement = self.0.prepare(
            "SELECT app_id, app_name, MAX(page IS NOT NULL) FROM capture_spans
             WHERE app_id IS NOT NULL AND app_name IS NOT NULL AND app_id != 'windows:desktop'
             GROUP BY app_id ORDER BY MAX(end_ms) DESC",
        )?;
        statement.query_map([], |row| {
            let id: String = row.get(0)?;
            let name: String = row.get(1)?;
            let eligible = !preferences::browser_name(&name);
            let identified: bool = row.get(2)?;
            let enabled = eligible && settings.documents.get(&id).copied()
                .unwrap_or_else(|| identified || preferences::default_documents(&name));
            Ok(DocumentApp { id, name, enabled, eligible })
        })?.collect()
    }

    pub fn set_document_capture(&self, app: &str, enabled: bool) -> rusqlite::Result<bool> {
        let name: Option<String> = self.0.query_row(
            "SELECT app_name FROM capture_spans WHERE app_id=? AND app_name IS NOT NULL AND app_id!='windows:desktop'
             ORDER BY start_ms DESC LIMIT 1", [app], |row| row.get(0),
        ).optional()?;
        if name.as_deref().is_none_or(preferences::browser_name) {
            return Ok(false);
        }
        self.0.execute(
            "INSERT INTO document_capture VALUES (?,?) ON CONFLICT(app_id) DO UPDATE SET enabled=excluded.enabled",
            params![app, enabled],
        )?;
        Ok(true)
    }

    pub fn last_end(&self) -> rusqlite::Result<Option<i64>> {
        self.0.query_row("SELECT MAX(end_ms) FROM capture_spans", [], |row| row.get(0))
    }

    pub fn append(&mut self, offset: i64, slices: &[Slice]) -> rusqlite::Result<()> {
        if slices.is_empty() {
            return Ok(());
        }
        let tx = self.0.transaction()?;
        let mut last = tx
            .query_row(
                "SELECT start_ms, end_ms, state, app_id, app_name, page, playback
                 FROM capture_spans ORDER BY start_ms DESC LIMIT 1",
                [],
                read_interval,
            )
            .optional()?;
        for slice in slices {
            let mut start = slice.start + offset;
            let end = slice.end + offset;
            if let Some(previous) = &last {
                start = start.max(previous.end_ms);
            }
            if start >= end {
                continue;
            }
            let state = slice.state as i64;
            let app = slice.target.as_ref();
            let app_id = app.map(|item| item.id.as_str());
            let app_name = app.map(|item| item.name.as_str());
            let page = app.and_then(|item| item.page.as_deref());
            if let Some(previous) = &mut last
                && previous.end_ms == start
                && previous.state == state
                && previous.media_exempt == slice.media_exempt
                && previous.app_id.as_deref() == app_id
                && previous.app_name.as_deref() == app_name
                && previous.page.as_deref() == page
            {
                tx.execute("UPDATE capture_spans SET end_ms = ? WHERE start_ms = ?", params![end, previous.start_ms])?;
                previous.end_ms = end;
                continue;
            }
            tx.execute(
                "INSERT INTO capture_spans(start_ms,end_ms,state,app_id,app_name,page,playback) VALUES (?,?,?,?,?,?,?)",
                params![start, end, state, app_id, app_name, page, slice.media_exempt],
            )?;
            last = Some(Interval {
                start_ms: start, end_ms: end, state, media_exempt: slice.media_exempt,
                app_id: app_id.map(str::to_owned),
                app_name: app_name.map(str::to_owned),
                page: page.map(str::to_owned),
                page_kind: page_kind(app_name, page),
            });
        }
        tx.commit()
    }

    pub fn range(&self, start: i64, end: i64) -> rusqlite::Result<Vec<Interval>> {
        let mut statement = self.0.prepare(
            "SELECT start_ms,end_ms,state,app_id,app_name,page,playback FROM capture_spans
             WHERE start_ms < ? AND end_ms > ? ORDER BY start_ms",
        )?;
        statement
            .query_map(params![end, start], |row| {
                let mut item = read_interval(row)?;
                item.start_ms = item.start_ms.max(start);
                item.end_ms = item.end_ms.min(end);
                Ok(item)
            })?
            .collect()
    }
}

fn has_playback_column(db: &Connection) -> rusqlite::Result<bool> {
    db.query_row("SELECT count(*) FROM pragma_table_info('capture_spans') WHERE name='playback'", [],
        |row| row.get::<_, u32>(0).map(|count| count != 0))
}

fn read_interval(row: &rusqlite::Row<'_>) -> rusqlite::Result<Interval> {
    let app_name: Option<String> = row.get(4)?;
    let page: Option<String> = row.get(5)?;
    Ok(Interval {
        start_ms: row.get(0)?,
        end_ms: row.get(1)?,
        state: row.get(2)?,
        app_id: row.get(3)?,
        page_kind: page_kind(app_name.as_deref(), page.as_deref()),
        app_name,
        page,
        media_exempt: row.get(6)?,
    })
}

fn page_kind(app_name: Option<&str>, page: Option<&str>) -> Option<&'static str> {
    page.map(|_| if app_name.is_some_and(|name| name.eq_ignore_ascii_case("Microsoft Edge")) {
        "site"
    } else {
        "document"
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::State;

    #[test]
    fn append_coalesces_and_clips_without_double_counting() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE capture_spans(start_ms INTEGER PRIMARY KEY, end_ms INTEGER NOT NULL,
                         state INTEGER NOT NULL, app_id TEXT, app_name TEXT, page TEXT, playback INTEGER NOT NULL DEFAULT 0);").unwrap();
        let mut store = Store(db);
        let first = Slice { start: 0, end: 20, state: State::Idle, media_exempt: false, target: None };
        let second = Slice { start: 20, end: 40, ..first.clone() };
        store.append(100, &[first.clone(), second, first]).unwrap();
        let rows = store.range(110, 130).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!((rows[0].start_ms, rows[0].end_ms, rows[0].state), (110, 130, 1));
    }
}
