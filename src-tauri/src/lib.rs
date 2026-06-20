//! LakeMind — local-first lakehouse analysis terminal.
//!
//! Entry point: wires the [`state::AppState`] singleton and the command surface
//! into the Tauri runtime. The DuckDB *session* connection is in-memory; each
//! workspace's tables/views live in a per-workspace DuckLake (`<ws>/lake.ducklake`
//! + `<ws>/lake_data/`). Business mappings (file↔table, tasks, config) live in
//! the global SQLite DB (`~/.lakemind/lakemind.db`).

mod commands;
mod db;
mod duckdb;
mod error;
mod model;
mod state;

use state::AppState;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Initialize the global SQLite metadata DB (workspaces / tasks / sources / config).
    if let Err(e) = db::init_global_db() {
        eprintln!("Failed to initialize central SQLite database: {e}");
    }

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            commands::list_sources,
            commands::describe_table,
            commands::execute_sql,
            commands::import_file_to_workspace,
            commands::select_directory,
            commands::read_directory,
            commands::register_workspace_sources,
            commands::load_workspaces,
            commands::add_workspace,
            commands::remove_workspace,
            commands::load_workspace_tasks,
            commands::save_sql_task,
            commands::save_chat_task,
            commands::delete_task,
            commands::list_duckdb_tables,
            commands::get_app_config,
            commands::set_app_config,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
