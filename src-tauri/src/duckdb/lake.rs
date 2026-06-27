//! DuckLake lakehouse connection management.
//!
//! Each workspace owns a DuckLake stored under its directory:
//!   `<workspace>/lake.ducklake` — the catalog (a DuckDB file holding metadata)
//!   `<workspace>/lake_data/`    — the data (parquet files for materialized tables)
//!
//! The DuckDB *session* connection is in-memory; DuckLake is the sole persistent
//! layer for imported tables and views. After [`attach_workspace_lake`], the lake
//! is the default catalog, so unqualified names resolve there (e.g. `FROM s_sales`).
//!
//! Import strategy (see `commands::import_file_to_workspace`):
//!   * small file (≤ threshold) → copy into workspace dir + `CREATE TABLE` (materialized)
//!   * large file (> threshold) → register in place + `CREATE VIEW` (zero-copy)
//! The threshold is configurable; default [`DEFAULT_ZERO_COPY_THRESHOLD`].

use std::path::Path;

use duckdb::Connection;

use crate::error::{AppError, AppResult};

/// Hidden directory under a workspace that holds ALL DuckDB/DuckLake artifacts
/// (catalog + data + WAL). Keeping them here leaves the workspace root clean
/// (only the user's data files) and they never appear in the Files tree.
pub const LAKE_DIR: &str = ".lake";
/// DuckLake catalog file (SQLite backend — avoids DuckDB's ART-index corruption
/// bugs that the default DuckDB catalog hits on crash; see duckdb #18505/#21468).
pub const CATALOG_FILE: &str = "lake.sqlite";
/// DuckLake parquet data directory within [`LAKE_DIR`].
pub const DATA_DIR: &str = "lake_data";
/// Config key holding the zero-copy threshold in bytes.
pub const THRESHOLD_CONFIG_KEY: &str = "import.zero_copy_threshold_bytes";

/// Default threshold (bytes) above which an import is registered as a zero-copy
/// VIEW rather than materialized into a DuckLake table. Configurable via the
/// [`THRESHOLD_CONFIG_KEY`] key (see `db::get_zero_copy_threshold`).
pub const DEFAULT_ZERO_COPY_THRESHOLD: u64 = 100 * 1024 * 1024; // 100 MB

/// Make sure the `ducklake` extension is loaded on `conn`. Idempotent.
///
/// `INSTALL` is best-effort: an already-installed extension errors on re-INSTALL,
/// which we deliberately ignore. `LOAD` must succeed — every downstream lake
/// operation depends on it, so its error is surfaced with a clear message.
pub fn ensure_ducklake_loaded(conn: &Connection) -> AppResult<()> {
    if let Err(e) = conn.execute("INSTALL ducklake;", []) {
        // Already-installed extensions error on re-INSTALL; that's fine.
        eprintln!("INSTALL ducklake (ignored if already installed): {e}");
    }
    conn.execute("LOAD ducklake;", []).map_err(|e| {
        AppError::new(format!(
            "无法加载 ducklake 扩展。LakeMind 使用 DuckLake 存储表，首次运行需要联网下载该扩展。\n原始错误: {e}"
        ))
    })?;
    // SQLite catalog backend: the default DuckDB catalog uses an ART index that
    // corrupts on crash ("node without metadata in ARTOperator::Insert",
    // duckdb #18505/#21468/#18190, unfixed even in 1.5). SQLite has no such index.
    if let Err(e) = conn.execute("INSTALL sqlite;", []) {
        eprintln!("INSTALL sqlite (ignored if already installed): {e}");
    }
    conn.execute("LOAD sqlite;", [])
        .map_err(|e| AppError::new(format!("无法加载 sqlite 扩展: {e}")))?;
    Ok(())
}

/// Resolve `<workspace>/lake.ducklake` and `<workspace>/lake_data/`, creating
/// the directories if they do not yet exist.
pub fn ensure_lake_paths(ws_dir: &Path) -> AppResult<(std::path::PathBuf, std::path::PathBuf)> {
    std::fs::create_dir_all(ws_dir)
        .map_err(|e| AppError::new(format!("无法创建工作区目录: {e}")))?;
    let lake_dir = ws_dir.join(LAKE_DIR);
    std::fs::create_dir_all(&lake_dir)
        .map_err(|e| AppError::new(format!("无法创建 lake 目录: {e}")))?;
    let data_dir = lake_dir.join(DATA_DIR);
    std::fs::create_dir_all(&data_dir)
        .map_err(|e| AppError::new(format!("无法创建 lake 数据目录: {e}")))?;
    Ok((lake_dir.join(CATALOG_FILE), data_dir))
}

/// ATTACH a workspace's DuckLake and set it as the default catalog (`USE lake`).
///
/// Creates the catalog + data dir on first use. After this returns, every
/// unqualified table/view reference resolves inside the lake. The `DATA_PATH` is
/// passed on every attach: on first creation it seeds the catalog, on reconnect
/// it matches the stored path (so DuckDB's `OVERRIDE_DATA_PATH` leaves it alone).
///
/// **Crash recovery:** if the previous process was killed mid-write (common when
/// restarting `tauri dev`), the catalog's WAL can be left out of sync with the
/// catalog file. We detect that and drop the stale WAL — the already-checkpointed
/// catalog file is intact, so there is no data loss. Only if the catalog itself
/// is corrupt do we rebuild an empty one (its tables are then re-materialized
/// from the workspace files on the next `register_workspace_sources`).
pub fn attach_workspace_lake(conn: &Connection, ws_dir: &Path) -> AppResult<()> {
    let (catalog, data_dir) = ensure_lake_paths(ws_dir)?;
    // DuckDB paths inside SQL must use forward slashes (Windows backslashes break).
    let catalog_str = catalog.to_string_lossy().replace('\\', "/");
    let wal_str = format!("{catalog_str}.wal");
    let data_str = format!("{}/", data_dir.to_string_lossy().replace('\\', "/"));
    let sql = format!("ATTACH 'ducklake:sqlite:{catalog_str}' AS lake (DATA_PATH '{data_str}');");

    if let Err(first_err) = conn.execute(&sql, []) {
        let msg = first_err.to_string();
        let wal_mismatch = msg.contains("WAL")
            || msg.contains("checkpoint iteration")
            || msg.contains("iteration does not match");
        if !wal_mismatch {
            return Err(AppError::new(format!("ATTACH ducklake 失败: {msg}")));
        }
        // Unclean shutdown (e.g. killing `tauri dev`) left a WAL that doesn't match
        // the catalog. The catalog file itself is usually intact (DuckLake's
        // SQLite catalog checkpoints the committed state). Try the least-
        // destructive recovery first: drop just the stale WAL and re-ATTACH. Only
        // if that also fails (catalog genuinely corrupt/truncated) do we wipe the
        // whole lake store and let `register_workspace_sources` re-materialize.
        eprintln!("ducklake: WAL mismatch after crash, attempting WAL-only recovery: {msg}");
        let _ = std::fs::remove_file(&wal_str);
        if conn.execute(&sql, []).is_ok() {
            eprintln!("ducklake: recovered via WAL drop (catalog intact)");
        } else {
            eprintln!("ducklake: WAL drop failed, rebuilding lake store");
            let _ = std::fs::remove_file(&catalog);
            let _ = std::fs::remove_dir_all(&data_dir);
            std::fs::create_dir_all(&data_dir)
                .map_err(|e| AppError::new(format!("无法重建 lake 数据目录: {e}")))?;
            conn.execute(&sql, [])
                .map_err(|e| AppError::new(format!("ATTACH ducklake 失败（已尝试重建 lake store）: {e}")))?;
        }
    }

    conn.execute("USE lake;", [])
        .map_err(|e| AppError::new(format!("USE lake 失败: {e}")))?;
    Ok(())
}
