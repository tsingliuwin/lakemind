// 通信类型定义 —— 与 src-tauri/src/model.rs 一一对应。
// 修改 M1 通信格式时请同步两侧。

export type SourceKind = "parquet" | "csv" | "json" | "delta" | "excel" | "table" | "view" | "postgres" | "mysql";

export interface DbConnection {
  id: string;
  name: string;
  dbType: "postgres" | "mysql";
  host: string;
  port: number;
  databaseName: string;
  username: string;
  password?: string;
  sslMode?: string;
  createdAt?: number;
}

/** 文件导入进度事件（后端 emit "import-progress"）。 */
export interface ImportProgress {
  file: string;
  /** "copying" | "scanning" | "registering" | "done" | "error" */
  stage: string;
  table?: string;
  columns?: number;
  rows?: number;
  error?: string;
}

/** 依赖关系：上游（它依赖谁）+ 下游（谁依赖它）。 */
export interface DepInfo {
  upstreams: string[];
  downstreams: string[];
}

/** 项目名行状态点的颜色档位：
 * - "all"     绿：应注册的数据文件已全部注册成表（或工作区无数据文件）
 * - "partial" 橙：仅部分注册成功
 * - "none"    红：有数据文件但全部未注册 */
export type RegisterStatus = "all" | "partial" | "none";

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
  isSampled?: boolean;
  fullRowCount?: number | null;
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

/**
 * Agent 执行过程中的一段产物。一条 assistant 消息是 Segment 的有序列表，
 * 按真实发生顺序排列：reasoning → tool → reasoning → tool → … → text(结论)。
 * 这取代了旧的 {content, reasoning, cards, phase} 平行桶，保留思考与工具
 * 调用的时序关系。
 */
export type Segment =
  | { type: "reasoning"; id: string; text: string; elapsedMs?: number; startTime?: number }
  | {
      type: "tool";
      id: string;
      tool: string; // "list_tables" | "describe_table" | "execute_query" | "sample_data" | "create_table" | "create_view" | "drop_object"
      args?: unknown;
      status: "running" | "ok" | "error" | "awaiting";
      /** 人类可读摘要（折叠时显示）。 */
      summary?: string;
      /** execute_query 等：SQL 文本，可「在 SQL 面板打开」。awaiting 时为待确认 DDL。 */
      sql?: string;
      table?: SqlResult;
      elapsedMs?: number;
      result?: string;
    }
  | {
      type: "chart";
      id: string;
      chartType: "bar" | "line" | "pie" | "scatter" | "funnel" | "gauge";
      title?: string;
      /** X 轴 / 分类列名。 */
      xField?: string;
      /** Y 轴 / 数值列名（多列 = 多系列）。 */
      yFields?: string[];
      /** 原始查询数据（用于渲染 + 切换类型时重算）。 */
      table: SqlResult;
    }
  | { type: "text"; id: string; text: string }
  | { type: "error"; id: string; text: string };

/** 一条对话消息。assistant 消息由有序 Segment 构成。 */
export interface ChatMessage {
  id: string;
  role: "user" | "assistant";
  /** 有序产物段（user 消息为单个 text 段）。 */
  segments: Segment[];
  ts: number;
}

/** @deprecated 旧卡片模型，仅用于迁移历史 chat 任务。 */
export interface ChatCard {
  id: string;
  kind: "step" | "sql" | "table" | "conclusion";
  title: string;
  detail?: string;
  sql?: string;
  rows?: number;
}

export interface TokenUsage {
  // ── Legacy fields (kept for backward-compat with persisted data written
  //    before the metrics refactor). `derivePanelMetrics` falls back to these
  //    when the new real fields are absent; `mergeUsage` no longer writes them. ──
  inputTokens?: number;
  outputTokens?: number;
  totalTokens?: number;
  cachedInputTokens?: number;
  messagesTokens?: number;
  toolsTokens?: number;
  preambleTokens?: number;
  cacheHitRate?: number;
  _totalInputAllTurns?: number;
  _totalCachedAllTurns?: number;
  _peakInputTokens?: number;

  // ── New real fields (provider-normalized by the backend). ──
  /** True total prompt tokens this call (cache read + creation + fresh). */
  promptTokens?: number;
  /** Completion (output) tokens this call. */
  completionTokens?: number;
  /** Tokens served from the provider cache (cheap). */
  cacheReadTokens?: number;
  /** Tokens written to the provider cache this call. */
  cacheCreationTokens?: number;
  /** Full-price input tokens (neither cached nor newly-cached). */
  freshInputTokens?: number;
  /** `k = 1` (uncalibrated) token estimate of the fixed system prompt. */
  estPreambleRaw?: number;
  /** `k = 1` (uncalibrated) token estimate of the tool definitions block. */
  estToolsRaw?: number;
  /**
   * Per-model calibration factor (EMA of real/estimated prompt). Applied to
   * `estPreambleRaw` / `estToolsRaw` so the composition estimate converges
   * toward reality over turns. Defaults to 1.0 when no sample exists.
   */
  _calibK?: number;
  /** True when the current values are a pre-FinalResponse estimate (internal
   *  only — never displayed as a label; drives the "freeze real, advance
   *  output" merge behavior). */
  isEstimate?: boolean;

  // ── Cumulative across the whole conversation (real, per LLM call). ──
  _totalPrompt?: number;
  _totalCompletion?: number;
  _totalCacheRead?: number;
  _totalCacheCreation?: number;
  /** Number of LLM calls that produced a real FinalResponse. Drives the
   *  composition multiplier (preamble/tools are sent on every call). */
  _llmCallCount?: number;
  /** Number of completed user turns (one per finished agent run). Displayed. */
  _turnCount?: number;
  /** Peak `promptTokens` ever seen — the context-window bar never shrinks. */
  _peakPromptTokens?: number;
  /** Generation speed (tok/s) of the most recently completed run. */
  _lastTokPerSec?: number;
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
  /** 使用的模型 ID */
  modelId?: string;
  /** chat task 的累计 token 用量（持久化，伴随对话全生命周期）。 */
  tokenUsage?: TokenUsage;
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
