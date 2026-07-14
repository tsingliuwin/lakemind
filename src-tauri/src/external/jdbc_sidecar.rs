//! Client for the dbx JDBC sidecar (`plugins/jdbc` in t8y2/dbx).
//!
//! Protocol (line-delimited JSON-RPC over the sidecar's stdin/stdout):
//!   request  : `{"id": <n>, "method": <name>, "params": {..}}`
//!   response : `{"id": <n>, "result": <..>}` | `{"id": <n>, "error": {"message": <..>}}`
//! The sidecar caches the JDBC `Connection` by a key derived from the
//! `connection` object, so every DB-touching call must carry `connection`.
//!
//! This module is pure (no Tauri state): the caller resolves the sidecar binary
//! path (from `resource_dir()`), the driver JAR paths (from `driver_resolver`),
//! and the credentials (from a `DbConnectionRecord`) and hands them in. That
//! keeps it unit-testable.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use serde_json::{json, Value};

use crate::db::DbConnectionRecord;

/// A live dbx JDBC sidecar process + its stdio JSON-RPC channel.
pub struct JdbcSidecar {
    stdin: std::process::ChildStdin,
    stdout: BufReader<std::process::ChildStdout>,
    next_id: u64,
}

impl JdbcSidecar {
    /// Spawn the dbx sidecar launcher (`bin/dbx-jdbc-plugin`). The launcher
    /// itself locates a JRE and sets the required JVM `--add-opens` flags.
    pub fn spawn(sidecar_bin: &str) -> Result<Self, String> {
        let mut child = Command::new(sidecar_bin)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| format!("启动 JDBC sidecar 失败 ({sidecar_bin}): {e}"))?;
        Ok(Self {
            stdin: child.stdin.take().ok_or("no sidecar stdin")?,
            stdout: BufReader::new(child.stdout.take().ok_or("no sidecar stdout")?),
            next_id: 1,
        })
    }

    /// Send a JSON-RPC request and read stdout lines until the matching `id`.
    /// Non-matching lines (handshake/log) are skipped to stderr.
    pub fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        let req = json!({ "id": id, "method": method, "params": params });
        let line = serde_json::to_string(&req).map_err(|e| e.to_string())?;
        self.stdin
            .write_all(format!("{line}\n").as_bytes())
            .map_err(|e| format!("write sidecar stdin: {e}"))?;
        self.stdin.flush().ok();

        let mut buf = String::new();
        loop {
            buf.clear();
            let n = self.stdout.read_line(&mut buf).map_err(|e| format!("read sidecar stdout: {e}"))?;
            if n == 0 {
                return Err("JDBC sidecar 关闭了输出（EOF）".to_string());
            }
            let trimmed = buf.trim();
            if trimmed.is_empty() {
                continue;
            }
            let v: Value = match serde_json::from_str(trimmed) {
                Ok(v) => v,
                Err(_) => {
                    tracing::debug!(target: "jdbc_sidecar", "non-json line: {trimmed}");
                    continue;
                }
            };
            if v.get("id").and_then(|i| i.as_u64()) == Some(id) {
                return Ok(v);
            }
            tracing::debug!(target: "jdbc_sidecar", "extra line: {trimmed}");
        }
    }

    /// Like `call` but unwraps `result` / surfaces `error.message` as Err.
    fn call_ok(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let resp = self.call(method, params)?;
        if let Some(msg) = resp.get("error").and_then(|e| e.get("message")).and_then(|m| m.as_str()) {
            return Err(msg.to_string());
        }
        Ok(resp.get("result").cloned().unwrap_or(Value::Null))
    }

    /// Close stdin → the sidecar loop ends on EOF.
    pub fn close(mut self) {
        let _ = self.stdin.flush();
    }
}

// ───────────────────── connection object ─────────────────────

/// Build the dbx sidecar `connection` JSON for a MaxCompute record.
/// `username`/`password` hold AK_ID/AK_SECRET; endpoint/project/region/tunnel
/// come from `options`; `driver_jars` are the resolved vendor JAR paths.
pub fn build_maxcompute_connection(
    rec: &DbConnectionRecord,
    driver_jars: &[String],
) -> Result<Value, String> {
    let opts = rec.maxcompute_opts();
    if opts.endpoint.is_empty() || opts.project.is_empty() {
        return Err("MaxCompute 连接缺少 endpoint/project".to_string());
    }
    let endpoint = opts.endpoint.trim_end_matches('/');
    let mut url = format!("jdbc:odps:{endpoint}?project={}", opts.project);
    if let Some(t) = &opts.tunnel_endpoint {
        if !t.is_empty() {
            url.push_str(&format!("&tunnelEndpoint={t}"));
        }
    }
    Ok(json!({
        "connection_string": url,
        "username": rec.username,
        "password": rec.password,
        "jdbc_driver_paths": driver_jars,
        "jdbc_driver_class": "com.aliyun.odps.jdbc.OdpsDriver",
        "connect_timeout_secs": 60
    }))
}

// ───────────────────── high-level ops ─────────────────────

impl JdbcSidecar {
    /// Round-trip a `testConnection` (no state retained by the sidecar).
    pub fn test_connection(&mut self, conn: &Value) -> Result<(), String> {
        self.call_ok("testConnection", json!({ "connection": conn })).map(|_| ())
    }

    /// Enumerate tables in a database (project). Returns bare table names.
    pub fn list_tables(&mut self, conn: &Value, database: &str, limit: u64) -> Result<Vec<String>, String> {
        let res = self.call_ok("listTables", json!({
            "connection": conn, "database": database, "limit": limit
        }))?;
        let arr = res.as_array().ok_or("listTables: 结果不是数组")?;
        Ok(arr
            .iter()
            .filter_map(|t| {
                t.as_str()
                    .map(|s| s.to_string())
                    .or_else(|| t.get("name").and_then(|n| n.as_str()).map(|s| s.to_string()))
            })
            .collect())
    }

    /// One-shot SQL: paginate `executeQueryPage`/`fetchQueryPage` until the
    /// session is exhausted (or `max_rows` reached). Returns (columns, rows).
    /// The ODPS instance-tunnel caps each SELECT at 10000 rows, which is fine
    /// here — this path is for ad-hoc / aggregate pushdown, not bulk pulls.
    pub fn execute_query(
        &mut self,
        conn: &Value,
        sql: &str,
        max_rows: u64,
    ) -> Result<(Vec<String>, Vec<Vec<Value>>), String> {
        let page = 10_000u64.min(max_rows.max(1));
        let res = self.call_ok("executeQueryPage", json!({
            "connection": conn, "sql": sql, "pageSize": page, "maxRows": max_rows, "fetchSize": page
        }))?;
        let columns: Vec<String> = res
            .get("columns")
            .and_then(|c| c.as_array())
            .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
            .unwrap_or_default();
        let mut rows: Vec<Vec<Value>> = res
            .get("rows")
            .and_then(|r| r.as_array())
            .map(|a| a.iter().filter_map(|r| r.as_array().cloned()).collect())
            .unwrap_or_default();
        let has_more = res.get("has_more").and_then(|h| h.as_bool()).unwrap_or(false);
        let session_id = res.get("session_id").and_then(|s| s.as_str()).unwrap_or("").to_string();
        if has_more && !session_id.is_empty() {
            loop {
                let fr = self.call_ok("fetchQueryPage", json!({
                    "connection": conn, "sessionId": session_id, "pageSize": page
                }))?;
                if let Some(a) = fr.get("rows").and_then(|r| r.as_array()) {
                    for r in a {
                        if let Some(row) = r.as_array() {
                            rows.push(row.clone());
                        }
                    }
                }
                if rows.len() as u64 >= max_rows
                    || !fr.get("has_more").and_then(|h| h.as_bool()).unwrap_or(false)
                {
                    break;
                }
            }
            let _ = self.call_ok("closeQuerySession", json!({ "connection": conn, "sessionId": session_id }));
        }
        Ok((columns, rows))
    }
}

// ───────────────────── JRE discovery ─────────────────────

/// Locate a `java` binary: `DBX_JAVA_BIN` env → `JAVA_HOME/bin/java` → `java`
/// on PATH. Used both for the dbx launcher (which also does its own lookup)
/// and for the `check_java_runtime` Tauri command.
pub fn find_java_bin() -> Option<String> {
    if let Ok(v) = std::env::var("DBX_JAVA_BIN") {
        if std::path::Path::new(&v).is_file() {
            return Some(v);
        }
    }
    if let Ok(jh) = std::env::var("JAVA_HOME") {
        let p = std::path::Path::new(&jh).join("bin/java");
        if p.is_file() {
            return p.to_str().map(String::from);
        }
    }
    if Command::new("java").arg("-version").output().is_ok() {
        Some("java".to_string())
    } else {
        None
    }
}

/// Run `java -version`, returning the first stderr line (e.g.
/// "openjdk version \"17.0.19\"") — used by the frontend to show whether the
/// optional Java runtime is present for MaxCompute sources.
pub fn check_java_runtime() -> Result<String, String> {
    let bin = find_java_bin().ok_or_else(|| "未找到 Java 运行时（安装 JRE 17+ 或设置 JAVA_HOME）".to_string())?;
    let out = Command::new(&bin).arg("-version").output().map_err(|e| format!("执行 {bin} -version 失败: {e}"))?;
    // `java -version` prints to stderr.
    let first = String::from_utf8_lossy(&out.stderr).lines().next().unwrap_or("").to_string();
    if out.status.success() || !first.is_empty() {
        Ok(first)
    } else {
        Err("java -version 异常退出".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::DbConnectionRecord;

    #[test]
    fn build_maxcompute_connection_url_and_creds() {
        // `options` JSON uses camelCase keys (matches MaxcomputeOpts serde rename,
        // consistent with the rest of the wire format).
        let opts = serde_json::json!({
            "endpoint": "http://svc/api",
            "project": "p1",
            "tunnelEndpoint": "http://dt/api",
            "driverCoord": "com.aliyun.odps:odps-jdbc:3.9.3",
            "concurrency": 5
        }).to_string();
        let rec = DbConnectionRecord {
            id: "x".into(), name: "n".into(), db_type: "maxcompute".into(),
            host: String::new(), port: 0, database_name: String::new(),
            username: "AKID".into(), password: "AKSECRET".into(), ssl_mode: "disable".into(),
            created_at: 0, options: Some(opts),
        };
        let conn = build_maxcompute_connection(&rec, &["a.jar".into()]).unwrap();
        assert_eq!(conn["connection_string"], "jdbc:odps:http://svc/api?project=p1&tunnelEndpoint=http://dt/api");
        assert_eq!(conn["username"], "AKID");
        assert_eq!(conn["password"], "AKSECRET");
        assert_eq!(conn["jdbc_driver_class"], "com.aliyun.odps.jdbc.OdpsDriver");
        assert_eq!(conn["jdbc_driver_paths"][0], "a.jar");
        assert_eq!(conn["connect_timeout_secs"], 60);
    }

    #[test]
    fn build_maxcompute_connection_missing_endpoint() {
        // Parses fine (driverCoord defaulted), but the explicit endpoint-empty
        // check in build_maxcompute_connection fires.
        let rec = DbConnectionRecord {
            id: "x".into(), name: "n".into(), db_type: "maxcompute".into(),
            host: String::new(), port: 0, database_name: String::new(),
            username: "u".into(), password: "p".into(), ssl_mode: "disable".into(),
            created_at: 0, options: Some(serde_json::json!({"endpoint":"","project":"p","driverCoord":"x"}).to_string()),
        };
        assert!(build_maxcompute_connection(&rec, &[]).is_err());
    }
}
