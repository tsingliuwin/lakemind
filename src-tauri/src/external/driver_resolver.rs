//! Resolve vendor JDBC driver JARs (Maven coordinates) into the app-data cache
//! and build the sidecar classpath. Reuses the dbx plugin's bundled Maven
//! resolver (`app.dbx.jdbc.maven.DbxMavenResolver`), validated in the spike.

use std::path::Path;
use std::process::Command;

/// Recursively collect every `*.jar` under `cache` (the dbx maven cache dir).
pub fn collect_jars(cache: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![cache.to_path_buf()];
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

/// Resolve a Maven coordinate (e.g. `com.aliyun.odps:odps-jdbc:3.9.3`) and its
/// transitive deps into `cache_dir` via the dbx `dbx-maven-resolver` launcher,
/// returning the resolved JAR paths. Idempotent: the resolver skips already-
/// cached artifacts. It prints a JSON object with `artifacts[].file` (absolute
/// paths); falls back to globbing `cache_dir` if the JSON shape differs.
pub fn resolve_driver_jars(
    resolver_bin: &str,
    coord: &str,
    cache_dir: &Path,
) -> Result<Vec<String>, String> {
    let out = Command::new(resolver_bin)
        .arg(coord)
        .output()
        .map_err(|e| format!("启动 maven-resolver ({resolver_bin}) 失败: {e}"))?;
    if !out.status.success() {
        return Err(format!("maven-resolver 失败: {}", String::from_utf8_lossy(&out.stderr)));
    }
    let v: serde_json::Value = serde_json::from_slice(&out.stdout)
        .map_err(|e| format!("解析 maven-resolver 输出失败: {e}"))?;
    if let Some(err) = v.get("error").and_then(|e| e.as_str()) {
        return Err(format!("maven-resolver 错误: {err}"));
    }
    let jars: Vec<String> = v
        .get("artifacts")
        .and_then(|a| a.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|a| a.get("file").and_then(|f| f.as_str()).map(String::from))
                .collect()
        })
        .unwrap_or_default();
    if jars.is_empty() {
        Ok(collect_jars(cache_dir))
    } else {
        Ok(jars)
    }
}
