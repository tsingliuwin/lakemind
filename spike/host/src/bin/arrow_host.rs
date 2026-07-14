//! End-to-end Arrow spike: spawn the Java Arrow sidecar, read its stdout as an
//! Arrow IPC stream, ingest every RecordBatch into an in-memory DuckDB table
//! via the `appender-arrow` Appender, and measure end-to-end rows/sec
//! (download + IPC + ingest).
//!
//!   SPIKE_TABLE=yantubi.dim_users_sc_track  SPIKE_MAX_ROWS=2000000  arrow-host

use std::collections::HashMap;
use std::io::BufReader;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Instant;
use arrow::datatypes::DataType;
use arrow::ipc::reader::StreamReader;

fn load_env(path: &str) -> HashMap<String, String> {
    let mut m = HashMap::new();
    let txt = std::fs::read_to_string(path).unwrap_or_default();
    for line in txt.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') { continue; }
        if let Some((k, v)) = line.split_once('=') {
            m.insert(k.trim().to_string(), v.trim().to_string());
        }
    }
    m
}

fn collect_jars(root: &PathBuf) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![root.clone()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() { stack.push(p); }
            else if p.extension().and_then(|s| s.to_str()) == Some("jar") {
                if let Some(s) = p.to_str() { out.push(s.to_string()); }
            }
        }
    }
    out.sort();
    out
}

/// Map an Arrow DataType to a DuckDB column type for CREATE TABLE.
fn arrow_to_duckdb_type(t: &DataType) -> &'static str {
    match t {
        DataType::Boolean => "BOOLEAN",
        DataType::Int8 | DataType::Int16 | DataType::Int32 => "INTEGER",
        DataType::Int64 | DataType::UInt64 => "BIGINT",
        DataType::UInt8 | DataType::UInt16 | DataType::UInt32 => "INTEGER",
        DataType::Float16 | DataType::Float32 => "FLOAT",
        DataType::Float64 => "DOUBLE",
        DataType::Utf8 | DataType::LargeUtf8 => "VARCHAR",
        DataType::Binary | DataType::LargeBinary => "BLOB",
        DataType::Date32 | DataType::Date64 => "DATE",
        DataType::Timestamp(_, _) => "TIMESTAMP",
        DataType::Time32(_) | DataType::Time64(_) => "TIME",
        DataType::Decimal128(_, _) | DataType::Decimal256(_, _) => "DECIMAL",
        DataType::Duration(_) => "BIGINT",
        // complex (List/Struct/Map) — fall back to VARCHAR for the spike
        _ => "VARCHAR",
    }
}

fn main() {
    let repo_root = std::env::var("SPIKE_REPO_ROOT").unwrap_or_else(|_| ".".into());
    let env = load_env(&format!("{repo_root}/spike/odps-spike.env"));
    let endpoint = env.get("ODPS_ENDPOINT").cloned().unwrap_or_default()
        .trim_end_matches('/').to_string();
    let ak_id = env.get("ODPS_ACCESS_KEY_ID").cloned().unwrap_or_default();
    let ak_secret = env.get("ODPS_ACCESS_KEY_SECRET").cloned().unwrap_or_default();
    let table = std::env::var("SPIKE_TABLE")
        .unwrap_or_else(|_| env.get("ODPS_LARGE_TABLE").cloned().unwrap_or_default());
    let max_rows = std::env::var("SPIKE_MAX_ROWS").unwrap_or_else(|_| "2000000".into());
    if table.is_empty() || ak_id.is_empty() || ak_secret.is_empty() || endpoint.is_empty() {
        eprintln!("!! missing SPIKE_TABLE / ODPS creds in spike/odps-spike.env");
        std::process::exit(2);
    }

    let maven_cache = PathBuf::from(format!(
        "{}/.dbx/maven", std::env::var("HOME").unwrap_or_default()));
    let jars = collect_jars(&maven_cache);
    let classpath = format!("{}:{repo_root}/spike/arrow-sidecar", jars.join(":"));

    let add_opens: [&str; 11] = [
        "--add-opens=java.base/java.io=ALL-UNNAMED",
        "--add-opens=java.base/java.lang=ALL-UNNAMED",
        "--add-opens=java.base/java.lang.reflect=ALL-UNNAMED",
        "--add-opens=java.base/java.net=ALL-UNNAMED",
        "--add-opens=java.base/java.nio=ALL-UNNAMED",
        "--add-opens=java.base/java.nio.charset=ALL-UNNAMED",
        "--add-opens=java.base/java.util=ALL-UNNAMED",
        "--add-opens=java.base/java.util.concurrent=ALL-UNNAMED",
        "--add-opens=java.base/jdk.internal.misc=ALL-UNNAMED",
        "--add-opens=java.base/sun.nio.ch=ALL-UNNAMED",
        "-XX:MaxDirectMemorySize=8G",
    ];

    eprintln!("[arrow-host] table={table} max_rows={max_rows} jars={}", jars.len());
    let t0 = Instant::now();
    let mut child = Command::new("java")
        .args(add_opens.iter())
        .arg("-cp").arg(&classpath)
        .arg("ArrowSidecar").arg(&table).arg(&max_rows)
        .env("ODPS_ENDPOINT", &endpoint)
        .env("ODPS_ACCESS_KEY_ID", &ak_id)
        .env("ODPS_ACCESS_KEY_SECRET", &ak_secret)
        // deliberately NOT setting ODPS_TUNNEL_ENDPOINT / ODPS_REGION (wrong in .env)
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn java sidecar");

    let stdout = child.stdout.take().expect("no sidecar stdout");
    let reader = BufReader::with_capacity(1 << 20, stdout);
    let mut stream = StreamReader::try_new(reader, None).expect("arrow stream reader");

    let schema = stream.schema().clone();
    eprintln!("[arrow-host] arrow schema: {schema}");

    // Build CREATE TABLE from the Arrow schema (DuckDB infers nothing here).
    let cols: Vec<String> = schema.fields().iter().enumerate()
        .map(|(i, f)| format!("c{} {}", i + 1, arrow_to_duckdb_type(f.data_type())))
        .collect();
    let create_sql = format!("CREATE TABLE t ({});", cols.join(", "));

    let db = duckdb::Connection::open_in_memory().expect("open in-memory duckdb");
    db.execute(&create_sql, []).expect("create table");
    eprintln!("[arrow-host] {create_sql}");

    let mut app = db.appender("t").expect("appender");
    let mut rows: u64 = 0;
    let mut batches: u64 = 0;
    let mut next_log: u64 = 100_000;
    while let Some(batch) = stream.next() {
        let batch = batch.expect("arrow batch");
        rows += batch.num_rows() as u64;
        batches += 1;
        app.append_record_batch(batch).expect("append_record_batch");
        if rows >= next_log {
            eprintln!("[arrow-host]   …{rows} rows ({batches} batches, {:.0} rows/s)",
                rows as f64 / t0.elapsed().as_secs_f64().max(1e-9));
            next_log = rows + 100_000;
        }
    }
    drop(app);

    // wait for the sidecar to exit so download time is fully captured
    let _ = child.wait();
    let elapsed = t0.elapsed().as_secs_f64();
    let rps = if elapsed > 0.0 { rows as f64 / elapsed } else { 0.0 };

    let cnt: i64 = db.query_row("SELECT count(*) FROM t", [], |r| r.get(0))
        .expect("count");
    eprintln!("[arrow-host] DONE batches={batches} rows(arrow)={rows} rows(duckdb)={cnt} elapsed={elapsed:.2}s -> {rps:.0} rows/sec");
    println!("{{\"arrow_end_to_end\": true, \"rows\": {rows}, \"duckdb_rows\": {cnt}, \"batches\": {batches}, \"elapsed_secs\": {elapsed:.3}, \"rows_per_sec\": {rps:.0}}}");
}
