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

/** 硬上限：即使 UI 误传 0/负数/超大值，也绝不会发起可能 OOM 的全量查询。
 * 1,000,000 行 × 行虚拟滚动是当前 M1 的安全边界；真·流式分块留到 M2/M3。 */
const ROW_CAP_HARD_MAX = 1_000_000;

/**
 * 执行一条即席 SELECT。`rowCap` 是行数上限，结果超过会被截断并置 truncated 标志。
 * 不存在"无限制"路径——0/负数/缺省都会被夹到硬上限，杜绝前后端 OOM。
 */
export async function executeSql(sql: string, rowCap: number): Promise<SqlResult> {
  const cap = rowCap && rowCap > 0 ? Math.min(rowCap, ROW_CAP_HARD_MAX) : ROW_CAP_HARD_MAX;
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


