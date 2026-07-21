//! Resolve the bundled sidecar binaries + the driver-jar cache.
//!
//! Sidecar artifacts ship as Tauri `resources` under `resources/sidecars/...`
//! (resolved via `app.path().resource_dir()`). Vendor JDBC driver JARs are NOT
//! bundled — they're resolved on demand from Maven into a per-app cache.
//!
//! `init(&app)` is called once at app startup (setup hook) to cache the paths
//! so agent tools (which have no `AppHandle`) can reach them via `get()`.

use std::path::PathBuf;
use std::sync::OnceLock;

use tauri::{AppHandle, Manager};

use crate::db::get_home_dir;
use crate::external::driver_resolver;

/// Filesystem locations the sidecar host needs.
#[derive(Clone)]
pub struct SidecarPaths {
    /// dbx JDBC plugin launcher (`bin/dbx-jdbc-plugin`).
    pub dbx_launcher: PathBuf,
    /// MaxCompute Arrow sidecar jar.
    pub arrow_jar: PathBuf,
    /// dbx maven-resolver launcher (`bin/dbx-maven-resolver`).
    pub resolver_bin: PathBuf,
    /// Local cache for resolved driver jars (`~/.lakemind/.odps-maven`).
    pub maven_cache: PathBuf,
}

/// Global cache, set once at startup so agent tools (no AppHandle) can `get()`.
static SIDECAR_PATHS: OnceLock<SidecarPaths> = OnceLock::new();

impl SidecarPaths {
    /// Resolve from the bundled resources + the app-data dir. Paths may not yet
    /// exist (sidecars not packaged / drivers not yet downloaded); callers
    /// surface a clear error when they try to use them.
    pub fn resolve(app: &AppHandle) -> Result<Self, String> {
        let res = app
            .path()
            .resource_dir()
            .map_err(|e| format!("解析 resource_dir 失败: {e}"))?;
        let base = res.join("sidecars");
        let home = get_home_dir().unwrap_or_else(|| PathBuf::from("."));
        let ext = if cfg!(windows) { ".bat" } else { "" };
        Ok(Self {
            dbx_launcher: base.join(format!("dbx-jdbc-plugin/bin/dbx-jdbc-plugin{ext}")),
            arrow_jar: base.join("arrow-maxcompute/arrow-maxcompute-sidecar.jar"),
            resolver_bin: base.join(format!("dbx-jdbc-plugin/bin/dbx-maven-resolver{ext}")),
            maven_cache: home.join(".lakemind").join(".odps-maven"),
        })
    }

    /// Initialize the global cache once at app startup. Idempotent. Called from
    /// the Tauri `setup` hook. Failures are non-fatal (the app still starts;
    /// MaxCompute sources just won't work until fixed).
    pub fn init(app: &AppHandle) -> Result<(), String> {
        let paths = Self::resolve(app)?;
        let _ = SIDECAR_PATHS.set(paths);
        Ok(())
    }

    /// Startup-cached paths, for callers without an `AppHandle` (agent tools).
    pub fn get() -> Result<&'static SidecarPaths, String> {
        SIDECAR_PATHS.get().ok_or_else(|| {
            "sidecar 路径未初始化（app setup 未调用 SidecarPaths::init）".to_string()
        })
    }

    /// String view of a path or a clear error (avoids `Option::unwrap` panics).
    fn s(p: &PathBuf) -> Result<String, String> {
        p.to_str().map(String::from).ok_or_else(|| format!("路径非 UTF-8: {}", p.display()))
    }

    pub fn dbx_launcher(&self) -> Result<String, String> {
        Self::s(&self.dbx_launcher)
    }
    pub fn arrow_jar(&self) -> Result<String, String> {
        Self::s(&self.arrow_jar)
    }
    pub fn resolver_bin(&self) -> Result<String, String> {
        Self::s(&self.resolver_bin)
    }

    /// Resolve (downloading if needed) the vendor driver jars for a Maven
    /// coordinate, returning the classpath entries. Idempotent — the resolver
    /// skips already-cached artifacts.
    pub fn driver_jars(&self, coord: &str) -> Result<Vec<String>, String> {
        driver_resolver::resolve_driver_jars(&self.resolver_bin()?, coord, &self.maven_cache)
    }
}
