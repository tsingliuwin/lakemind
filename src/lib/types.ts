// 通信类型定义 —— 与 src-tauri/src/model.rs 一一对应。
// 修改 M1 通信格式时请同步两侧。

export type SourceKind = "parquet" | "csv" | "json" | "delta" | "excel" | "table" | "view";

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
  /** How this source is stored: materialized DuckLake table / zero-copy view / user custom. */
  storage?: "table" | "view" | "custom";
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

/** 编辑器工具栏可选的行数上限预设。
 * 没有"无限制"档——全量物化会导致前端/后端 OOM。
 * 最高档为 1,000,000，配合行虚拟滚动已能覆盖绝大多数探索场景。 */
export const ROW_CAP_OPTIONS = [
  { label: "1,000", value: 1_000 },
  { label: "10,000", value: 10_000 },
  { label: "100,000", value: 100_000 },
  { label: "1,000,000", value: 1_000_000 },
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

export type TaskKind = "sql" | "chat";

/** Agent 执行过程中的产物卡片，嵌在助手消息里展示。 */
export interface ChatCard {
  id: string;
  /** 卡片类型：步骤摘要 / SQL 代码 / 结果表 / 最终结论。 */
  kind: "step" | "sql" | "table" | "conclusion";
  /** 卡片标题，如「探测了 3 张表」「执行查询」。 */
  title: string;
  /** 详细说明（步骤描述或 SQL 文本）。 */
  detail?: string;
  /** SQL 卡片：可「在 SQL 面板打开」注入到新 SQL task。 */
  sql?: string;
  /** table 卡片：结果行数提示。 */
  rows?: number;
}

/** 一条对话消息。assistant 消息可携带多张步骤卡片。 */
export interface ChatMessage {
  id: string;
  role: "user" | "assistant";
  content: string;
  /** 仅 assistant：本条回复产出的步骤卡片。 */
  cards?: ChatCard[];
  ts: number;
}

export interface QueryTask {
  id: string;
  name: string;
  /** SQL task 的查询文本；chat task 此字段为空字符串。 */
  sql: string;
  createdAt: number;
  /** 任务类型，默认 "sql"。决定主区渲染 ChatView 还是 SqlEditor。 */
  kind?: TaskKind;
  /** chat task 的消息历史。 */
  messages?: ChatMessage[];
  /** 仅 sql：该查询是否已保存 */
  saved?: boolean;
}

export interface Workspace {
  name: string;
  path: string;
}

export interface FileItem {
  name: string;
  path: string;
  is_dir: boolean;
  is_modified: boolean;
}
