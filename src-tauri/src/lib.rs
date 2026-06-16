//! LakeMind M1 — pure-compute DuckDB client.
//!
//! No Agent, no Canvas, no Polars. This entry point wires the [`AppState`]
//! singleton and the four M1 commands into the Tauri runtime.

mod commands;
mod db;
mod duckdb;
mod error;
mod model;
mod state;

use state::AppState;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Initialize central SQLite database
    if let Err(e) = db::init_global_db() {
        eprintln!("Failed to initialize central SQLite database: {e}");
    }

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            commands::register_folder,
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
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
