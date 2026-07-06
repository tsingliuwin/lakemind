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
mod agent;
mod fingerprint;
mod usage;
mod okf;

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
        .setup(|_app| {
            #[cfg(not(target_os = "macos"))]
            {
                use tauri::Manager;
                if let Some(window) = _app.get_webview_window("main") {
                    let _ = window.set_decorations(false);
                }
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_sources,
            commands::describe_table,
            commands::execute_sql,
            commands::import_file_to_workspace,
            commands::select_directory,
            commands::select_file,
            commands::select_files,
            commands::read_directory,
            commands::register_workspace_sources,
            commands::workspace_register_status,
            commands::load_workspaces,
            commands::add_workspace,
            commands::remove_workspace,
            commands::load_workspace_tasks,
            commands::save_sql_task,
            commands::save_chat_task,
            commands::delete_task,
            commands::list_duckdb_tables,
            commands::list_tables_fast,
            commands::warmup_sources,
            commands::get_dependencies,
            commands::drop_table_safe,
            commands::delete_file,
            commands::get_app_config,
            commands::set_app_config,
            commands::load_settings_json,
            commands::save_settings_json,
            commands::start_agent_chat,
            commands::resolve_tool_confirmation,
            commands::abort_chat,
            commands::log_debug_info,
            commands::get_db_connections,
            commands::upsert_db_connection,
            commands::delete_db_connection,
            commands::test_db_connection,
            commands::link_connection_to_workspace,
            commands::unlink_connection_from_workspace,
            commands::list_workspace_connections,
            commands::list_db_connection_tables,
            commands::register_database_table,
            commands::get_table_ddl,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
