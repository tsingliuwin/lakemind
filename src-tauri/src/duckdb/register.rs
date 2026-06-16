//! SOURCE registration: turn a [`ScanEntry`] into a DuckDB table.
//!
//! This handles file-to-table conversion with multi-strategy loading for robustness.

use duckdb::Connection;

use crate::duckdb::scan::ScanEntry;
use crate::duckdb::schema;
use crate::error::AppResult;
use crate::model::SourceTable;

/// Create the table for one scan entry and return a fully-populated
/// `SourceTable` (with columns + row-count estimate already filled in).
pub fn register(conn: &Connection, e: &ScanEntry) -> AppResult<SourceTable> {
    match e.kind {
        crate::model::SourceKind::Csv => {
            load_csv_as_table(conn, &e.view_name, &e.scan_path)?;
        }
        crate::model::SourceKind::Excel => {
            load_xlsx_as_table(conn, &e.view_name, &e.scan_path)?;
        }
        _ => {
            let drop_sql = format!("DROP TABLE IF EXISTS \"{}\";", e.view_name);
            conn.execute(&drop_sql, [])?;
            let sql = build_create_table_sql(e);
            conn.execute(&sql, [])?;
        }
    }

    let columns = schema::describe_view(conn, &e.view_name)?;
    let row_count_estimate = schema::estimate_row_count(conn, e).unwrap_or(None);

    Ok(SourceTable {
        name: e.view_name.clone(),
        label: e.label.clone(),
        kind: e.kind.clone(),
        path: e.path.clone(),
        scan_path: e.scan_path.clone(),
        partition_keys: e.partition_keys.clone(),
        row_count_estimate,
        columns,
    })
}

/// Helper function to create a table and check if it has a valid schema (at least 2 columns).
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
fn load_csv_as_table(conn: &Connection, table_name: &str, file_path: &str) -> AppResult<()> {
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
fn load_xlsx_as_table(conn: &Connection, table_name: &str, file_path: &str) -> AppResult<()> {
    let drop_sql = format!("DROP TABLE IF EXISTS \"{}\";", table_name);
    let escaped_path = file_path.replace('\'', "''");

    let mut candidates = Vec::new();

    // Strategy 1: Default load
    let default_source = format!("read_xlsx('{escaped_path}')");
    if let Some(c) = evaluate_candidate(conn, table_name, &default_source)? {
        candidates.push(c);
    }

    // Strategy 2: Try different header offsets (common in exported reports) with ignore_errors
    let offsets = ["A1:ZZ100000", "A2:ZZ100000", "A3:ZZ100000", "A4:ZZ100000", "A5:ZZ100000"];
    for r in offsets {
        let source = format!(
            "read_xlsx('{escaped_path}', header=true, range='{r}', stop_at_empty=false, ignore_errors=true)"
        );
        if let Some(c) = evaluate_candidate(conn, table_name, &source)? {
            candidates.push(c);
        }
    }

    // Strategy 3: stop_at_empty=false + header + ignore_errors
    let robust_source = format!(
        "read_xlsx('{escaped_path}', header=true, stop_at_empty=false, ignore_errors=true)"
    );
    if let Some(c) = evaluate_candidate(conn, table_name, &robust_source)? {
        candidates.push(c);
    }

    // Strategy 4: all_varchar + stop_at_empty=false + ignore_errors
    let varchar_source = format!(
        "read_xlsx('{escaped_path}', header=true, stop_at_empty=false, all_varchar=true, ignore_errors=true)"
    );
    if let Some(c) = evaluate_candidate(conn, table_name, &varchar_source)? {
        candidates.push(c);
    }

    if !candidates.is_empty() {
        // Pick the candidate with the highest header_score.
        // If header_scores are equal, pick the one with fewer columns (cleaner range).
        candidates.sort_by(|a, b| {
            b.header_score.cmp(&a.header_score)
                .then_with(|| a.col_count.cmp(&b.col_count))
        });

        let best = &candidates[0];
        println!("Selected best Excel ingestion strategy: {} (score: {}, cols: {})", best.source_fn, best.header_score, best.col_count);
        let create_sql = format!("CREATE TABLE \"{table_name}\" AS SELECT * FROM {};", best.source_fn);
        conn.execute(&create_sql, [])?;
        return Ok(());
    }

    // Fallback: accept best-effort result using Strategy 4 source
    let _ = conn.execute(&drop_sql, []);
    let create_sql = format!("CREATE TABLE \"{table_name}\" AS SELECT * FROM {varchar_source};");
    conn.execute(&create_sql, [])?;
    Ok(())
}

/// Compose the `CREATE TABLE` statement for other entries (Parquet, JSON, Delta).
fn build_create_table_sql(e: &ScanEntry) -> String {
    let scan = e.scan_path.replace('\'', "''");
    let partition_clause = if e.partition_keys.is_empty() {
        String::new()
    } else {
        format!(", hive_partitioning = 1")
    };
    let inner = match e.kind {
        crate::model::SourceKind::Parquet => {
            format!("read_parquet('{scan}'{partition_clause})")
        }
        crate::model::SourceKind::Csv => {
            format!("read_csv_auto('{scan}', header = true)")
        }
        crate::model::SourceKind::Json => {
            format!("read_json_auto('{scan}')")
        }
        crate::model::SourceKind::Excel => {
            format!("read_xlsx('{scan}')")
        }
        crate::model::SourceKind::Delta => {
            format!("delta('{scan}')")
        }
        crate::model::SourceKind::Table | crate::model::SourceKind::View => {
            unreachable!("Table/View sources are not registered from raw paths")
        }
    };
    format!("CREATE TABLE \"{}\" AS SELECT * FROM {};", e.view_name, inner)
}
