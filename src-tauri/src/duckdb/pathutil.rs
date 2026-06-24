//! Path hygiene helpers.
//!
//! View-name generation (ASCII identifier from a possibly-Chinese file name)
//! now lives in [`crate::duckdb::naming`]. This module keeps the small
//! path-normalization helper used when embedding filesystem paths in SQL.

use std::path::Path;

/// Normalize a path to forward slashes for embedding inside DuckDB SQL.
pub fn forward_slashes(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}
