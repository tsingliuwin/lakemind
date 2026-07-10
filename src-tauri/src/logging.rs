//! Unified logging hub: a `tracing_subscriber` layer that persists every
//! backend event into the SQLite `logs` table and (for info+) pushes it to the
//! frontend console via the `app-log` channel.
//!
//! ## Call convention
//!
//! Every log call MUST set a `category` field so the event can be filed into
//! the fixed taxonomy ([`crate::model::LOG_CATEGORIES`]). Optional fields:
//! - `workspace = "..."` — associated workspace path
//! - `task_id = "..."` — associated agent task
//! - `detail = json!({...})` — structured payload (sql/rowCount/elapsedMs/...)
//!
//! ```ignore
//! tracing::info!(category = "sync", "{} fingerprint match → reuse", label);
//! tracing::warn!(category = "warmup", "rebuild {} failed: {e}", name);
//! tracing::error!(category = "system", workspace = %ws, "init failed: {e}");
//! ```
//!
//! ## Why a global AppHandle, not a Layer field
//!
//! The subscriber is initialized once and shared across all threads; `tracing`
//! layers must be `Sized + 'static`. The Tauri `AppHandle` is only available
//! after `Builder::setup`, so we stash it in a [`OnceLock`] from `setup` and the
//! layer reads it at emit time. Before the handle is set (very early boot),
//! events degrade gracefully to SQLite-only writes.

use std::sync::OnceLock;

use tauri::{AppHandle, Emitter};
use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_subscriber::Layer;
use tracing_subscriber::layer::Context;

use crate::model::{LogLevel, LogRecord};

/// Globally-set AppHandle used by [`SqliteEmitLayer`] to reach the SQLite store
/// and the frontend event bus. Set exactly once from `setup` via [`set_handle`].
static APP_HANDLE: OnceLock<AppHandle> = OnceLock::new();

/// Stash the Tauri handle so the logging layer can emit to the frontend and open
/// a SQLite connection. Idempotent; call once from `Builder::setup`.
pub fn set_handle(handle: AppHandle) {
    let _ = APP_HANDLE.set(handle);
}

/// Collect the `category` / `workspace` / `task_id` / `detail` custom fields and
/// the formatted message out of a `tracing::Event`.
struct LogFieldVisitor {
    category: Option<String>,
    workspace: Option<String>,
    task_id: Option<String>,
    detail: Option<serde_json::Value>,
    message: String,
}

impl LogFieldVisitor {
    fn new() -> Self {
        Self {
            category: None,
            workspace: None,
            task_id: None,
            detail: None,
            message: String::new(),
        }
    }
}

impl Visit for LogFieldVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        // The "message" field is the format-string body — render it without the
        // surrounding quotes that Debug would add.
        if field.name() == "message" {
            self.message = format!("{:?}", value).trim_matches('"').to_string();
            return;
        }
        match field.name() {
            "category" => self.category = Some(format!("{:?}", value).trim_matches('"').to_string()),
            "workspace" => self.workspace = Some(format!("{:?}", value).trim_matches('"').to_string()),
            "task_id" => self.task_id = Some(format!("{:?}", value).trim_matches('"').to_string()),
            "detail" => {
                // detail is recorded via tracing::field::display(json_string) so
                // the Debug form is the JSON string in quotes; parse it back.
                let raw = format!("{:?}", value).trim_matches('"').to_string();
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) {
                    self.detail = Some(v);
                }
            }
            _ => {}
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        match field.name() {
            "category" => self.category = Some(value.to_string()),
            "workspace" => self.workspace = Some(value.to_string()),
            "task_id" => self.task_id = Some(value.to_string()),
            _ => {}
        }
    }
}

/// A `tracing` layer that turns every event into a [`LogRecord`], writes it to
/// SQLite, and (info+) emits it to the frontend `app-log` channel.
pub struct SqliteEmitLayer;

impl SqliteEmitLayer {
    pub fn new() -> Self {
        Self
    }
}

impl<S> Layer<S> for SqliteEmitLayer
where
    S: Subscriber,
{
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let level = LogLevel::from_tracing(event.metadata().level());

        let mut visitor = LogFieldVisitor::new();
        event.record(&mut visitor);

        // `category` is mandatory per convention; fall back to the module path's
        // last segment so an untagged call still files somewhere sensible.
        let category = visitor.category.unwrap_or_else(|| {
            event
                .metadata()
                .target()
                .rsplit("::")
                .next()
                .unwrap_or("system")
                .to_string()
        });

        let rec = LogRecord {
            id: None,
            ts: crate::db::now_ms(),
            level,
            category,
            message: visitor.message,
            detail: visitor.detail,
            workspace: visitor.workspace,
            task_id: visitor.task_id,
        };

        // Persist + emit. Both are best-effort: logging must NEVER panic or
        // propagate errors into business code. We swallow everything.
        let Some(handle) = APP_HANDLE.get() else {
            // Very early boot (before setup) — SQLite may not even be ready.
            // Best-effort direct write so we don't lose startup diagnostics.
            try_write_sqlite_only(&rec);
            return;
        };

        let mut to_emit: Option<LogRecord> = None;
        match crate::db::get_db_conn() {
            Ok(conn) => match crate::db::insert_log(&conn, &rec) {
                Ok(id) => {
                    let mut e = rec.clone();
                    e.id = Some(id);
                    to_emit = Some(e);
                }
                Err(e) => {
                    // Avoid recursion: do NOT log this via tracing.
                    eprintln!("[logging] insert_log failed: {e}");
                }
            },
            Err(e) => {
                eprintln!("[logging] get_db_conn failed: {e}");
            }
        }

        // Only push info+ to the frontend — debug/trace would flood the console.
        if let Some(rec) = to_emit {
            if !matches!(level, LogLevel::Debug) {
                let _ = handle.emit("app-log", &rec);
            }
        }
    }
}

fn try_write_sqlite_only(rec: &LogRecord) {
    if let Ok(conn) = crate::db::get_db_conn() {
        let _ = crate::db::insert_log(&conn, rec);
    }
}
