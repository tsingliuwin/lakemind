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

use std::io::Read;

use crate::duckdb::scan::ScanEntry;
use crate::duckdb::schema;
use crate::error::AppResult;
use crate::model::{ColumnInfo, SourceKind, SourceTable, StorageKind};

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
        sheet: e.sheet.clone(),
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
        SourceKind::Excel => load_xlsx_as_table(conn, &e.view_name, &e.scan_path, e.sheet.as_deref(), progress)?,
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
        SourceKind::Excel => match &e.sheet {
            Some(sheet) => {
                let esc_sheet = sheet.replace('\'', "''");
                format!("SELECT * FROM read_xlsx('{scan}', sheet='{esc_sheet}')")
            }
            None => format!("SELECT * FROM read_xlsx('{scan}')"),
        },
        SourceKind::Delta => format!("SELECT * FROM delta('{scan}')"),
        SourceKind::Table | SourceKind::View => unreachable!("Table/View are not raw-path sources"),
        SourceKind::Postgres | SourceKind::Mysql | SourceKind::Sqlite => unreachable!("external-database sources are registered through register_database_table"),
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

    // Check columns via the real column names (not just a count). DuckDB
    // sometimes "succeeds" at reading a malformed/encoding-mismatched CSV by
    // silently dropping every row and auto-naming the columns `column00`,
    // `column01`, … — yielding an empty table with > 1 columns. The bare
    // `col_count > 1` check used to accept that, masking the real failure.
    let columns = schema::describe_view(conn, table_name).unwrap_or_default();
    let col_count = columns.len() as i64;
    println!("Succeeded! Column count: {}", col_count);
    if col_count <= 1 {
        println!("Rejected (column count <= 1)");
        let _ = conn.execute(&drop_sql, []);
        return Ok(false);
    }
    // Reject candidates whose headers are entirely DuckDB auto-generated
    // positional names (`columnNN`). This is the signature of an
    // encoding-mismatch / dropped-header read; later strategies (e.g. GBK)
    // stand a real chance once this one stops short-circuiting.
    if looks_auto_generated(&columns) {
        println!("Rejected (all headers are auto-generated columnNN)");
        let _ = conn.execute(&drop_sql, []);
        return Ok(false);
    }
    Ok(true)
}

/// True when every non-empty column name matches DuckDB's auto-generated
/// positional placeholder `column<digits>` (e.g. `column00`). A table whose
/// headers are ALL such names was almost certainly produced by a read that
/// failed to parse the header row (encoding mismatch, malformed CSV) rather
/// than a genuine headerless file — genuine files still get materialized via
/// the unconditional final fallback.
fn looks_auto_generated(columns: &[ColumnInfo]) -> bool {
    let mut named = 0usize;
    let mut auto = 0usize;
    for col in columns {
        let name = col.name.trim();
        if name.is_empty() {
            continue;
        }
        named += 1;
        if is_auto_column_name(name) {
            auto += 1;
        }
    }
    // Need at least one header to judge, and every header must be auto-generated.
    named > 0 && auto == named
}

/// Matches DuckDB's positional placeholder pattern `column` followed by one or
/// more digits, e.g. `column00`, `column13`.
fn is_auto_column_name(name: &str) -> bool {
    let rest = match name.strip_prefix("column") {
        Some(r) => r,
        None => return false,
    };
    !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit())
}

/// Multi-strategy CSV loader
fn load_csv_as_table(conn: &Connection, table_name: &str, file_path: &str, _progress: Option<&dyn Fn(&str)>) -> AppResult<()> {
    let drop_sql = format!("DROP TABLE IF EXISTS \"{}\";", table_name);
    let escaped_path = file_path.replace('\'', "''");

    // Strategy 0: detect non-UTF-8 encoding up front (the common Chinese
    // Windows case is GBK/GB18030 CSVs exported by Excel/WPS, which have no
    // BOM). DuckDB's `read_csv_auto` decodes as UTF-8 by default, and with
    // `ignore_errors=true` (used by the strategies below) it *silently* drops
    // every GBK byte and auto-names columns `column00…`, producing an empty
    // table that still has > 1 columns — so it would wrongly "succeed".
    // Trying GBK first avoids that trap entirely for the dominant case.
    if looks_like_non_utf8(file_path) && try_gbk_strategies(conn, table_name, &escaped_path)? {
        return Ok(());
    }

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
    if try_gbk_strategies(conn, table_name, &escaped_path)? {
        return Ok(());
    }

    // Fallback: accept best-effort full scan
    let _ = conn.execute(&drop_sql, []);
    let create_sql = format!("CREATE TABLE \"{table_name}\" AS SELECT * FROM {full_scan};");
    conn.execute(&create_sql, [])?;
    Ok(())
}

/// Try loading a CSV as GBK via DuckDB's `encodings` extension. Returns true if
/// any candidate produced a valid (non-auto-generated-header) table. Loads the
/// extension on demand; a missing extension simply yields false.
fn try_gbk_strategies(conn: &Connection, table_name: &str, escaped_path: &str) -> AppResult<bool> {
    let _ = conn.execute("INSTALL encodings;", []);
    if conn.execute("LOAD encodings;", []).is_err() {
        return Ok(false);
    }
    // all_varchar avoids type-sniffing failures on mixed GBK numeric/text cols.
    let gbk_source = format!(
        "read_csv_auto('{escaped_path}', header = true, encoding = 'zh_CN.gbk', all_varchar = true)"
    );
    if try_create_and_validate(conn, table_name, &gbk_source)? {
        return Ok(true);
    }
    let gbk_robust = format!(
        "read_csv_auto('{escaped_path}', header = true, encoding = 'zh_CN.gbk', all_varchar = true, ignore_errors=true, null_padding=true)"
    );
    if try_create_and_validate(conn, table_name, &gbk_robust)? {
        return Ok(true);
    }
    Ok(false)
}

/// Sniff the file's first few KB and return true only when it contains bytes
/// that are *definitively* invalid UTF-8 — i.e. a hard decoding error
/// (`error_len().is_some()`), not just a truncated multi-byte sequence at the
/// chunk boundary (which we ignore to avoid false positives on a clean cut).
/// This is intentionally conservative: valid UTF-8 and pure-ASCII files always
/// return false. A positive result means the file is almost certainly GBK /
/// GB18030 (the Chinese Windows Excel/WPS export default).
fn looks_like_non_utf8(path: &str) -> bool {
    let mut file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return false,
    };
    let mut buf = [0u8; 8192];
    let n = match file.read(&mut buf) {
        Ok(n) => n,
        Err(_) => return false,
    };
    match std::str::from_utf8(&buf[..n]) {
        Ok(_) => false,
        // error_len == None means the only problem is a (possibly) incomplete
        // multi-byte sequence at the very end of our chunk — inconclusive, so
        // don't claim non-UTF-8.
        Err(e) => e.error_len().is_some(),
    }
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

/// Multi-strategy Excel loader. `sheet` selects a specific worksheet for
/// multi-sheet files; `None` leaves DuckDB to its default (first sheet).
fn load_xlsx_as_table(
    conn: &Connection,
    table_name: &str,
    file_path: &str,
    sheet: Option<&str>,
    progress: Option<&dyn Fn(&str)>,
) -> AppResult<()> {
    let drop_sql = format!("DROP TABLE IF EXISTS \"{}\";", table_name);
    let escaped_path = file_path.replace('\'', "''");
    // Sheet clause injected into every strategy's `read_xlsx(...)` call.
    let sheet_clause = match sheet {
        Some(s) => format!(", sheet='{}'", s.replace('\'', "''")),
        None => String::new(),
    };

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
    try_strategy!(1, "默认读取", format!("read_xlsx('{escaped_path}'{sheet_clause})"));

    // Strategy 2-6: Try different header offsets (common in exported reports)
    let offsets = ["A1:ZZ100000", "A2:ZZ100000", "A3:ZZ100000", "A4:ZZ100000", "A5:ZZ100000"];
    for (i, r) in offsets.iter().enumerate() {
        let label = format!("表头偏移{}", r.split(':').next().unwrap_or(r));
        try_strategy!(i + 2, label, format!(
            "read_xlsx('{escaped_path}', header=true, range='{r}', stop_at_empty=false, ignore_errors=true{sheet_clause})"
        ));
    }

    // Strategy 7: stop_at_empty=false + header + ignore_errors
    try_strategy!(7, "忽略错误全扫描", format!(
        "read_xlsx('{escaped_path}', header=true, stop_at_empty=false, ignore_errors=true{sheet_clause})"
    ));

    // Strategy 8: all_varchar + stop_at_empty=false + ignore_errors
    try_strategy!(8, "全文本类型", format!(
        "read_xlsx('{escaped_path}', header=true, stop_at_empty=false, all_varchar=true, ignore_errors=true{sheet_clause})"
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
        "read_xlsx('{escaped_path}', header=true, stop_at_empty=false, all_varchar=true, ignore_errors=true{sheet_clause})"
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ColumnInfo;

    fn col(name: &str) -> ColumnInfo {
        ColumnInfo {
            name: name.to_string(),
            r#type: "VARCHAR".to_string(),
            null: true,
        }
    }

    #[test]
    fn auto_column_name_matches_duckdb_pattern() {
        assert!(is_auto_column_name("column0"));
        assert!(is_auto_column_name("column00"));
        assert!(is_auto_column_name("column13"));
    }

    #[test]
    fn auto_column_name_rejects_non_patterns() {
        assert!(!is_auto_column_name("column")); // no digits
        assert!(!is_auto_column_name("columnA"));
        assert!(!is_auto_column_name("订单编号"));
        assert!(!is_auto_column_name("uuid"));
        assert!(!is_auto_column_name("Column0")); // case-sensitive
    }

    #[test]
    fn looks_auto_generated_true_for_all_positional() {
        let cols = vec![col("column00"), col("column01"), col("column13")];
        assert!(looks_auto_generated(&cols));
    }

    #[test]
    fn looks_auto_generated_false_for_real_headers() {
        let cols = vec![col("uuid"), col("订单编号"), col("支付时间")];
        assert!(!looks_auto_generated(&cols));
    }

    #[test]
    fn looks_auto_generated_false_for_mixed_names() {
        // Even one real header means it wasn't a fully-failed read.
        let cols = vec![col("column00"), col("订单编号"), col("column02")];
        assert!(!looks_auto_generated(&cols));
    }

    #[test]
    fn looks_auto_generated_false_for_empty_input() {
        assert!(!looks_auto_generated(&[]));
    }

    #[test]
    fn looks_like_non_utf8_detects_gbk() {
        // "订单" in GBK = b6 a9 b5 a5. Write a temp file and sniff it.
        let dir = tempfile_dir();
        let path = dir.join("gbk.csv");
        // uuid,<订单 in GBK>\n1,<bytes>\n
        let bytes: Vec<u8> = vec![
            b'u', b'u', b'i', b'd', b',', 0xb6, 0xa9, 0xb5, 0xa5, b'\n', b'1', b',', 0xb6,
            0xa9, 0xb5, 0xa5, b'\n',
        ];
        std::fs::write(&path, &bytes).unwrap();
        assert!(looks_like_non_utf8(path.to_str().unwrap()));
    }

    #[test]
    fn looks_like_non_utf8_false_for_utf8_and_ascii() {
        let dir = tempfile_dir();
        let utf8 = dir.join("utf8.csv");
        std::fs::write(&utf8, "uuid,订单编号\n1,abc\n").unwrap();
        assert!(!looks_like_non_utf8(utf8.to_str().unwrap()));

        let ascii = dir.join("ascii.csv");
        std::fs::write(&ascii, "uuid,name\n1,abc\n").unwrap();
        assert!(!looks_like_non_utf8(ascii.to_str().unwrap()));
    }

    /// Minimal temp dir for these tests (the codebase has no `tempfile` crate).
    /// Auto-cleans on drop.
    struct TmpDir(std::path::PathBuf);
    impl Drop for TmpDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    impl TmpDir {
        fn join(&self, name: &str) -> std::path::PathBuf {
            self.0.join(name)
        }
    }
    fn tempfile_dir() -> TmpDir {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "lakemind_reg_test_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&p).unwrap();
        TmpDir(p)
    }

    /// End-to-end: a GBK-encoded CSV must ingest with its real Chinese headers,
    /// not DuckDB's auto-generated `columnNN` placeholders. Requires the DuckDB
    /// `encodings` extension; skipped gracefully when it isn't loadable.
    #[test]
    fn gbk_csv_ingests_with_real_headers() {
        let dir = tempfile_dir();
        let path = dir.join("gmv.csv");
        // Header: uuid,订单编号,支付金额  then 2 data rows, all GBK-encoded.
        let header = b"uuid";
        let row = |fields: &[&[u8]]| -> Vec<u8> {
            let mut v = Vec::new();
            for (i, f) in fields.iter().enumerate() {
                if i > 0 {
                    v.push(b',');
                }
                v.extend_from_slice(f);
            }
            v.push(b'\n');
            v
        };
        // 订单编号 GBK = b6 a9 b5 a5 b1 e0 ba c5 ; 支付金额 GBK = d6 a7 b8 b6 bd f0 b6 ee
        let dingdan: &[u8] = &[0xb6, 0xa9, 0xb5, 0xa5, 0xb1, 0xe0, 0xba, 0xc5];
        let zhifu: &[u8] = &[0xd6, 0xa7, 0xb8, 0xb6, 0xbd, 0xf0, 0xb6, 0xee];
        let mut content = row(&[header, dingdan, zhifu]);
        content.extend(row(&[b"1", dingdan, b"100"]));
        content.extend(row(&[b"2", dingdan, b"200"]));
        std::fs::write(&path, &content).unwrap();

        let conn = duckdb::Connection::open_in_memory().unwrap();
        // Gate on extension availability so `cargo test` stays green everywhere.
        if conn.execute("LOAD encodings;", []).is_err() {
            eprintln!("[skip] DuckDB `encodings` extension unavailable — GBK test skipped");
            return;
        }
        let entry = ScanEntry {
            label: "gmv".into(),
            view_name: "s_gmv".into(),
            kind: SourceKind::Csv,
            path: path.to_string_lossy().to_string(),
            scan_path: path.to_string_lossy().to_string(),
            partition_keys: Vec::new(),
            file_size: content.len() as u64,
            mtime: 0,
            sheet: None,
        };
        let table = register(&conn, &entry, StorageKind::Table, None).unwrap();
        let names: Vec<&str> = table.columns.iter().map(|c| c.name.as_str()).collect();
        assert_ne!(
            names,
            vec!["column00", "column01", "column02"],
            "headers must not be auto-generated"
        );
        assert!(
            names.contains(&"订单编号") || names.iter().any(|n| n.contains("订")),
            "expected the real GBK-decoded header, got {names:?}"
        );
        // And the data rows must have survived (not silently dropped).
        let count: i64 = conn
            .query_row("SELECT count(*) FROM \"s_gmv\"", [], |r| r.get(0))
            .unwrap();
        assert!(count >= 2, "expected >=2 rows, got {count}");
    }
}
