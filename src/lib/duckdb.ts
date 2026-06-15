// Thin typed wrappers over the four M1 Tauri commands.
// Each maps 1:1 to a `#[tauri::command]` in src-tauri/src/commands.rs.

import { invoke } from "@tauri-apps/api/core";
import type { ColumnInfo, SourceTable, SqlResult } from "./types";

// Re-export the wire types so components can import them from a single module.
export type { ColumnInfo, SourceTable, SqlResult };

/** Scan a dropped folder/file and register every detected SOURCE as a view. */
export async function registerFolder(path: string): Promise<SourceTable[]> {
  return invoke<SourceTable[]>("register_folder", { path });
}

/** Return all currently registered SOURCE tables. */
export async function listSources(): Promise<SourceTable[]> {
  return invoke<SourceTable[]>("list_sources");
}

/** Column metadata for a single registered view. */
export async function describeTable(name: string): Promise<ColumnInfo[]> {
  return invoke<ColumnInfo[]>("describe_table", { name });
}

/**
 * Run an ad-hoc SELECT. `rowCap` of `0` (or falsy) means uncapped; otherwise
 * the result is truncated to that many rows.
 */
export async function executeSql(sql: string, rowCap: number): Promise<SqlResult> {
  // The Rust side treats `null` as uncapped; we convert 0 → null here so the
  // UI can represent "no limit" with the number 0.
  const cap = rowCap && rowCap > 0 ? rowCap : null;
  return invoke<SqlResult>("execute_sql", { sql, rowCap: cap });
}

/** Import a file or folder into the active workspace directory and register it as a view. */
export async function importFileToWorkspace(workspace: string, path: string): Promise<SourceTable[]> {
  return invoke<SourceTable[]>("import_file_to_workspace", { workspace, path });
}

/** Open a native folder picker and return the selected absolute path. */
export async function selectDirectory(): Promise<string | null> {
  return invoke<string | null>("select_directory");
}


