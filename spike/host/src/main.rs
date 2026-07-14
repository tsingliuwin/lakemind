//! Minimal spike host for the dbx JDBC sidecar (C4: connectivity).
//!
//! Loads `spike/odps-spike.env`, spawns the dbx jdbc sidecar, speaks the
//! line-delimited JSON-RPC protocol, then:
//!   1. `connect`   — connect to MaxCompute via `com.aliyun.odps:odps-jdbc`
//!   2. `listTables`— prove we can enumerate the project's tables
//!   3. `executeQueryPage` + `fetchQueryPage` — pull 5 sample rows + schema
//!
//! SECURITY: credentials are read from the env file at runtime and are NEVER
//! printed. Only non-secret fields (endpoint, project, table names, schema,
//! row counts) appear in output.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::time::Instant;
use serde_json::{json, Value};

// ───────────────────────── env file ─────────────────────────

fn load_env(path: &str) -> HashMap<String, String> {
    let mut m = HashMap::new();
    let txt = std::fs::read_to_string(path).unwrap_or_default();
    for line in txt.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            m.insert(k.trim().to_string(), v.trim().to_string());
        }
    }
    m
}

fn get(env: &HashMap<String, String>, k: &str) -> String {
    env.get(k).cloned().unwrap_or_default()
}

// ─────────────────────── driver jar discovery ───────────────

/// Recursively collect every `*.jar` under `root` (the dbx maven cache dir).
fn collect_jars(root: &PathBuf) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![root.clone()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().and_then(|s| s.to_str()) == Some("jar") {
                if let Some(s) = p.to_str() {
                    out.push(s.to_string());
                }
            }
        }
    }
    out.sort();
    out
}

use std::path::PathBuf;

// ─────────────────────────── sidecar ────────────────────────

struct Sidecar {
    stdin: std::process::ChildStdin,
    stdout: BufReader<std::process::ChildStdout>,
    next_id: u64,
}

impl Sidecar {
    fn spawn(bin: &str) -> std::io::Result<Self> {
        let mut child = Command::new(bin)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()?;
        Ok(Self {
            stdin: child.stdin.take().unwrap(),
            stdout: BufReader::new(child.stdout.take().unwrap()),
            next_id: 1,
        })
    }

    /// Send a JSON-RPC request and read stdout lines until the matching `id`.
    /// Non-matching lines (handshake/log) are skipped.
    fn call(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        let req = json!({ "id": id, "method": method, "params": params });
        let line = serde_json::to_string(&req).unwrap();
        let _ = self.stdin.write_all(format!("{line}\n").as_bytes());
        let _ = self.stdin.flush();

        let mut buf = String::new();
        loop {
            buf.clear();
            let n = self.stdout.read_line(&mut buf).unwrap_or(0);
            if n == 0 {
                return json!({ "error": { "message": "sidecar closed stdout (EOF)" } });
            }
            let trimmed = buf.trim();
            if trimmed.is_empty() {
                continue;
            }
            if let Ok(v) = serde_json::from_str::<Value>(trimmed) {
                if v.get("id").and_then(|i| i.as_u64()) == Some(id) {
                    return v;
                }
                // not our response — log to stderr and keep reading
                eprintln!("[sidecar-extra] {trimmed}");
            } else {
                eprintln!("[sidecar-nonjson] {trimmed}");
            }
        }
    }

    fn close(&mut self) {
        // Closing stdin ends the sidecar loop (EOF).
        let _ = self.stdin.flush();
    }
}

fn ok(resp: &Value) -> bool {
    resp.get("result").is_some() && resp.get("error").is_none()
}

fn err_msg(resp: &Value) -> String {
    resp.get("error")
        .and_then(|e| e.get("message"))
        .and_then(|m| m.as_str())
        .unwrap_or("(no error message)")
        .to_string()
}

// ───────────────────────────── main ─────────────────────────

fn main() {
    let repo_root = std::env::var("SPIKE_REPO_ROOT")
        .unwrap_or_else(|_| ".".to_string());
    let env_path = format!("{repo_root}/spike/odps-spike.env");
    let env = load_env(&env_path);
    if env.is_empty() {
        eprintln!("!! env file not found or empty: {env_path}");
        std::process::exit(2);
    }

    // Allow shell overrides for endpoint/tunnel so we can iterate on the right
    // region without editing the env file. SPIKE_TUNNEL="" explicitly omits the
    // tunnel param (lets the driver auto-resolve from the main endpoint).
    let endpoint = std::env::var("SPIKE_ENDPOINT")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| get(&env, "ODPS_ENDPOINT").trim_end_matches('/').to_string());
    let tunnel = match std::env::var("SPIKE_TUNNEL") {
        Ok(v) => v, // explicit value (possibly empty = omit)
        Err(_) => get(&env, "ODPS_TUNNEL_ENDPOINT"),
    };
    let project = get(&env, "ODPS_PROJECT");
    // Allow overriding the sample-table from the shell (e.g. point at the
    // large table when ODPS_TABLE doesn't resolve). Falls back to ODPS_TABLE.
    let table = std::env::var("SPIKE_SAMPLE_TABLE")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| get(&env, "ODPS_TABLE"));
    let region = get(&env, "ODPS_REGION");

    // credentials — used but never printed
    let ak_id = get(&env, "ODPS_ACCESS_KEY_ID");
    let ak_secret = get(&env, "ODPS_ACCESS_KEY_SECRET");
    if ak_id.is_empty() || ak_secret.is_empty() || project.is_empty() || endpoint.is_empty() {
        eprintln!("!! missing required env field (ODPS_ENDPOINT/PROJECT/ACCESS_KEY_ID/SECRET)");
        std::process::exit(2);
    }

    // sidecar + driver paths
    let sidecar_bin = format!("{repo_root}/spike/dbx-jdbc-plugin/dbx-jdbc-plugin-0.1.21/bin/dbx-jdbc-plugin");
    let maven_cache = PathBuf::from(
        std::env::var("DBX_MAVEN_CACHE").unwrap_or_else(|_| {
            format!("{}/.dbx/maven", std::env::var("HOME").unwrap_or_default())
        }),
    );
    let driver_jars = collect_jars(&maven_cache);
    eprintln!("[setup] sidecar bin : {sidecar_bin}");
    eprintln!("[setup] driver jars : {} jars under {}", driver_jars.len(), maven_cache.display());
    eprintln!("[setup] region      : {region}");
    eprintln!("[setup] project      : {project}");
    eprintln!("[setup] endpoint    : {endpoint}");
    eprintln!("[setup] credentials  : AK id={} chars, SK={} chars (not printed)",
        ak_id.len(), ak_secret.len());
    if !table.is_empty() { eprintln!("[setup] test table  : {table}"); }

    // Build the MaxCompute JDBC URL: jdbc:odps:<endpoint>?project=<project>[&tunnelEndpoint=..]
    let mut url = format!("jdbc:odps:{endpoint}?project={project}");
    if !tunnel.is_empty() {
        url.push_str(&format!("&tunnelEndpoint={tunnel}"));
    }
    // Optional extra JDBC URL params for tuning (e.g. fetchResultSplitSize).
    if let Ok(extra) = std::env::var("SPIKE_URL_PARAMS") {
        if !extra.is_empty() {
            url.push_str(&format!("&{extra}"));
        }
    }
    eprintln!("[setup] jdbc url     : {url}");

    let mut sc = match Sidecar::spawn(&sidecar_bin) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("!! failed to spawn sidecar: {e}");
            std::process::exit(3);
        }
    };

    // The dbx sidecar caches the JDBC Connection by connectionKey, but each
    // DB-touching call must still carry the `connection` object so the sidecar
    // can look up / establish the cached connection. Build it once, reuse.
    let conn = json!({
        "connection_string": url,
        "username": ak_id,
        "password": ak_secret,
        "jdbc_driver_paths": driver_jars,
        "jdbc_driver_class": "com.aliyun.odps.jdbc.OdpsDriver",
        "connect_timeout_secs": 60
    });

    // ── C4.1 connect ──
    eprintln!("\n[C4.1] calling connect …");
    let resp = sc.call("connect", json!({ "connection": conn }));
    if !ok(&resp) {
        eprintln!("[C4.1] FAIL connect: {}", err_msg(&resp));
        println!("{{\"c4_connect\": false, \"error\": {:?}}}", err_msg(&resp));
        sc.close();
        std::process::exit(1);
    }
    eprintln!("[C4.1] OK  connect succeeded (result: {})", resp["result"]);
    println!("{{\"c4_connect\": true}}");

    // ── C4.2 listTables ──
    eprintln!("\n[C4.2] calling listTables …");
    let resp = sc.call("listTables", json!({ "connection": conn, "database": project, "limit": 2000 }));
    if !ok(&resp) {
        eprintln!("[C4.2] FAIL listTables: {}", err_msg(&resp));
        println!("{{\"c4_listTables\": false, \"error\": {:?}}}", err_msg(&resp));
        sc.close();
        std::process::exit(1);
    }
    let tables = resp["result"].clone();
    let n = tables.as_array().map(|a| a.len()).unwrap_or(0);
    eprintln!("[C4.2] OK  listTables returned {n} entries");
    // Dump ALL table names to spike/tables.txt for searching, and print a sample.
    if let Some(arr) = tables.as_array() {
        let names: Vec<String> = arr.iter().filter_map(|t| {
            t.as_str().or_else(|| t.get("name").and_then(|n| n.as_str())).map(|s| s.to_string())
        }).collect();
        let _ = std::fs::write(format!("{repo_root}/spike/tables.txt"),
            names.join("\n"));
        let sample: Vec<&str> = names.iter().take(20).map(|s| s.as_str()).collect();
        println!("{{\"c4_listTables\": true, \"count\": {n}, \"dumped_to\": \"spike/tables.txt\", \"sample\": {:?}}}", sample);
    } else {
        println!("{{\"c4_listTables\": true, \"result\": {}}}", tables);
    }

    // ── C6 sample + count + declared types ──
    if !table.is_empty() {
        let bare_table = table.rsplit('.').next().unwrap_or(table.as_str());

        // sample 5 rows. Small results come back INLINE (has_more=false, rows in
        // `rows`, session_id null). Only large results need session_id + fetchQueryPage.
        eprintln!("\n[C6] sample: SELECT * FROM {table} LIMIT 5");
        let resp = sc.call("executeQueryPage", json!({
            "connection": conn,
            "sql": format!("SELECT * FROM {table} LIMIT 5"),
            "pageSize": 5, "maxRows": 5, "fetchSize": 100
        }));
        if ok(&resp) {
            let raw = &resp["result"];
            let cols = raw["columns"].clone();
            let mut all_rows = raw["rows"].clone();
            let has_more = raw["has_more"].as_bool().unwrap_or(false);
            let session_id = raw["session_id"].as_str().unwrap_or("");
            if has_more && !session_id.is_empty() {
                let mut acc = all_rows.as_array().cloned().unwrap_or_default();
                loop {
                    let fr = sc.call("fetchQueryPage",
                        json!({ "connection": conn, "sessionId": session_id, "pageSize": 5 }));
                    if !ok(&fr) { break; }
                    let r = &fr["result"];
                    if let Some(arr) = r["rows"].as_array() { acc.extend(arr.iter().cloned()); }
                    if !r["has_more"].as_bool().unwrap_or(false) { break; }
                }
                let _ = sc.call("closeQuerySession", json!({ "sessionId": session_id }));
                all_rows = Value::Array(acc);
            }
            println!("{{\"c6_sample\": true, \"columns\": {}, \"rows\": {}}}", cols, all_rows);
        } else {
            println!("{{\"c6_sample\": false, \"error\": {:?}}}", err_msg(&resp));
        }

        // row count — sizes the table for the C5 throughput test
        eprintln!("\n[C6] count: SELECT count(*) FROM {table}");
        let resp = sc.call("executeQueryPage", json!({
            "connection": conn,
            "sql": format!("SELECT count(*) AS c FROM {table}"),
            "pageSize": 1, "maxRows": 1
        }));
        if ok(&resp) {
            let cnt = resp["result"]["rows"][0][0].clone();
            println!("{{\"c6_count\": {}}}", cnt);
        } else {
            println!("{{\"c6_count\": false, \"error\": {:?}}}", err_msg(&resp));
        }

        // declared columns/types via JDBC DatabaseMetaData.getColumns
        eprintln!("\n[C6] getColumns({bare_table})");
        let resp = sc.call("getColumns", json!({
            "connection": conn, "database": project, "schema": null, "table": bare_table
        }));
        println!("{{\"c6_getColumns\": {}}}", resp["result"]);
    }

    // ── C5 throughput: pull SPIKE_PULL_ROWS rows, measure rows/sec (JSON path) ──
    let pull: u64 = std::env::var("SPIKE_PULL_ROWS").ok().and_then(|s| s.parse().ok()).unwrap_or(0);
    if pull > 0 && !table.is_empty() {
        let page_size: u64 = std::env::var("SPIKE_PAGE_SIZE")
            .ok().and_then(|s| s.parse().ok()).unwrap_or(10000);
        let pull_sql = std::env::var("SPIKE_PULL_SQL")
            .ok().filter(|s| !s.is_empty())
            .unwrap_or_else(|| format!("SELECT * FROM {table}"));
        eprintln!("\n[C5] pulling {pull} rows from {table} (page_size={page_size}) sql: {pull_sql} …");
        let t0 = Instant::now();
        let resp = sc.call("executeQueryPage", json!({
            "connection": conn,
            "sql": pull_sql,
            "pageSize": page_size, "maxRows": pull, "fetchSize": page_size
        }));
        if !ok(&resp) {
            println!("{{\"c5_throughput\": false, \"error\": {:?}}}", err_msg(&resp));
        } else {
            let raw = &resp["result"];
            let mut rows: u64 = raw["rows"].as_array().map(|a| a.len() as u64).unwrap_or(0);
            let has_more = raw["has_more"].as_bool().unwrap_or(false);
            let session_id = raw["session_id"].as_str().unwrap_or("").to_string();
            eprintln!("[C5] first page: {rows} rows, has_more={has_more}, session={session_id}");
            let mut next_log: u64 = 100_000;
            if has_more && !session_id.is_empty() {
                loop {
                    let fr = sc.call("fetchQueryPage", json!({
                        "connection": conn, "sessionId": session_id, "pageSize": page_size
                    }));
                    if !ok(&fr) {
                        eprintln!("[C5] fetchQueryPage FAIL: {}", err_msg(&fr));
                        break;
                    }
                    let r = &fr["result"];
                    if let Some(arr) = r["rows"].as_array() { rows += arr.len() as u64; }
                    if rows >= next_log { eprintln!("[C5]   …{rows} rows"); next_log = rows + 100_000; }
                    if !r["has_more"].as_bool().unwrap_or(false) { break; }
                }
                let _ = sc.call("closeQuerySession", json!({ "sessionId": session_id }));
            }
            let elapsed = t0.elapsed().as_secs_f64();
            let rps = if elapsed > 0.0 { rows as f64 / elapsed } else { 0.0 };
            eprintln!("[C5] done: {rows} rows in {elapsed:.2}s → {rps:.0} rows/sec");
            println!("{{\"c5_throughput\": true, \"rows\": {rows}, \"elapsed_secs\": {elapsed:.3}, \"rows_per_sec\": {rps:.0}, \"page_size\": {page_size}}}");
        }
    }

    sc.close();
    eprintln!("\n[done] spike host finished.");
}
