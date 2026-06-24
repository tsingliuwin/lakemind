// Pure-frontend DuckDB SQL formatting helper.
// Centralizes the sql-formatter options so the editor's manual "format" button
// and the auto-injected SQL (click a table/column, "open in SQL panel") share
// one source of truth for dialect, indent, and keyword case.

import { format } from "sql-formatter";
import type { FormatOptionsWithLanguage } from "sql-formatter";

/**
 * DuckDB 方言格式化配置：保留原大小写、2 空格缩进、表达式宽度 100 列。
 * 只整理缩进与换行，不改语义、不强制关键字大小写。
 */
const DUCKDB_FORMAT_OPTIONS: FormatOptionsWithLanguage = {
  language: "duckdb",
  tabWidth: 2,
  keywordCase: "preserve",
  expressionWidth: 100,
};

/**
 * 用 DuckDB 方言格式化 SQL。语法不合法时抛错——交给调用方决定回退策略
 * （编辑器手动格式化需要错误信息做 UI 反馈，故直接抛）。
 */
export function formatDuckdbSql(sql: string): string {
  return format(sql, DUCKDB_FORMAT_OPTIONS);
}

/**
 * 格式化 SQL，失败时静默回退原值。用于自动注入编辑器的场景：即使
 * 格式化失败，也保证后续 run() 拿到可执行的原始 SQL，不阻塞查询。
 */
export function tryFormatDuckdbSql(sql: string): string {
  try {
    return formatDuckdbSql(sql);
  } catch (err) {
    console.error("[sqlFormat] format failed, falling back to original:", err);
    return sql;
  }
}
