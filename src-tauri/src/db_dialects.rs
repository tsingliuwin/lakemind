//! Per-database-dialect usage guidance - the "what surprises this DB backend"
//! cheat-sheet that the agent pulls in at the start of a conversation.
//!
//! Each backend (maxcompute, postgres, mysql, sqlite) has different SQL
//! dialect quirks (MaxCompute needs `project.table` and `ORDER BY ... LIMIT`,
//! postgres/mysql pushdown via `{kind}_query`, etc.). Rather than letting the
//! agent rediscover these by trial-and-error on every new conversation, this
//! module returns the relevant guidance text for the database types *actually
//! wired into the current workspace*.
//!
//! The agent fetches this via the `get_workspace_dialects` tool (parallel to
//! `get_current_time`) at the start of a turn, so the first SQL it writes
//! already respects the dialect. Guidance text lives in
//! `dialect_seed/<kind>.md` and is compiled in via `include_str!` (same pattern
//! as `tenets_seed/`). Only `maxcompute` is authored today; adding a new kind
//! is one `.md` file + one `match` arm in [`guidance_for_kind`].

use std::collections::BTreeSet;

// ── Seed guidance (compiled into the binary via include_str!) ──────────────
const MAXCOMPUTE_GUIDANCE: &str = include_str!("dialect_seed/maxcompute.md");

/// Database backend `kind`/`db_type` values that carry dialect guidance.
/// Lowercase strings, matching `SourceRecord.kind` / `DbConnectionRecord.db_type`.
/// File-format kinds (parquet/csv/json/...) and structural kinds (table/view)
/// are intentionally excluded - they have no remote-dialect quirks.
const DB_BACKEND_KINDS: &[&str] = &["postgres", "mysql", "sqlite", "maxcompute"];

/// Return the guidance markdown for a database backend kind, or `None` if no
/// guidance has been authored for it yet. Adding a backend = adding a seed file
/// + one `match` arm here.
fn guidance_for_kind(kind: &str) -> Option<&'static str> {
    match kind.to_ascii_lowercase().as_str() {
        "maxcompute" => Some(MAXCOMPUTE_GUIDANCE),
        // postgres / mysql / sqlite: guidance not yet authored. When added,
        // they return Some(...) here and are automatically picked up by
        // `compose_block` - no other code change required.
        _ => None,
    }
}

/// Is `kind` a known database backend (vs a file format or structural kind)?
fn is_db_backend(kind: &str) -> bool {
    let lower = kind.to_ascii_lowercase();
    DB_BACKEND_KINDS.iter().any(|k| *k == lower.as_str())
}

/// Collect the distinct database backend kinds wired into `ws_path`, drawing
/// from both registered DB connections and registered sources. Deduplicated and
/// sorted (via `BTreeSet`) for a stable ordering. Touches SQLite - must be
/// called from the blocking pool (see `get_workspace_dialects` tool).
fn active_db_kinds(ws_path: &str) -> Vec<String> {
    let mut kinds: BTreeSet<String> = BTreeSet::new();
    if let Ok(conn) = crate::db::get_db_conn() {
        // Registered external DB connections (postgres/mysql/sqlite/maxcompute).
        if let Ok(conns) = crate::db::list_workspace_connections(&conn, ws_path) {
            for c in conns {
                if is_db_backend(&c.db_type) {
                    kinds.insert(c.db_type.to_ascii_lowercase());
                }
            }
        }
        // Registered sources - also carry a `kind` (includes file formats,
        // which `is_db_backend` filters out).
        if let Ok(sources) = crate::db::list_sources(&conn, ws_path) {
            for s in sources {
                if is_db_backend(&s.kind) {
                    kinds.insert(s.kind.to_ascii_lowercase());
                }
            }
        }
    }
    kinds.into_iter().collect()
}

/// Compose the dialect-guidance block from a list of backend kinds. Returns
/// `None` when no guidance is available for any of the given kinds (so a
/// workspace with only file sources, or only not-yet-authored backends, yields
/// `None` and the tool emits a "nothing special" message instead of an empty
/// block). Pure function - unit-testable without SQLite.
fn compose_block(kinds: &[String]) -> Option<String> {
    let bodies: Vec<&str> = kinds
        .iter()
        .filter_map(|k| guidance_for_kind(k))
        .collect();
    if bodies.is_empty() {
        return None;
    }
    let mut out = String::from(
        "# 当前工作区接入的数据库方言要点\n\
         本工作区接入了以下数据库类型，编写/下推 SQL 时务必遵循其方言规则（已列出常见报错码，便于你自识别）：\n",
    );
    for body in bodies {
        out.push_str("\n");
        out.push_str(body);
        out.push('\n');
    }
    Some(out)
}

/// Build the dialect-guidance block for the database types actually wired into
/// `ws_path`, or `None` when none of them have authored guidance yet. Called
/// from the `get_workspace_dialects` tool (which wraps the SQLite access in
/// `spawn_blocking`).
pub fn active_dialect_block(ws_path: &str) -> Option<String> {
    let kinds = active_db_kinds(ws_path);
    compose_block(&kinds)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guidance_for_known_and_unknown() {
        let mc = guidance_for_kind("maxcompute");
        assert!(mc.is_some(), "maxcompute should have guidance");
        assert!(!mc.unwrap().is_empty(), "maxcompute guidance is non-empty");
        // Case-insensitive.
        assert!(guidance_for_kind("MaxCompute").is_some());
        assert!(guidance_for_kind("MAXCOMPUTE").is_some());
        // Not yet authored.
        assert!(guidance_for_kind("postgres").is_none());
        assert!(guidance_for_kind("mysql").is_none());
        assert!(guidance_for_kind("sqlite").is_none());
        // Unknown.
        assert!(guidance_for_kind("parquet").is_none());
        assert!(guidance_for_kind("unknown").is_none());
        assert!(guidance_for_kind("").is_none());
    }

    fn s(v: &str) -> String {
        v.to_string()
    }

    #[test]
    fn compose_block_empty_is_none() {
        assert!(compose_block(&[]).is_none());
    }

    #[test]
    fn compose_block_with_only_unauthored_kinds_is_none() {
        // postgres/mysql/sqlite guidance not yet authored -> None.
        assert!(compose_block(&[s("postgres")]).is_none());
        assert!(compose_block(&[s("postgres"), s("mysql"), s("sqlite")]).is_none());
    }

    #[test]
    fn compose_block_maxcompute_present() {
        let block = compose_block(&[s("maxcompute")]).expect("maxcompute yields a block");
        assert!(block.contains("# 当前工作区接入的数据库方言要点"));
        assert!(block.contains("MaxCompute"));
        assert!(block.contains("project.table"));
        assert!(block.contains("ODPS-0130131"));
        assert!(block.contains("ODPS-0130071"));
        assert!(block.contains("ORDER BY"));
        assert!(block.contains("LIMIT"));
    }

    #[test]
    fn compose_block_skips_unauthored_keeps_authored() {
        // Mix of authored + unauthored: only maxcompute content appears.
        let block = compose_block(&[s("maxcompute"), s("postgres"), s("mysql")])
            .expect("at least maxcompute is authored");
        assert!(block.contains("MaxCompute"));
        // postgres/mysql sections absent (not authored).
        assert!(!block.contains("postgres_query"));
        assert!(!block.contains("mysql_query"));
    }

    #[test]
    fn is_db_backend_filters_file_formats() {
        assert!(is_db_backend("postgres"));
        assert!(is_db_backend("MaxCompute")); // case-insensitive
        assert!(!is_db_backend("parquet"));
        assert!(!is_db_backend("csv"));
        assert!(!is_db_backend("json"));
        assert!(!is_db_backend("table"));
        assert!(!is_db_backend("view"));
        assert!(!is_db_backend(""));
    }

    #[test]
    fn seed_guidance_is_nonempty() {
        assert!(!MAXCOMPUTE_GUIDANCE.is_empty());
    }
}
