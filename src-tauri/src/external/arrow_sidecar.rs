//! MaxCompute bulk download via the Arrow sidecar: ODPS `TableTunnel` +
//! `ArrowTunnelRecordReader` → Arrow IPC stream → DuckDB `appender-arrow`.
//!
//! This is the MaxCompute-specific "fast lane": no instance-tunnel 10000-row
//! cap, no RecordPack binary decode, columnar zero-copy ingest. Validated in
//! the spike (~17k rows/sec single-session, ~64k at 10-way concurrency; see
//! `spike/REPORT.md`).

use std::io::BufReader;
use std::process::{Command, Stdio};
use std::time::Instant;

use arrow::datatypes::DataType;
use arrow::ipc::reader::StreamReader;
use serde::Serialize;

use crate::db::{DbConnectionRecord, MaxcomputeOpts};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PullStats {
    pub rows: u64,
    pub batches: u64,
    pub elapsed_secs: f64,
    pub rows_per_sec: f64,
}

/// JVM flags required by the shaded Arrow `MemoryUtil` (reflective access to
/// `java.nio.Buffer.address`) on JDK 17+, plus a generous direct-memory ceiling.
/// Windowed pulls + per-window `allocator.close()` keep direct memory bounded.
const ADD_OPENS: &[&str] = &[
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

/// Map an Arrow `DataType` to a DuckDB column type for `CREATE TABLE`.
/// Complex types (List/Struct/Map/FixedSizeList) fall back to VARCHAR for now
/// (the ODPS SDK's Arrow accessors cover them; full mapping is a follow-up).
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
        _ => "VARCHAR",
    }
}

/// Pull a MaxCompute table (or a `[start, start+count)` row window of it, or a
/// single partition of a partitioned table) through the Arrow sidecar and
/// ingest it into DuckDB as `local_table`.
///
/// - `table_ref`: fully-qualified `project.table` (the sidecar resolves the
///   session's default project from `ODPS_PROJECT`; a bare name also works).
/// - `sidecar_jar`: path to `arrow-maxcompute-sidecar.jar` (a bundled resource).
/// - `driver_jars`: resolved vendor JARs (odps-sdk-core etc.) — runtime classpath.
/// - `start` / `count`: row window; `count == 0` means "to end of table/partition".
/// - `partition_spec`: when `Some`, download only that partition (e.g.
///   `ds=20250701` or multi-level `ds=20250701/region=cn`). Required for
///   partitioned tables; ignored for non-partitioned tables.
///
/// AK/SK are passed via the child env (`ODPS_ACCESS_KEY_ID/SECRET`), never logged.
pub fn pull_table(
    duck_conn: &duckdb::Connection,
    rec: &DbConnectionRecord,
    opts: &MaxcomputeOpts,
    table_ref: &str,
    local_table: &str,
    sidecar_jar: &str,
    driver_jars: &[String],
    start: u64,
    count: u64,
    partition_spec: Option<&str>,
) -> Result<PullStats, String> {
    let java = crate::external::jdbc_sidecar::find_java_bin()
        .ok_or_else(|| "未找到 Java 运行时（MaxCompute 物化需要 JRE 17+）".to_string())?;

    // classpath = driver jars : sidecar jar
    let mut cp = driver_jars.join(":");
    if !cp.is_empty() {
        cp.push(':');
    }
    cp.push_str(sidecar_jar);

    let t0 = Instant::now();
    let mut cmd = Command::new(&java);
    cmd.args(ADD_OPENS)
        .arg("-cp")
        .arg(&cp)
        .arg("ArrowSidecar")
        .arg(table_ref)
        .arg(start.to_string())
        .arg(count.to_string());
    // Optional 4th arg: partition spec for partitioned tables.
    if let Some(ps) = partition_spec {
        if !ps.is_empty() {
            cmd.arg(ps);
        }
    }
    let mut child = cmd
        .env("ODPS_ENDPOINT", &opts.endpoint)
        .env("ODPS_ACCESS_KEY_ID", rec.username.as_str())
        .env("ODPS_ACCESS_KEY_SECRET", rec.password.as_str())
        .env("ODPS_PROJECT", &opts.project)
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| format!("启动 Arrow sidecar 失败: {e}"))?;

    let stdout = child.stdout.take().ok_or("arrow sidecar 无 stdout")?;
    let reader = BufReader::with_capacity(1 << 20, stdout);
    let mut stream = StreamReader::try_new(reader, None)
        .map_err(|e| format!("打开 Arrow IPC 流失败: {e}"))?;
    let schema = stream.schema().clone();

    // CREATE TABLE IF NOT EXISTS from the Arrow schema. For a fresh pull this
    // creates the table; for subsequent partition/window pulls into the same
    // local table, the IF NOT EXISTS makes it a no-op (the appender below
    // appends to the existing table). This lets the caller drive multiple
    // pull_table() calls into one local table without managing DDL itself.
    let cols: Vec<String> = schema
        .fields()
        .iter()
        .map(|f| format!("\"{}\" {}", f.name().replace('"', "\"\""), arrow_to_duckdb_type(f.data_type())))
        .collect();
    let create_sql = format!("CREATE TABLE IF NOT EXISTS \"{local_table}\" ({});", cols.join(", "));
    duck_conn
        .execute(&create_sql, [])
        .map_err(|e| format!("建本地表失败: {e}"))?;

    let mut app = duck_conn
        .appender(local_table)
        .map_err(|e| format!("打开 appender 失败: {e}"))?;
    let mut rows: u64 = 0;
    let mut batches: u64 = 0;
    let mut last_log = Instant::now();
    while let Some(batch) = stream.next() {
        let batch = batch.map_err(|e| format!("读取 Arrow batch 失败: {e}"))?;
        rows += batch.num_rows() as u64;
        batches += 1;
        app.append_record_batch(batch)
            .map_err(|e| format!("写入 DuckDB 失败: {e}"))?;
        // Log progress every 5s so the operator can see the pull is alive.
        if last_log.elapsed().as_secs() >= 5 {
            let elapsed = t0.elapsed().as_secs_f64();
            let rps = if elapsed > 0.0 { rows as f64 / elapsed } else { 0.0 };
            tracing::info!(
                category = "link",
                "maxcompute: pulling {table_ref} -> {local_table}: {rows} rows, {batches} batches, {elapsed:.1}s ({rps:.0} rows/s)"
            );
            last_log = Instant::now();
        }
    }
    drop(app);
    let _ = child.wait();
    let elapsed = t0.elapsed().as_secs_f64();
    let rps = if elapsed > 0.0 { rows as f64 / elapsed } else { 0.0 };
    Ok(PullStats { rows, batches, elapsed_secs: elapsed, rows_per_sec: rps })
}
