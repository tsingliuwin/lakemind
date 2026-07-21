//! End-to-end acceptance tests for the MaxCompute sidecar pipeline.
//!
//! These are `#[ignore]` by default — they hit the real MaxCompute service
//! and need credentials + network + a JRE. Run explicitly:
//!
//! ```sh
//! set -a && source spike/odps-spike.env && set +a
//! cargo test --lib external::tests::maxcompute_e2e -- --nocapture --ignored
//! ```
//!
//! Credentials are read from the environment (never hardcoded). The driver
//! JARs are read from the dbx maven cache (`~/.dbx/maven/`), populated by a
//! prior `dbx-maven-resolver` run. Sidecar binaries are resolved relative to
//! `CARGO_MANIFEST_DIR` (the dev layout: `src-tauri/sidecars/...`).

#![cfg(test)]

use std::path::PathBuf;

use super::arrow_sidecar;
use super::driver_resolver;
use super::jdbc_sidecar::{self, JdbcSidecar};
use crate::db::{DbConnectionRecord, MaxcomputeOpts};

/// Dev-layout sidecar paths (mirrors `SidecarPaths::resolve` but without an
/// `AppHandle`, so this runs outside the Tauri runtime). `CARGO_MANIFEST_DIR`
/// is `src-tauri/`, so sidecars live under `sidecars/...`.
fn dev_sidecar_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("sidecars")
}

fn dbx_launcher() -> String {
    let ext = if cfg!(windows) { ".bat" } else { "" };
    dev_sidecar_root()
        .join(format!("dbx-jdbc-plugin/bin/dbx-jdbc-plugin{ext}"))
        .to_str()
        .unwrap()
        .to_string()
}

fn arrow_jar() -> String {
    dev_sidecar_root()
        .join("arrow-maxcompute/arrow-maxcompute-sidecar.jar")
        .to_str()
        .unwrap()
        .to_string()
}

fn resolver_bin() -> String {
    let ext = if cfg!(windows) { ".bat" } else { "" };
    dev_sidecar_root()
        .join(format!("dbx-jdbc-plugin/bin/dbx-maven-resolver{ext}"))
        .to_str()
        .unwrap()
        .to_string()
}

/// Read the connection params from the environment (the spike env file is
/// `set -a`-sourced before the test run). Returns a fully-populated record +
/// parsed opts. AK_ID/AK_SECRET map to username/password.
fn record_from_env() -> Option<(DbConnectionRecord, MaxcomputeOpts)> {
    let endpoint = std::env::var("ODPS_ENDPOINT").ok()?;
    let project = std::env::var("ODPS_PROJECT").ok()?;
    let ak_id = std::env::var("ODPS_ACCESS_KEY_ID").ok()?;
    let ak_secret = std::env::var("ODPS_ACCESS_KEY_SECRET").ok()?;
    let tunnel = std::env::var("ODPS_TUNNEL_ENDPOINT").ok();
    let region = std::env::var("ODPS_REGION").ok();
    let opts = MaxcomputeOpts {
        endpoint,
        project: project.clone(),
        region,
        tunnel_endpoint: tunnel,
        driver_coord: "com.aliyun.odps:odps-jdbc:3.9.3".to_string(),
        concurrency: None,
    };
    let options_json = serde_json::to_string(&opts).ok()?;
    let rec = DbConnectionRecord {
        id: "e2e".into(),
        name: "e2e".into(),
        db_type: "maxcompute".into(),
        host: String::new(),
        port: 0,
        database_name: project,
        username: ak_id,
        password: ak_secret,
        ssl_mode: "disable".into(),
        created_at: 0,
        options: Some(options_json),
    };
    Some((rec, opts))
}

/// Resolve the driver JARs (cached under `~/.dbx/maven/` after the first
/// `dbx-maven-resolver` run). Falls back to globbing the cache dir.
fn driver_jars() -> Vec<String> {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    let cache = PathBuf::from(&home).join(".dbx/maven");
    driver_resolver::resolve_driver_jars(&resolver_bin(), "com.aliyun.odps:odps-jdbc:3.9.3", &cache)
        .unwrap_or_else(|_| driver_resolver::collect_jars(&cache))
}

/// Full end-to-end acceptance: test → list → count → arrow-pull a small window
/// → DuckDB row-count integrity → execute_query aggregate pushdown.
///
/// Pulls only the first 50 000 rows (a windowed pull, not the whole table) so
/// it finishes in seconds rather than minutes while still exercising every
/// stage of the production code path (`register_maxcompute_table` uses the same
/// `pull_table` with count=0 for the full table).
#[test]
#[ignore]
fn maxcompute_e2e() {
    let (rec, opts) = record_from_env().expect(
        "ODPS_ENDPOINT/ODPS_PROJECT/ODPS_ACCESS_KEY_ID/ODPS_ACCESS_KEY_SECRET must be set \
         (source spike/odps-spike.env)",
    );
    let table = std::env::var("ODPS_LARGE_TABLE")
        .unwrap_or_else(|_| format!("{}.dim_users_sc_track", opts.project));
    let jars = driver_jars();
    assert!(!jars.is_empty(), "driver JARs not resolved (run dbx-maven-resolver first)");

    // ── 1) test_connection ────────────────────────────────────────────────
    let mut sc = JdbcSidecar::spawn(&dbx_launcher()).expect("spawn dbx sidecar");
    let conn_obj = jdbc_sidecar::build_maxcompute_connection(&rec, &jars).expect("build conn");
    sc.test_connection(&conn_obj).expect("test_connection");
    println!("[e2e] ✓ test_connection ok");

    // ── 2) list_tables ────────────────────────────────────────────────────
    let tables = sc.list_tables(&conn_obj, &opts.project, 2000).expect("list_tables");
    println!("[e2e] ✓ list_tables returned {} tables", tables.len());
    assert!(tables.len() > 10, "expected real table list, got {}", tables.len());

    // ── 3) count via execute_query (instance-tunnel, 1-row result) ─────────
    let table_ref = rec.maxcompute_table_ref(&table);
    let count_sql = format!("SELECT count(*) AS c FROM {table_ref}");
    let (cols, rows) = sc.execute_query(&conn_obj, &count_sql, 1).expect("count query");
    let full_count = rows.get(0).and_then(|r| r.get(0)).and_then(|v| v.as_i64()).unwrap_or(0);
    println!("[e2e] ✓ count(*) = {full_count} (cols={cols:?})");
    assert!(full_count > 0, "table {table_ref} reported 0 rows");

    // ── 4) ad-hoc aggregate pushdown (proves execute_query beyond count) ───
    let agg_sql = format!("SELECT count(*) AS c, min(id) AS mn, max(id) AS mx FROM {table_ref}");
    let (_, agg_rows) = sc.execute_query(&conn_obj, &agg_sql, 1).expect("agg query");
    println!("[e2e] ✓ aggregate pushdown: {:?}", agg_rows.get(0));
    sc.close();

    // ── 5) Arrow pull → DuckDB (windowed: first 50k rows) ─────────────────
    // Arrow batches are ~1024-row chunks, so the sidecar returns whole batches
    // covering [0, window) — the pulled count is >= window (rounded up to a
    // batch boundary). The zero-loss invariant is pulled == DuckDB count.
    // Window size is overridable via ODPS_E2E_WINDOW (default 50k — fast CI
    // smoke; set to e.g. 2000000 to measure steady-state throughput).
    let window = std::env::var("ODPS_E2E_WINDOW")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(50_000)
        .min(full_count as u64);
    let duck = duckdb::Connection::open_in_memory().expect("open duckdb");
    let _ = duck.execute("DROP TABLE IF EXISTS e2e_pull;", []);
    let stats = arrow_sidecar::pull_table(
        &duck, &rec, &opts, &table_ref, "e2e_pull",
        &arrow_jar(), &jars, 0, window, None,
    )
    .expect("arrow pull");
    println!(
        "[e2e] ✓ arrow pull: {} rows (window={window}), {} batches, {:.1}s ({:.0} rows/s)",
        stats.rows, stats.batches, stats.elapsed_secs, stats.rows_per_sec
    );
    assert!(stats.rows >= window, "pulled {} < window {}", stats.rows, window);

    // ── 6) DuckDB row-count integrity (zero-loss check) ───────────────────
    let duck_count: i64 = duck
        .query_row("SELECT count(*) FROM e2e_pull", [], |r| r.get(0))
        .expect("count pulled");
    assert_eq!(duck_count as u64, stats.rows, "DuckDB row count != pulled count (data loss!)");
    println!("[e2e] ✓ DuckDB row-count integrity: {duck_count} == {} (zero loss)", stats.rows);

    // ── 7) local aggregate on the materialized window ─────────────────────
    let (local_min, local_max): (i64, i64) = duck
        .query_row("SELECT min(c1), max(c1) FROM e2e_pull", [], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
        })
        .expect("local agg");
    println!("[e2e] ✓ local aggregate on materialized window: min={local_min} max={local_max}");

    println!("\n[e2e] ALL STAGES PASSED — full sidecar pipeline verified end-to-end.");
}
