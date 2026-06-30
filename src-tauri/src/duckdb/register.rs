//! SOURCE registration: turn a [`ScanEntry`] into a DuckLake table or view.
//!
//! Two storage modes, chosen by the caller (commands layer) using the
//! configurable zero-copy threshold:
//!   * [`StorageKind::Table`] — materialize into a DuckLake table (small files).
//!     Multi-strategy loaders so messy CSV/Excel exports still ingest cleanly.
//!   * [`StorageKind::View`]  — zero-copy VIEW over the external `read_*` path
//!     (large files). The source file is NOT copied; the view reads it in place.
//!
//! Both run inside the current session, whose default catalog is the workspace's
//! DuckLake (`USE lake`), so unqualified `s_xxx` names land in the lake either way.

use duckdb::Connection;

use crate::duckdb::scan::ScanEntry;
use crate::duckdb::schema;
use crate::error::AppResult;
use crate::model::{SourceKind, SourceTable, StorageKind};

/// Create the table/view for one scan entry and return a fully-populated
/// `SourceTable` (with columns + row-count already filled in).
///
/// `progress` is an optional callback for UI progress reporting (file import
/// path). Called with human-readable stage messages. Pass `None` for
/// agent/internal paths.
pub fn register(
    conn: &Connection,
    e: &ScanEntry,
    storage: StorageKind,
    progress: Option<&dyn Fn(&str)>,
) -> AppResult<SourceTable> {
    match storage {
        StorageKind::View => create_view(conn, e)?,
        StorageKind::Table => create_table(conn, e, progress)?,
        StorageKind::Custom => unreachable!("custom sources are not registered from scan entries"),
    }

    let columns = schema::describe_view(conn, &e.view_name)?;
    let row_count_estimate = schema::estimate_row_count(conn, e).unwrap_or(None);

    Ok(SourceTable {
        name: e.view_name.clone(),
        label: e.label.clone(),
        kind: e.kind.clone(),
        storage,
        path: e.path.clone(),
        scan_path: e.scan_path.clone(),
        partition_keys: e.partition_keys.clone(),
        row_count_estimate,
        columns,
        is_sampled: false,
        full_row_count: None,
    })
}

/// Zero-copy: `CREATE VIEW s_xxx AS SELECT * FROM read_xxx('外部路径')`.
fn create_view(conn: &Connection, e: &ScanEntry) -> AppResult<()> {
    // A name could previously exist as either flavor; drop both to be safe.
    conn.execute(&format!("DROP VIEW IF EXISTS \"{}\";", e.view_name), [])?;
    conn.execute(&format!("DROP TABLE IF EXISTS \"{}\";", e.view_name), [])?;
    let sql = format!("CREATE VIEW \"{}\" AS {};", e.view_name, build_select_sql(e));
    conn.execute(&sql, [])?;
    Ok(())
}

/// Materialized: `CREATE TABLE s_xxx AS SELECT ...`, with multi-strategy
/// loaders for CSV/Excel.
fn create_table(conn: &Connection, e: &ScanEntry, progress: Option<&dyn Fn(&str)>) -> AppResult<()> {
    match e.kind {
        SourceKind::Csv => load_csv_as_table(conn, &e.view_name, &e.scan_path, progress)?,
        SourceKind::Excel => load_xlsx_as_table(conn, &e.view_name, &e.scan_path, progress)?,
        _ => {
            conn.execute(&format!("DROP TABLE IF EXISTS \"{}\";", e.view_name), [])?;
            let sql = format!("CREATE TABLE \"{}\" AS {};", e.view_name, build_select_sql(e));
            conn.execute(&sql, [])?;
        }
    }
    Ok(())
}

/// Build the `SELECT * FROM read_xxx('...')` body shared by the table & view paths.
/// Partition clause is added for Parquet when Hive keys were detected.
fn build_select_sql(e: &ScanEntry) -> String {
    let scan = e.scan_path.replace('\'', "''");
    let partition_clause = if e.partition_keys.is_empty() {
        String::new()
    } else {
        ", hive_partitioning = 1".to_string()
    };
    match e.kind {
        SourceKind::Parquet => format!("SELECT * FROM read_parquet('{scan}'{partition_clause})"),
        SourceKind::Csv => format!("SELECT * FROM read_csv_auto('{scan}', header = true)"),
        SourceKind::Json => format!("SELECT * FROM read_json_auto('{scan}')"),
        SourceKind::Excel => format!("SELECT * FROM read_xlsx('{scan}')"),
        SourceKind::Delta => format!("SELECT * FROM delta('{scan}')"),
        SourceKind::Table | SourceKind::View => unreachable!("Table/View are not raw-path sources"),
        SourceKind::Postgres | SourceKind::Mysql => unreachable!("Postgres/Mysql sources are registered through register_database_table"),
    }
}

/// Helper function to create a table and check if it has a valid schema (≥ 2 columns).
fn try_create_and_validate(conn: &Connection, table_name: &str, source_fn: &str) -> AppResult<bool> {
    let drop_sql = format!("DROP TABLE IF EXISTS \"{}\";", table_name);
    let _ = conn.execute(&drop_sql, []);

    let create_sql = format!("CREATE TABLE \"{table_name}\" AS SELECT * FROM {source_fn};");
    println!("Executing creation query: {}", create_sql);
    if let Err(e) = conn.execute(&create_sql, []) {
        println!("Execution failed: {}", e);
        return Ok(false);
    }

    // Check columns
    let check_sql = format!("SELECT count(column_name) FROM (DESCRIBE \"{table_name}\")");
    let col_count = conn.query_row(&check_sql, [], |row| row.get::<_, i64>(0)).unwrap_or(0);
    println!("Succeeded! Column count: {}", col_count);
    if col_count > 1 {
        Ok(true)
    } else {
        println!("Rejected (column count <= 1)");
        let _ = conn.execute(&drop_sql, []);
        Ok(false)
    }
}

/// Multi-strategy CSV loader
fn load_csv_as_table(conn: &Connection, table_name: &str, file_path: &str, _progress: Option<&dyn Fn(&str)>) -> AppResult<()> {
    let drop_sql = format!("DROP TABLE IF EXISTS \"{}\";", table_name);
    let escaped_path = file_path.replace('\'', "''");

    // Strategy 1: sniff_csv pre-check
    let sniff_sql = format!("SELECT count(column_name) FROM (DESCRIBE (SELECT * FROM sniff_csv('{escaped_path}')))");
    let sniff_ok = conn.query_row(&sniff_sql, [], |row| row.get::<_, i64>(0)).unwrap_or(0) > 1;

    if sniff_ok {
        let source = format!("read_csv_auto('{escaped_path}', ignore_errors=true, null_padding=true)");
        if try_create_and_validate(conn, table_name, &source)? {
            return Ok(());
        }
    }

    // Strategy 2: full-file scan
    let full_scan = format!("read_csv_auto('{escaped_path}', sample_size=-1, ignore_errors=true, null_padding=true)");
    if try_create_and_validate(conn, table_name, &full_scan)? {
        return Ok(());
    }

    // Strategy 3: try common delimiters
    let delimiters = [";", "\t", "|"];
    for delim in &delimiters {
        let source = format!(
            "read_csv_auto('{escaped_path}', delim='{delim}', sample_size=-1, ignore_errors=true, null_padding=true)"
        );
        if try_create_and_validate(conn, table_name, &source)? {
            return Ok(());
        }
    }

    // Strategy 4: Try GBK encoding if standard encodings fail
    let _ = conn.execute("INSTALL encodings;", []);
    if conn.execute("LOAD encodings;", []).is_ok() {
        let gbk_source = format!("read_csv_auto('{escaped_path}', header = true, encoding = 'zh_CN.gbk')");
        if try_create_and_validate(conn, table_name, &gbk_source)? {
            return Ok(());
        }
        let gbk_robust = format!("read_csv_auto('{escaped_path}', header = true, encoding = 'zh_CN.gbk', ignore_errors=true, null_padding=true)");
        if try_create_and_validate(conn, table_name, &gbk_robust)? {
            return Ok(());
        }
    }

    // Fallback: accept best-effort full scan
    let _ = conn.execute(&drop_sql, []);
    let create_sql = format!("CREATE TABLE \"{table_name}\" AS SELECT * FROM {full_scan};");
    conn.execute(&create_sql, [])?;
    Ok(())
}

struct IngestionCandidate {
    source_fn: String,
    col_count: i64,
    header_score: usize,
}

fn evaluate_candidate(conn: &Connection, table_name: &str, source_fn: &str) -> AppResult<Option<IngestionCandidate>> {
    let drop_sql = format!("DROP TABLE IF EXISTS \"{}\";", table_name);
    let _ = conn.execute(&drop_sql, []);

    let create_sql = format!("CREATE TABLE \"{table_name}\" AS SELECT * FROM {source_fn};");
    println!("Evaluating candidate creation query: {}", create_sql);
    if let Err(e) = conn.execute(&create_sql, []) {
        println!("Candidate evaluation failed: {}", e);
        return Ok(None);
    }

    // Check columns
    let columns = match schema::describe_view(conn, table_name) {
        Ok(cols) => cols,
        Err(e) => {
            println!("Failed to describe table columns: {}", e);
            let _ = conn.execute(&drop_sql, []);
            return Ok(None);
        }
    };

    let col_count = columns.len() as i64;
    if col_count <= 1 {
        println!("Rejected candidate: column count <= 1");
        let _ = conn.execute(&drop_sql, []);
        return Ok(None);
    }

    // Score headers and penalize numeric headers in the first few columns
    let mut header_score = 0;
    let mut penalty = 0;
    for (i, col) in columns.iter().enumerate() {
        let name = col.name.trim();
        if name.is_empty() {
            continue;
        }
        if name.starts_with('_') {
            continue;
        }
        // Skip default Excel col names A, B, C... Z, AA, AB... ZZ
        if name.len() <= 3 && name.chars().all(|c| c.is_ascii_uppercase()) {
            continue;
        }
        // If a column name is numeric, it is likely a data row, not a header row
        let is_numeric = name.parse::<f64>().is_ok();
        if is_numeric {
            // Penalize heavily if the first few columns (typically Month, Name, ID, etc.) are numeric
            if i < 3 {
                penalty += 10;
            }
            continue;
        }
        header_score += 1;
    }

    let final_score = if header_score > penalty { header_score - penalty } else { 0 };

    // Clean up candidate table
    let _ = conn.execute(&drop_sql, []);

    println!("Candidate succeeded! Columns: {}, Original Score: {}, Penalty: {}, Final Score: {}", col_count, header_score, penalty, final_score);
    Ok(Some(IngestionCandidate {
        source_fn: source_fn.to_string(),
        col_count,
        header_score: final_score,
    }))
}

/// Multi-strategy Excel loader
fn load_xlsx_as_table(conn: &Connection, table_name: &str, file_path: &str, progress: Option<&dyn Fn(&str)>) -> AppResult<()> {
    let drop_sql = format!("DROP TABLE IF EXISTS \"{}\";", table_name);
    let escaped_path = file_path.replace('\'', "''");

    let mut candidates = Vec::new();

    // Helper: evaluate a strategy and report the result via progress.
    macro_rules! try_strategy {
        ($idx:expr, $label:expr, $source:expr) => {{
            if let Some(p) = progress { p(&format!("评估策略 {}/8: {}", $idx, $label)); }
            let src = $source;
            match evaluate_candidate(conn, table_name, &src)? {
                Some(c) => {
                    if let Some(p) = progress {
                        p(&format!("策略 {}/8 {}: {}列 评分{}", $idx, $label, c.col_count, c.header_score));
                    }
                    candidates.push(c);
                }
                None => {
                    if let Some(p) = progress {
                        p(&format!("策略 {}/8 {}: 失败或列数≤1", $idx, $label));
                    }
                }
            }
        }};
    }

    // Strategy 1: Default load
    try_strategy!(1, "默认读取", format!("read_xlsx('{escaped_path}')"));

    // Strategy 2-6: Try different header offsets (common in exported reports)
    let offsets = ["A1:ZZ100000", "A2:ZZ100000", "A3:ZZ100000", "A4:ZZ100000", "A5:ZZ100000"];
    for (i, r) in offsets.iter().enumerate() {
        let label = format!("表头偏移{}", r.split(':').next().unwrap_or(r));
        try_strategy!(i + 2, label, format!(
            "read_xlsx('{escaped_path}', header=true, range='{r}', stop_at_empty=false, ignore_errors=true)"
        ));
    }

    // Strategy 7: stop_at_empty=false + header + ignore_errors
    try_strategy!(7, "忽略错误全扫描", format!(
        "read_xlsx('{escaped_path}', header=true, stop_at_empty=false, ignore_errors=true)"
    ));

    // Strategy 8: all_varchar + stop_at_empty=false + ignore_errors
    try_strategy!(8, "全文本类型", format!(
        "read_xlsx('{escaped_path}', header=true, stop_at_empty=false, all_varchar=true, ignore_errors=true)"
    ));

    if !candidates.is_empty() {
        // Pick the candidate with the highest header_score.
        // If header_scores are equal, pick the one with fewer columns (cleaner range).
        candidates.sort_by(|a, b| {
            b.header_score.cmp(&a.header_score).then_with(|| a.col_count.cmp(&b.col_count))
        });

        let best = &candidates[0];
        println!("Selected best Excel ingestion strategy: {} (score: {}, cols: {})", best.source_fn, best.header_score, best.col_count);
        if let Some(p) = progress { p("过滤空列空行"); }
        return create_xlsx_table_pruned(conn, table_name, &best.source_fn);
    }

    // Fallback: accept best-effort result using Strategy 8 source (all_varchar)
    let _ = conn.execute(&drop_sql, []);
    let fallback_source = format!(
        "read_xlsx('{escaped_path}', header=true, stop_at_empty=false, all_varchar=true, ignore_errors=true)"
    );
    create_xlsx_table_pruned(conn, table_name, &fallback_source)?;
    Ok(())
}

/// Create the final table from an Excel `read_xlsx` source, pruning columns
/// that are entirely NULL and rows that are entirely empty.
///
/// Excel files often have a large "used range" (e.g. A1:ZZ100000) that includes
/// many empty columns and trailing empty rows. Materializing them as-is bloats
/// the DuckLake table and clutters analysis. Here we load into a temp table,
/// scan once to find which columns have any non-NULL value, then build the real
/// table selecting only those columns and only rows where at least one of them
/// is non-NULL.
fn create_xlsx_table_pruned(conn: &Connection, table_name: &str, source_fn: &str) -> AppResult<()> {
    let drop_sql = format!("DROP TABLE IF EXISTS \"{}\";", table_name);
    let _ = conn.execute(&drop_sql, []);

    // 1. Load the raw Excel data into a temp table.
    let tmp_name = format!("_tmp_{}", table_name);
    let _ = conn.execute(&format!("DROP TABLE IF EXISTS \"{}\";", tmp_name), []);
    let tmp_create = format!("CREATE TABLE \"{}\" AS SELECT * FROM {};", tmp_name, source_fn);
    conn.execute(&tmp_create, [])?;

    // Helper: always clean up the temp table before returning.
    let cleanup_tmp = || {
        let _ = conn.execute(&format!("DROP TABLE IF EXISTS \"{}\";", tmp_name), []);
    };

    // 2. Get the column list of the temp table.
    let columns = schema::describe_view(conn, &tmp_name).unwrap_or_default();
    if columns.is_empty() {
        cleanup_tmp();
        let create_sql = format!("CREATE TABLE \"{table_name}\" AS SELECT * FROM {source_fn};");
        conn.execute(&create_sql, [])?;
        return Ok(());
    }

    // 3. One single scan to get the non-NULL count for every column. We use
    //    DuckDB's positional column references (#1, #2, …) instead of column
    //    NAMES, because Excel often produces empty or odd column names that
    //    break SQL quoting (e.g. "" or names with embedded quotes). Positional
    //    refs sidestep all of that.
    let count_exprs: Vec<String> = (0..columns.len())
        .map(|i| format!("#{}", i + 1))
        .map(|r| format!("count({})", r))
        .collect();
    let count_sql = format!(
        "SELECT {} FROM \"{}\"",
        count_exprs.join(", "),
        tmp_name.replace('"', "\"\"")
    );
    let non_null_counts: Vec<i64> = match conn.query_row(&count_sql, [], |row| {
        let mut v = Vec::with_capacity(columns.len());
        for i in 0..columns.len() {
            v.push(row.get::<_, i64>(i).unwrap_or(0));
        }
        Ok(v)
    }) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("[xlsx] non-null count query failed: {e}, skipping prune");
            cleanup_tmp();
            let create_sql = format!("CREATE TABLE \"{table_name}\" AS SELECT * FROM {source_fn};");
            conn.execute(&create_sql, [])?;
            return Ok(());
        }
    };

    // 4. Keep only columns with at least one non-NULL value. Use positional
    //    refs again for the SELECT list to avoid name-quoting issues.
    let kept_positions: Vec<usize> = non_null_counts
        .iter()
        .enumerate()
        .filter(|(_, &cnt)| cnt > 0)
        .map(|(i, _)| i)
        .collect();

    println!(
        "[xlsx] {}: {} total cols, {} non-empty cols → keeping {}",
        table_name, columns.len(), kept_positions.len(), kept_positions.len()
    );

    if kept_positions.is_empty() {
        cleanup_tmp();
        let create_sql = format!("CREATE TABLE \"{table_name}\" AS SELECT * FROM {source_fn};");
        conn.execute(&create_sql, [])?;
        return Ok(());
    }

    // 5. Build the final table: select kept columns (by position) + rows where
    //    at least one kept column is non-NULL. We use `#1 IS NOT NULL OR #2 IS
    //    NOT NULL OR …` instead of COALESCE because COALESCE requires all
    //    arguments to share a type — Excel columns are often mixed VARCHAR/DOUBLE,
    //    which makes COALESCE fail with a binder error.
    let select_cols: Vec<String> = kept_positions.iter().map(|&i| format!("#{}", i + 1)).collect();
    let row_filter: Vec<String> = kept_positions.iter().map(|&i| format!("#{} IS NOT NULL", i + 1)).collect();
    let create_sql = format!(
        "CREATE TABLE \"{table}\" AS SELECT {cols} FROM \"{tmp}\" WHERE {filter};",
        table = table_name.replace('"', "\"\""),
        cols = select_cols.join(", "),
        tmp = tmp_name.replace('"', "\"\""),
        filter = row_filter.join(" OR ")
    );
    println!("Executing creation query: {}", create_sql);
    if let Err(e) = conn.execute(&create_sql, []) {
        eprintln!("[xlsx] pruned create failed: {e}, falling back to raw");
        cleanup_tmp();
        let fallback = format!("CREATE TABLE \"{table_name}\" AS SELECT * FROM {source_fn};");
        conn.execute(&fallback, [])?;
        return Ok(());
    }

    // 6. Clean up the temp table.
    cleanup_tmp();
    Ok(())
}
