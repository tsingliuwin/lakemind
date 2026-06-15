// 通信类型定义 —— 与 src-tauri/src/model.rs 一一对应。
// 修改 M1 通信格式时请同步两侧。

export type SourceKind = "parquet" | "csv" | "json" | "delta";

export interface ColumnInfo {
  name: string;
  type: string;
  null: boolean;
}

export interface SourceTable {
  /** Sanitized view name actually used in SQL, e.g. `s_sales`. */
  name: string;
  /** Human-friendly name shown in the tree. */
  label: string;
  kind: SourceKind;
  /** Original filesystem path the user dropped. */
  path: string;
  /** Glob / path expression handed to DuckDB's `read_*` function. */
  scanPath: string;
  /** Hive partition keys detected from the directory layout, if any. */
  partitionKeys: string[];
  /** Fast estimate (parquet row-group metadata) or full count; null until computed. */
  rowCountEstimate: number | null;
  columns: ColumnInfo[];
}

export type JsonValue = string | number | boolean | null | JsonValue[] | { [key: string]: JsonValue };

export interface SqlResult {
  columns: string[];
  columnTypes: string[];
  rows: JsonValue[][];
  rowCount: number;
  truncated: boolean;
  elapsedMs: number;
}

/** 编辑器工具栏可选的行数上限预设。 */
export const ROW_CAP_OPTIONS = [
  { label: "1,000", value: 1_000 },
  { label: "10,000", value: 10_000 },
  { label: "100,000", value: 100_000 },
  { label: "无限制", value: 0 }, // 0 === 不限
] as const;

/** 底部 SQL 执行日志的一条记录。成功与失败都会记录，
 * 保证用户始终能看到执行过的查询痕迹。 */
export interface LogEntry {
  id: number;
  /** Unix 毫秒时间戳。 */
  ts: number;
  sql: string;
  status: "ok" | "error";
  rowCount?: number;
  truncated?: boolean;
  elapsedMs?: number;
  /** 原始 DuckDB 报错信息（仅失败时）。 */
  error?: string;
}

/** 将 DuckDB 类型名归类到检查器徽标用的颜色族。 */
export type TypeFamily = "int" | "float" | "str" | "time" | "bool" | "other";

export function typeFamily(type: string): TypeFamily {
  const t = type.toUpperCase();
  if (/INT|BIGINT|HUGEINT|TINYINT|SMALLINT|SHORT|LONG/.test(t)) return "int";
  if (/FLOAT|DOUBLE|DECIMAL|REAL/.test(t)) return "float";
  if (/VARCHAR|CHAR|TEXT|STRING|BLOB/.test(t)) return "str";
  if (/TIME|DATE|TIMESTAMP|INTERVAL/.test(t)) return "time";
  if (/BOOL/.test(t)) return "bool";
  return "other";
}
