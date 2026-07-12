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
mod logging;
mod model;
mod state;
mod agent;
mod fingerprint;
mod usage;
mod okf;
mod tenets;

use state::AppState;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Initialize the global SQLite metadata DB (workspaces / tasks / sources / config).
    if let Err(e) = db::init_global_db() {
        eprintln!("Failed to initialize central SQLite database: {e}");
    }

    // Seed the global analysis-tenets bundle (~/.lakemind/tenets/) on first
    // launch. Non-fatal — a failure only means the agent's tenets tools will
    // report "library not initialized" until a successful seed.
    if let Err(e) = tenets::seed_tenets_if_empty() {
        eprintln!("Failed to seed tenets bundle: {e}");
    }

    // Install the tracing subscriber: the custom [`SqliteEmitLayer`] persists
    // every event to SQLite and pushes info+ to the frontend console, while the
    // fmt layer mirrors to stdout for dev diagnostics. Installed BEFORE the
    // Tauri runtime starts so early-boot events are captured; the AppHandle is
    // wired in from `setup` below (events before then degrade to SQLite-only).
    use tracing_subscriber::EnvFilter;
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    let fmt_layer = tracing_subscriber::fmt::layer()
        .with_target(false)
        .with_level(true)
        .compact();
    // RUST_LOG overrides; default to `info` so debug events don't spam.
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::registry()
        .with(fmt_layer)
        .with(logging::SqliteEmitLayer::new())
        .with(filter)
        .init();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .manage(AppState::default())
        .setup(|_app| {
            // Hand the AppHandle to the logging layer so it can emit to the
            // frontend `app-log` channel.
            logging::set_handle(_app.handle().clone());

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
            commands::get_system_preamble,
            commands::list_tenets,
            commands::get_tenet_content,
            commands::start_agent_chat,
            commands::resolve_tool_confirmation,
            commands::abort_chat,
            commands::append_log,
            commands::query_logs,
            commands::clear_logs,
            commands::get_db_connections,
            commands::upsert_db_connection,
            commands::delete_db_connection,
            commands::test_db_connection,
            commands::test_llm_connection,
            commands::link_connection_to_workspace,
            commands::unlink_connection_from_workspace,
            commands::list_workspace_connections,
            commands::list_db_connection_tables,
            commands::register_database_table,
            commands::get_table_ddl,
            commands::save_image_from_base64,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
