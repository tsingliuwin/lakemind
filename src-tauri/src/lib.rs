//! LakeMind M1 — pure-compute DuckDB client.
//!
//! No Agent, no Canvas, no Polars. This entry point wires the [`AppState`]
//! singleton and the four M1 commands into the Tauri runtime.

mod commands;
mod duckdb;
mod error;
mod model;
mod state;

use state::AppState;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
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
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
