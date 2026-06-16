//! Filesystem scanning and SOURCE classification.
//!
//! Given a dropped folder, `scan_path` walks the tree and groups files into
//! logical SOURCE candidates. Classification rules (see PRD §3.1):
//!
//! - **Delta**: any directory containing `_delta_log/` → one SOURCE per Delta table dir.
//! - **Parquet**: `*.parquet` files; all parquet files under the dropped root
//!   are folded into a *single* globbed view (the common multi-shard case),
//!   plus per-directory views when Hive partitions are detected.
//! - **CSV**: `*.csv` / `*.tsv` (one view per file or per directory glob).
//! - **JSON**: `*.json` / `*.ndjson`.
//!
//! Hive partition keys (`/year=2026/month=06/`) are detected by scanning the
//! relative path segments; DuckDB's `read_parquet` consumes them directly via
//! `hive_partitioning = 1`.

use std::path::{Path, PathBuf};

use walkdir::WalkDir;

use crate::duckdb::pathutil::{forward_slashes, sanitize_label, to_view_name};
use crate::model::SourceKind;

/// Maximum number of files to walk before bailing. Guards against pathological
/// inputs; a real 50GB lake rarely has more than ~100k shards.
const MAX_FILES: usize = 200_000;

/// A raw classified entry before it becomes a `SourceTable`. The scan step is
/// filesystem-only (no DuckDB I/O) so it stays fast and testable.
#[derive(Debug, Clone)]
pub struct ScanEntry {
    pub label: String,
    pub view_name: String,
    pub kind: SourceKind,
    pub path: String,
    pub scan_path: String,
    pub partition_keys: Vec<String>,
}

/// Walk `root` and produce a deduplicated list of SOURCE scan entries.
///
/// Order: Delta dirs first, then Parquet globs, then CSV, then JSON.
pub fn scan_path(root: &Path, is_workspace: bool) -> Vec<ScanEntry> {
    // First pass: collect raw files & detect Delta roots.
    let mut parquet_files: Vec<PathBuf> = Vec::new();
    let mut csv_files: Vec<PathBuf> = Vec::new();
    let mut json_files: Vec<PathBuf> = Vec::new();
    let mut excel_files: Vec<PathBuf> = Vec::new();
    let mut delta_roots: Vec<PathBuf> = Vec::new();
    let mut root_str = root.to_path_buf();

    // If the dropped path is itself a single file, treat it as the whole root.
    let is_file = root.is_file();
    if is_file {
        root_str = root
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| root.to_path_buf());
    }

    for entry in WalkDir::new(&root_str)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
        .take(MAX_FILES)
    {
        // Delta detection: a directory holding `_delta_log/` is a Delta table.
        if entry.file_type().is_dir() {
            if entry.path().join("_delta_log").exists() {
                delta_roots.push(entry.path().to_path_buf());
            }
            continue;
        }

        // If the user dropped a single file, only consider that file.
        if is_file && entry.path() != root {
            continue;
        }

        let path = entry.path().to_path_buf();
        match path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref() {
            // `.parq` is a widespread Parquet extension alias (Spark, etc.);
            // DuckDB's read_parquet reads it identically. Treat both as Parquet.
            Some("parquet") | Some("parq") => parquet_files.push(path),
            Some("csv") | Some("tsv") => csv_files.push(path),
            Some("json") | Some("ndjson") => json_files.push(path),
            Some("xlsx") | Some("xls") => excel_files.push(path),
            _ => {}
        }
    }

    let mut out: Vec<ScanEntry> = Vec::new();

    // Delta: one entry per detected table directory.
    for d in &delta_roots {
        out.push(entry_for(d, d, SourceKind::Delta, &root_str));
    }

    if is_workspace || is_file {
        // Individual files mapping strategy
        for f in &csv_files {
            out.push(build_individual_entry(f, SourceKind::Csv, &root_str));
        }
        for f in &json_files {
            out.push(build_individual_entry(f, SourceKind::Json, &root_str));
        }
        for f in &excel_files {
            out.push(build_individual_entry(f, SourceKind::Excel, &root_str));
        }
        
        // Group parquet files by parent directory to detect subdirectories (shards)
        let mut parquet_groups: std::collections::BTreeMap<PathBuf, Vec<PathBuf>> = std::collections::BTreeMap::new();
        for f in &parquet_files {
            let dir = f.parent().unwrap_or(Path::new("")).to_path_buf();
            parquet_groups.entry(dir).or_default().push(f.clone());
        }
        for (dir, files) in parquet_groups {
            if dir == root_str {
                // Direct files under workspace root -> map individually!
                for f in files {
                    out.push(build_individual_entry(&f, SourceKind::Parquet, &root_str));
                }
            } else {
                // Nested directory -> group them as a single view glob!
                let label = sanitize_label(dir.file_name().and_then(|s| s.to_str()).unwrap_or("parquet"));
                let keys = hive_keys_of(&dir, &root_str);
                let exts = present_parquet_exts(&files);
                out.push(build_entry(&label, &dir, SourceKind::Parquet, &root_str, keys, &exts));
            }
        }
    } else {
        // Legacy grouping strategy (for folder drag-and-drop / import)
        if !parquet_files.is_empty() {
            // Detect the actual extension present (`.parquet` vs `.parq`); DuckDB's
            // read_parquet does NOT support shell brace expansion like {a,b}, so we
            // must emit a glob matching the extension(s) actually on disk.
            let exts = present_parquet_exts(&parquet_files);
            // Common case: a single directory of shards → one view named after it.
            let parents: std::collections::BTreeSet<PathBuf> =
                parquet_files.iter().map(|p| p.parent().unwrap_or(Path::new("")).to_path_buf()).collect();

            if parents.len() == 1 {
                let dir = parents.into_iter().next().unwrap();
                let mut keys = hive_keys_of(&dir, &root_str);
                // If no keys at the immediate parent, probe one level up (common:
                // files sit under /year=2026/).
                if keys.is_empty() {
                    if let Some(grand) = dir.parent() {
                        keys = hive_keys_of(&grand.join("**"), &root_str);
                    }
                }
                out.push(build_entry("parquet_root", &dir, SourceKind::Parquet, &root_str, keys, &exts));
            } else {
                // Multiple directories (likely partitioned): glob the whole root.
                let keys = hive_keys_glob(&root_str);
                out.push(build_entry("parquet_glob", &root_str, SourceKind::Parquet, &root_str, keys, &exts));
            }
        }

        // CSV: group by directory.
        for (label, dir) in group_by_dir(&csv_files) {
            out.push(build_entry(&label, &dir, SourceKind::Csv, &root_str, Vec::new(), &[]));
        }
        // JSON: group by directory.
        for (label, dir) in group_by_dir(&json_files) {
            out.push(build_entry(&label, &dir, SourceKind::Json, &root_str, Vec::new(), &[]));
        }
        // Excel: register individually since read_xlsx doesn't support globbing.
        for f in &excel_files {
            out.push(build_individual_entry(f, SourceKind::Excel, &root_str));
        }
    }

    // Deduplicate by scan_path (a file might be caught twice on edge cases).
    out.dedup_by(|a, b| a.scan_path == b.scan_path);
    out
}

fn build_individual_entry(file: &Path, kind: SourceKind, root: &Path) -> ScanEntry {
    let label = file
        .file_stem()
        .and_then(|s| s.to_str())
        .map(sanitize_label)
        .unwrap_or_else(|| "data".to_string());
    let view_name = to_view_name(&label);
    let _ = root; // root retained for potential relative-path formatting later
    ScanEntry {
        label,
        view_name,
        kind,
        path: forward_slashes(file),
        scan_path: forward_slashes(file),
        partition_keys: Vec::new(),
    }
}

fn group_by_dir(files: &[PathBuf]) -> Vec<(String, PathBuf)> {
    let mut map: std::collections::BTreeMap<PathBuf, Vec<PathBuf>> = std::collections::BTreeMap::new();
    for f in files {
        let dir = f.parent().unwrap_or(Path::new("")).to_path_buf();
        map.entry(dir).or_default().push(f.clone());
    }
    map.into_iter()
        .map(|(dir, _)| {
            let label = sanitize_label(dir.file_name().and_then(|s| s.to_str()).unwrap_or("data"));
            (label, dir)
        })
        .collect()
}

fn build_entry(
    base_label: &str,
    dir: &Path,
    kind: SourceKind,
    root: &Path,
    partition_keys: Vec<String>,
    // For Parquet: the extensions actually present (e.g. ["parq"] or
    // ["parquet","parq"]). Used to build a DuckDB-compatible glob — DuckDB's
    // read_parquet does NOT support shell brace expansion, so when both
    // extensions coexist we glob `*` (the directory was already filtered to
    // parquet files by the scan). Ignored for non-parquet kinds.
    parquet_exts: &[&str],
) -> ScanEntry {
    let label = dir
        .file_name()
        .and_then(|s| s.to_str())
        .map(sanitize_label)
        .unwrap_or_else(|| base_label.to_string());
    let view_name = to_view_name(&label);
    let glob_tail = match kind {
        // Single extension → "*.<ext>"; mixed → "*" (all files here are parquet).
        SourceKind::Parquet => {
            if parquet_exts.len() == 1 {
                format!("*.{}", parquet_exts[0])
            } else {
                "*".to_string()
            }
        }
        SourceKind::Csv => "*.csv".to_string(),
        SourceKind::Json => "*.json*".to_string(),
        SourceKind::Excel => "*.xls*".to_string(),
        SourceKind::Delta => String::new(),
        SourceKind::Table | SourceKind::View => unreachable!("Table/View sources are not resolved as physical globs"),
    };
    let glob = forward_slashes(&dir.join(glob_tail));
    let _ = root; // root retained for potential relative-path formatting later
    let partition_keys = if matches!(kind, SourceKind::Parquet) && !partition_keys.is_empty() {
        partition_keys
    } else {
        Vec::new()
    };
    ScanEntry {
        label,
        view_name,
        kind,
        path: forward_slashes(dir),
        scan_path: glob,
        partition_keys,
    }
}

fn entry_for(dir: &Path, _src: &Path, kind: SourceKind, root: &Path) -> ScanEntry {
    build_entry("", dir, kind, root, Vec::new(), &[])
}

/// Distinct Parquet extensions present among `files`, in sorted order.
/// E.g. returns `["parq"]` or `["parquet","parq"]`.
fn present_parquet_exts(files: &[PathBuf]) -> Vec<&'static str> {
    let has_parquet = files.iter().any(|p| ext_eq(p, "parquet"));
    let has_parq = files.iter().any(|p| ext_eq(p, "parq"));
    let mut out: Vec<&'static str> = Vec::new();
    if has_parquet {
        out.push("parquet");
    }
    if has_parq {
        out.push("parq");
    }
    out
}

fn ext_eq(p: &Path, ext: &str) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case(ext))
        .unwrap_or(false)
}

/// Hive partition keys present in `dir`'s path relative to `root`.
fn hive_keys_of(dir: &Path, root: &Path) -> Vec<String> {
    let strip = dir.strip_prefix(root).unwrap_or(dir);
    let mut keys = Vec::new();
    for comp in strip.components() {
        if let std::path::Component::Normal(s) = comp {
            if let Some(s) = s.to_str() {
                if let Some((k, _)) = s.split_once('=') {
                    if is_valid_key(k) && !keys.contains(&k.to_string()) {
                        keys.push(k.to_string());
                    }
                }
            }
        }
    }
    keys
}

/// Detect Hive keys by scanning the whole root tree for `key=value` segments.
fn hive_keys_glob(root: &Path) -> Vec<String> {
    let mut keys = Vec::new();
    for entry in WalkDir::new(root).follow_links(false).into_iter().filter_map(|e| e.ok()).take(MAX_FILES) {
        let strip = entry.path().strip_prefix(root).unwrap_or(entry.path());
        for comp in strip.components() {
            if let std::path::Component::Normal(s) = comp {
                if let Some(s) = s.to_str() {
                    if let Some((k, _)) = s.split_once('=') {
                        if is_valid_key(k) && !keys.contains(&k.to_string()) {
                            keys.push(k.to_string());
                        }
                    }
                }
            }
        }
    }
    keys
}

fn is_valid_key(k: &str) -> bool {
    let mut chars = k.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}
