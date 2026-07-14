//! Sampled-aggregation guard: refuses to run a SQL aggregation over a locally
//! cached *sampled* table (which holds only a tiny subset of a remote table),
//! because aggregating it silently produces badly wrong metrics. Used by the
//! `execute_query` and `render_chart` tools.

/// Case-insensitive, word-boundary check: does `sql` mention identifier `ident`
/// as a standalone token (not as a substring of a larger identifier like
/// `total_sum` matching `sum`)?
pub(super) fn sql_contains_ident(sql: &str, ident: &str) -> bool {
    let sb = sql.as_bytes();
    let ib = ident.as_bytes();
    if ib.is_empty() || sb.len() < ib.len() {
        return false;
    }
    let is_id_byte = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    for i in 0..=sb.len() - ib.len() {
        if sb[i..i + ib.len()].eq_ignore_ascii_case(ib) {
            let before = if i == 0 { b' ' } else { sb[i - 1] };
            let after = if i + ib.len() >= sb.len() { b' ' } else { sb[i + ib.len()] };
            if !is_id_byte(before) && !is_id_byte(after) {
                return true;
            }
        }
    }
    false
}

/// If `sql` runs an aggregation (SUM/COUNT/AVG/GROUP BY/…) over one of the
/// locally-cached *sampled* tables, return that table's record so the caller
/// can emit a pushdown hint instead of running a misleading aggregation.
fn sampled_aggregation_target<'a>(
    sql: &str,
    sampled: &'a [&crate::db::SourceRecord],
) -> Option<&'a crate::db::SourceRecord> {
    const AGG_KW: &[&str] = &[
        "SUM", "COUNT", "AVG", "MIN", "MAX", "GROUP", "HAVING", "DISTINCT",
        "STDDEV", "VARIANCE", "MEDIAN", "PERCENTILE", "APPROX", "BOOL_AND",
        "BOOL_OR", "STRING_AGG", "LIST", "ARRAY_AGG",
    ];
    if !AGG_KW.iter().any(|kw| sql_contains_ident(sql, kw)) {
        return None;
    }
    sampled.iter().find(|r| sql_contains_ident(sql, &r.table_name)).copied()
}

/// Build the error string returned when an aggregation over an incomplete
/// (sampled or partially-materialized) table is intercepted — points the agent
/// at the native pushdown function or `materialize_remote_table` so it reruns
/// against full data.
fn build_intercept_message(rec: &crate::db::SourceRecord) -> String {
    let db_alias = rec.scan_path.split('.').next().unwrap_or("");
    let kind = rec.kind.as_str();
    let local_rows = rec.row_count.unwrap_or(0);
    let full_rows = rec.full_row_count.unwrap_or(0);
    let is_partial = matches!(rec.materialize_status.as_deref(), Some("partial"));
    let intro = if is_partial {
        format!("检测到对本地表 `{table}` 的聚合查询，但该表当前仅部分物化（已落盘 {local} 行 / 远程全量约 {full} 行），直接聚合会导致指标失真，已拒绝执行。",
            table = rec.table_name, local = local_rows, full = full_rows)
    } else {
        format!("检测到对本地采样缓存表 `{table}` 的聚合查询。该采样表仅含 {local} 行，而远程全量约 {full} 行——在采样表上聚合会导致指标严重失真（如 COUNT 远小于真实值），已拒绝执行。",
            table = rec.table_name, local = local_rows, full = full_rows)
    };
    let mat_hint = if is_partial {
        "2) 或再次调用 `materialize_remote_table` 续传至完成（已物化部分会自动跳过），再做本地聚合分析。"
    } else {
        "2) 或调用 `materialize_remote_table` 工具，将该表全量物化到本地后再做本地聚合分析。"
    };
    // MaxCompute has no DuckDB `{kind}_query` pushdown function (no extension) —
    // route the agent to the sidecar-based pushdown tool instead.
    if kind == "maxcompute" {
        return format!(
            "[已拦截] {intro}\n\n请改用全量路径之一：\n1) 原生下推（推荐，最快）：调用 `maxcompute_pushdown_query` 工具，传入 table_name=\"{table}\" 和你要执行的聚合 SQL（FROM 用该表在 MaxCompute 远程的 project.table，如 yantubi.dim_users_sc_track），只拉回结果行。\n{mat_hint}",
            intro = intro, table = rec.table_name, mat_hint = mat_hint,
        );
    }
    format!(
        "[已拦截] {intro}\n\n请改用全量路径之一：\n1) 原生下推（推荐，最快）：将聚合下推到远程库执行，只拉回结果，形如：\n   SELECT * FROM {kind}_query('{alias}', '<你的聚合 SQL；FROM 用该表在远程库的 schema.table>')\n{mat_hint}",
        intro = intro, kind = kind, alias = db_alias, mat_hint = mat_hint,
    )
}

/// Refuse to run `sql` if it aggregates over a locally-cached table whose local
/// copy would mislead — i.e. a *sampled* or *partially-materialized* table.
/// Shared by `execute_query` and `render_chart`. Must be called from the
/// blocking pool (touches SQLite).
pub(super) fn check_sampled_aggregation(sql: &str, ws_path: &str) -> Result<(), String> {
    let sqlite = crate::db::get_db_conn()?;
    let all = crate::db::list_sources(&sqlite, ws_path)?;
    // A table misleads aggregation when it is sampled OR only partially
    // materialized — both have incomplete local data. Fully-materialized tables
    // (materialize_status = "full") pass through.
    let incomplete: Vec<&crate::db::SourceRecord> = all.iter().filter(|r| r.aggregation_misleads()).collect();
    if let Some(rec) = sampled_aggregation_target(sql, &incomplete) {
        return Err(build_intercept_message(rec));
    }
    Ok(())
}

#[cfg(test)]
mod tests_sampled_intercept {
    use super::*;

    fn rec(name: &str) -> crate::db::SourceRecord {
        crate::db::SourceRecord {
            table_name: name.to_string(),
            label: String::new(),
            kind: "postgres".to_string(),
            storage: String::new(),
            file_path: String::new(),
            scan_path: format!("db_cdp.public.{}", name.trim_start_matches("s_")),
            partition_keys: Vec::new(),
            created_at: 0,
            name_source: String::new(),
            file_mtime: 0,
            file_size: 0,
            columns: Vec::new(),
            row_count: Some(1000),
            is_sampled: true,
            full_row_count: Some(1_000_000),
            materialize_status: Some(crate::db::mat_status::SAMPLED.to_string()),
            sheet: None,
        }
    }

    #[test]
    fn full_status_not_intercepted_partial_is() {
        // A fully-materialized table does NOT mislead aggregation.
        let mut full = rec("s_done");
        full.materialize_status = Some(crate::db::mat_status::FULL.to_string());
        full.is_sampled = false;
        assert!(!full.aggregation_misleads());

        // A partially-materialized table DOES mislead (incomplete local data).
        let mut partial = rec("s_partial");
        partial.materialize_status = Some(crate::db::mat_status::PARTIAL.to_string());
        partial.is_sampled = false;
        assert!(partial.aggregation_misleads());

        // Legacy NULL status falls back to is_sampled.
        let mut legacy = rec("s_legacy");
        legacy.materialize_status = None;
        assert!(legacy.aggregation_misleads()); // is_sampled=true
        legacy.is_sampled = false;
        assert!(!legacy.aggregation_misleads());
    }

    #[test]
    fn word_boundary_ident_match() {
        assert!(sql_contains_ident("SELECT COUNT(*) FROM s_users", "COUNT"));
        assert!(sql_contains_ident("SELECT COUNT(*) FROM s_users", "s_users"));
        assert!(sql_contains_ident("select count(*) from s_users", "COUNT"));
        assert!(sql_contains_ident("SELECT * FROM \"s_users\"", "s_users"));
        // NOT a substring of a larger identifier
        assert!(!sql_contains_ident("SELECT * FROM total_sum", "sum"));
        assert!(!sql_contains_ident("SELECT * FROM s_users_backup", "s_users"));
        assert!(!sql_contains_ident("SELECT * FROM s_users", "SUM"));
        assert!(!sql_contains_ident("SELECT * FROM xs_users", "s_users"));
    }

    #[test]
    fn intercept_only_on_aggregation_over_sampled() {
        let users = rec("s_users");
        let sampled = vec![&users];
        // aggregation over a sampled table → intercepted
        assert!(sampled_aggregation_target("SELECT COUNT(*) FROM s_users", &sampled).is_some());
        assert!(sampled_aggregation_target(
            "SELECT sum(amount) FROM s_users GROUP BY region",
            &sampled,
        )
        .is_some());
        // plain select (no aggregation) → not intercepted, even on a sampled table
        assert!(sampled_aggregation_target("SELECT * FROM s_users LIMIT 5", &sampled).is_none());
        // aggregation over a non-sampled table → not intercepted
        assert!(sampled_aggregation_target("SELECT COUNT(*) FROM s_other", &sampled).is_none());
    }

    #[test]
    fn intercept_message_names_table_and_pushdown() {
        let r = rec("s_users");
        let msg = build_intercept_message(&r);
        assert!(msg.contains("s_users"));
        assert!(msg.contains("postgres_query"));
        assert!(msg.contains("1000"));
        assert!(msg.contains("1000000"));
    }
}
