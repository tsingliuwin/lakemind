//! Input-fingerprint computation for incremental builds.
//!
//! A `t_`/`v_` object built by the agent depends on a set of upstream objects
//! (source tables `s_*` and other derived objects `t_*`/`v_*`). To decide
//! whether re-running `CREATE TABLE t_x AS <select_sql>` can be skipped, we hash
//! the *current* fingerprints of every upstream object referenced by the
//! `select_sql`. If the combined hash is unchanged since last build, the lake
//! object is still valid and the expensive re-materialization is skipped.
//!
//! Fingerprint units:
//!   * `s_*` source → `file_mtime + file_size` (from the `sources` table)
//!   * `t_*`/`v_*`   → its own `input_hash` (from the `object_defs` table),
//!                      recursively — so a change deep in the chain propagates.

use std::collections::{HashMap, HashSet};

use rusqlite::Connection;

/// Identifiers we treat as "our" lake objects (vs. read_xxx('...') functions or
/// quoted literals). Anything starting with one of these is a candidate upstream.
fn looks_like_lake_object(name: &str) -> bool {
    name.starts_with("s_")
        || name.starts_with("t_")
        || name.starts_with("v_")
        || name.starts_with("tmp_")
        || name.starts_with("tmp_v_")
}

/// Extract upstream object names referenced after `FROM`/`JOIN` in a SELECT.
///
/// A lightweight scanner (no regex dependency): it walks the SQL, and whenever
/// it sees a `FROM` or `JOIN` keyword it grabs the next bareword (optionally
/// quoted with `"` or backtick). Subquery-openers `(` and `read_*(` function
/// calls are tolerated — a `FROM read_parquet(...)` yields `read_parquet` which
/// `looks_like_lake_object` rejects. Deduplicated, order preserved.
pub fn extract_upstreams(select_sql: &str) -> Vec<String> {
    let upper = select_sql.to_uppercase();
    let bytes = select_sql.as_bytes();
    let mut out: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut i = 0;
    while i < upper.len() {
        // Find the next FROM/JOIN keyword boundary.
        if matches_keyword(&upper, i, "FROM") || matches_keyword(&upper, i, "JOIN") {
            let kw_len = if matches_keyword(&upper, i, "FROM") { 4 } else { 4 };
            let mut j = i + kw_len;
            // Skip whitespace and stray '(' (subquery start, e.g. "FROM (SELECT ...)").
            while j < bytes.len() && (bytes[j] == b' ' || bytes[j] == b'\t' || bytes[j] == b'\n' || bytes[j] == b'(') {
                j += 1;
            }
            // Read one bareword (alnum/_), or a quoted identifier.
            if let Some((name, end)) = read_identifier(bytes, j) {
                if looks_like_lake_object(&name) && seen.insert(name.clone()) {
                    out.push(name);
                }
                i = end;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// True if `upper` has `kw` (case-folded already) at position `i`, surrounded
/// by non-identifier boundaries.
fn matches_keyword(upper: &str, i: usize, kw: &str) -> bool {
    let ub = upper.as_bytes();
    let kb = kw.as_bytes();
    if i + kb.len() > ub.len() {
        return false;
    }
    if &ub[i..i + kb.len()] != kb {
        return false;
    }
    // Boundary before.
    if i > 0 {
        let prev = ub[i - 1];
        if prev.is_ascii_alphanumeric() || prev == b'_' {
            return false;
        }
    }
    // Boundary after.
    let after = i + kb.len();
    if after < ub.len() {
        let nxt = ub[after];
        if nxt.is_ascii_alphanumeric() || nxt == b'_' {
            return false;
        }
    }
    true
}

/// Read one identifier starting at `start` (skipping an optional quote char),
/// returning `(name, next_index)`.
fn read_identifier(bytes: &[u8], start: usize) -> Option<(String, usize)> {
    if start >= bytes.len() {
        return None;
    }
    let first = bytes[start];
    // Quoted identifier: "..." or `...`
    if first == b'"' || first == b'`' {
        let quote = first;
        let mut j = start + 1;
        while j < bytes.len() && bytes[j] != quote {
            j += 1;
        }
        if j < bytes.len() {
            let name = String::from_utf8_lossy(&bytes[start + 1..j]).to_string();
            return Some((name, j + 1));
        }
        return None;
    }
    // Bareword: alnum + _.
    if !first.is_ascii_alphabetic() && first != b'_' {
        return None;
    }
    let mut j = start;
    while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_') {
        j += 1;
    }
    if j == start {
        return None;
    }
    let name = String::from_utf8_lossy(&bytes[start..j]).to_string();
    Some((name, j))
}

/// Combined input hash for an object defined by `select_sql` that references
/// `upstreams`.
///
/// The hash covers TWO things so that a stale cache is never served:
///   1. **The definition itself** — `select_sql` is hashed first. If the agent
///      rebuilds an object with a different query (e.g. added a WHERE, changed a
///      JOIN) while referencing the *same* upstream tables, the hash still
///      changes → the object is correctly rebuilt instead of reused.
///   2. **Each upstream's fingerprint:**
///      * registered `s_*` source → `<mtime>:<size>`
///      * `t_*`/`v_*` with an `object_defs` row → its stored `input_hash`
///      * otherwise (unknown) → `?` (forces a miss → rebuild, never stale)
///
/// Upstream units are sorted by name for determinism.
pub fn compute_input_hash(
    conn: &Connection,
    ws_path: &str,
    select_sql: &str,
    upstreams: &[String],
) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();

    // 1. The definition: any change to the SQL itself (logic, columns, filters)
    //    invalidates the cache even when the upstream tables are identical.
    select_sql.hash(&mut hasher);
    b'#'.hash(&mut hasher); // separator between definition and upstreams

    // 2. Upstream fingerprints (may be empty — e.g. SELECT 1 — but the SQL part
    //    above still makes the hash meaningful).
    if !upstreams.is_empty() {
        let source_fp: HashMap<String, String> = match crate::db::list_sources(conn, ws_path) {
            Ok(rows) => rows
                .iter()
                .map(|r| (r.table_name.clone(), format!("{}:{}", r.file_mtime, r.file_size)))
                .collect(),
            Err(_) => HashMap::new(),
        };
        let def_fp: HashMap<String, String> = list_all_object_hashes(conn, ws_path);

        let mut units: Vec<(String, String)> = Vec::new();
        for name in upstreams {
            let fp = if let Some(f) = source_fp.get(name) {
                f.clone()
            } else if let Some(f) = def_fp.get(name) {
                f.clone()
            } else {
                // Unknown upstream → conservative: treat as volatile so we never
                // serve a stale cached object.
                "?".to_string()
            };
            units.push((name.clone(), fp));
        }
        units.sort_by(|a, b| a.0.cmp(&b.0));

        for (name, fp) in &units {
            name.hash(&mut hasher);
            b'|'.hash(&mut hasher);
            fp.hash(&mut hasher);
            b';'.hash(&mut hasher);
        }
    }

    format!("{:x}", hasher.finish())
}

/// Read every `(table_name, input_hash)` from `object_defs` for one workspace.
fn list_all_object_hashes(conn: &Connection, ws_path: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let Ok(mut stmt) = conn.prepare(
        "SELECT table_name, input_hash FROM object_defs WHERE workspace_path = ?",
    ) else {
        return out;
    };
    let Ok(rows) = stmt.query_map([ws_path], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    }) else {
        return out;
    };
    for r in rows.flatten() {
        out.insert(r.0, r.1);
    }
    out
}

/// Get the upstream object names for a given table/view by looking up its
/// `object_defs.select_sql` and running `extract_upstreams`. Returns empty for
/// source tables (s_*) which have no select_sql — they depend on files, not
/// other lake objects.
pub fn get_upstreams(conn: &Connection, ws_path: &str, table_name: &str) -> Vec<String> {
    let Ok(def) = crate::db::get_object_def(conn, ws_path, table_name) else {
        return Vec::new();
    };
    match def {
        Some(d) => extract_upstreams(&d.select_sql),
        None => Vec::new(),
    }
}

/// Build a reverse dependency map: for each object in `object_defs`, extract its
/// upstreams; then invert so the result maps `upstream_name → [objects that
/// depend on it]`. This lets us answer "who depends on table X?" in O(1).
pub fn build_downstream_map(conn: &Connection, ws_path: &str) -> HashMap<String, Vec<String>> {
    let mut map: HashMap<String, Vec<String>> = HashMap::new();
    let Ok(defs) = crate::db::list_object_defs(conn, ws_path) else {
        return map;
    };
    for d in &defs {
        let ups = extract_upstreams(&d.select_sql);
        for u in ups {
            map.entry(u).or_default().push(d.table_name.clone());
        }
    }
    map
}

/// Get the downstream objects that depend on `table_name`. Convenience wrapper
/// around `build_downstream_map` for a single lookup.
pub fn get_downstreams(conn: &Connection, ws_path: &str, table_name: &str) -> Vec<String> {
    let map = build_downstream_map(conn, ws_path);
    map.get(table_name).cloned().unwrap_or_default()
}

/// Compute the cascade deletion order for `table_name` and all its transitive
/// downstreams. Returns names ordered so that dependents come **before** their
/// dependencies — i.e. leaf-most objects first, the target last. This way a
/// single batch of DROP statements removes everything without "still
/// referenced" errors.
///
/// Cycle-safe via a `visited` set (cyclic deps shouldn't happen in practice,
/// but we guard anyway).
pub fn cascade_delete_order(conn: &Connection, ws_path: &str, table_name: &str) -> Vec<String> {
    let map = build_downstream_map(conn, ws_path);
    let mut result: Vec<String> = Vec::new();
    let mut visited: std::collections::HashSet<String> = std::collections::HashSet::new();
    collect_cascade(table_name, &map, &mut result, &mut visited);
    result
}

fn collect_cascade(
    name: &str,
    map: &HashMap<String, Vec<String>>,
    result: &mut Vec<String>,
    visited: &mut std::collections::HashSet<String>,
) {
    if !visited.insert(name.to_string()) {
        return; // already processed (or cycle)
    }
    // Visit downstreams first (they depend on `name`, so must be deleted before it).
    if let Some(deps) = map.get(name) {
        for d in deps {
            collect_cascade(d, map, result, visited);
        }
    }
    result.push(name.to_string());
}
