use std::fs;
use std::path::PathBuf;
use rusqlite::Connection;

/// Get the system home directory
pub fn get_home_dir() -> Option<PathBuf> {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map(PathBuf::from)
        .ok()
}

/// Get the global config path ~/.lakemind/
pub fn get_lakemind_dir() -> Result<PathBuf, String> {
    let mut path = get_home_dir().ok_or("Could not resolve home directory".to_string())?;
    path.push(".lakemind");
    Ok(path)
}

/// Get the global sqlite database file path ~/.lakemind/lakemind.db
pub fn get_db_path() -> Result<PathBuf, String> {
    let mut path = get_lakemind_dir()?;
    path.push("lakemind.db");
    Ok(path)
}

/// Establish connection to sqlite database
pub fn get_db_conn() -> Result<Connection, String> {
    let db_path = get_db_path()?;
    Connection::open(&db_path).map_err(|e| format!("Failed to open SQLite database: {e}"))
}

/// Initialize central directory structure and table schemas
pub fn init_global_db() -> Result<(), String> {
    let lakemind_dir = get_lakemind_dir()?;
    
    // Create sqls/ and chats/ subdirectory structure
    let sqls_dir = lakemind_dir.join("sqls");
    let chats_dir = lakemind_dir.join("chats");
    
    fs::create_dir_all(&sqls_dir).map_err(|e| format!("Failed to create sqls directory: {e}"))?;
    fs::create_dir_all(&chats_dir).map_err(|e| format!("Failed to create chats directory: {e}"))?;
    
    let conn = get_db_conn()?;
    
    // Enable foreign key support in SQLite
    let _ = conn.execute("PRAGMA foreign_keys = ON;", []);

    // Create workspaces registry table
    conn.execute(
        "CREATE TABLE IF NOT EXISTS workspaces (
            path TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            created_at INTEGER NOT NULL
        )",
        [],
    ).map_err(|e| format!("Failed to create workspaces table: {e}"))?;
    
    // Create tasks index table
    conn.execute(
        "CREATE TABLE IF NOT EXISTS tasks (
            id TEXT PRIMARY KEY,
            workspace_path TEXT NOT NULL,
            name TEXT NOT NULL,
            kind TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            saved INTEGER NOT NULL,
            FOREIGN KEY(workspace_path) REFERENCES workspaces(path) ON DELETE CASCADE
        )",
        [],
    ).map_err(|e| format!("Failed to create tasks table: {e}"))?;
    
    // If the database has no workspaces, seed the DefaultProject
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM workspaces", [], |row| row.get(0))
        .unwrap_or(0);
        
    if count == 0 {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;
            
        conn.execute(
            "INSERT INTO workspaces (path, name, created_at) VALUES ('DefaultProject', 'DefaultProject', ?)",
            [now],
        ).map_err(|e| format!("Failed to insert default workspace: {e}"))?;
    }
    
    Ok(())
}
