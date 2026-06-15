//! Unified error type for the LakeMind backend.
//!
//! All errors are flattened to a single string (via `Display`) so that Tauri
//! can ship them to the frontend as the `Err(String)` half of a command.
//! We deliberately preserve the *original* DuckDB message verbatim (including
//! hints like "column name 'category' is mispelled") so that M2's Agent can
//! feed the raw hint into its self-correction loop.

use std::fmt;

/// The single error type surfaced out of the backend.
pub struct AppError(pub String);

impl AppError {
    pub fn new(msg: impl Into<String>) -> Self {
        Self(msg.into())
    }
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Tauri serializes command errors via Debug; make it human-readable
        // rather than the noisy default struct dump.
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for AppError {}

// --- Convenience conversions from third-party errors -----------------------

impl From<duckdb::Error> for AppError {
    fn from(e: duckdb::Error) -> Self {
        // Keep the raw DuckDB message so callers (and, later, the Agent) can
        // see column / parse hints without any wrapping noise.
        AppError(e.to_string())
    }
}

impl From<std::io::Error> for AppError {
    fn from(e: std::io::Error) -> Self {
        AppError(format!("io: {e}"))
    }
}

impl From<serde_json::Error> for AppError {
    fn from(e: serde_json::Error) -> Self {
        AppError(format!("json: {e}"))
    }
}

/// Convenience alias used across command implementations.
pub type AppResult<T> = std::result::Result<T, AppError>;
