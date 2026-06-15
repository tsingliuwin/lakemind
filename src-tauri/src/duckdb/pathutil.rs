//! Path / identifier hygiene helpers.
//!
//! Two recurring problems when turning filesystem paths into DuckDB SQL:
//!
//! 1. **Windows backslashes** break string literals and globbing. DuckDB
//!    accepts forward slashes on every platform, so we normalize first.
//! 2. **View names** must be valid SQL identifiers; we derive them from file
//!    names and prefix with `s_` (SOURCE) to avoid collisions with future
//!    WORKSPACE tables (M2 will use a `t_` prefix).

use std::path::Path;

/// Normalize a path to forward slashes for embedding inside DuckDB SQL.
pub fn forward_slashes(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

/// Sanitize an arbitrary label into a snake_case-ish SQL-identifier-friendly
/// fragment. Non-identifier characters become `_`.
pub fn sanitize_label(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut prev_under = false;
    for (i, c) in raw.chars().enumerate() {
        let ok = c.is_ascii_alphanumeric() || c == '_';
        if i == 0 && c.is_ascii_digit() {
            out.push('_'); // identifiers can't start with a digit
        }
        if ok {
            out.push(c);
            prev_under = false;
        } else if !prev_under {
            out.push('_');
            prev_under = true;
        }
    }
    let s = out.trim_matches('_').to_string();
    if s.is_empty() {
        "source".to_string()
    } else {
        s
    }
}

/// Build a view name for a SOURCE label: `s_<sanitized>`.
pub fn to_view_name(label: &str) -> String {
    let s = sanitize_label(label);
    format!("s_{s}")
}
