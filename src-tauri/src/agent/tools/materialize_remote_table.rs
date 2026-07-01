use serde::{Deserialize, Serialize};
use serde_json::json;
use rig_core::{completion::ToolDefinition, tool::Tool};

use super::super::config::get_query_hard_timeout;
use super::super::error::ToolError;
use super::super::events::{emit_tool_call, emit_tool_result, next_tool_id};
use crate::model::ColumnInfo;
use crate::state::AppState;

/// Chunk size: rows per OFFSET batch, and id-range width per Id-strategy step.
/// Tuned so each step is seconds of remote I/O (enough to amortize round-trips,
/// small enough that a timeout/interrupt lands within a reasonable window).
const CHUNK: i64 = 50_000;

#[derive(Deserialize, Serialize)]
pub(crate) struct MaterializeRemoteTableArgs {
    table_name: String,
    /// Optional partition-strategy hint. `None`/`"auto"` picks automatically
    /// (time → id → offset fallback). `"time"`/`"id"` force a strategy;
    /// `"none"` forces the OFFSET fallback.
    #[serde(default)]
    partition_strategy: Option<String>,
    /// When `true`, only pull rows newer than what's already materialized
    /// (requires a prior partitioned materialization — reuses the stored
    /// `partition_keys`). Default `false` = full (re)materialization.
    #[serde(default)]
    incremental: Option<bool>,
}

pub(crate) struct MaterializeRemoteTableTool {
    pub(crate) app_state: AppState,
    pub(crate) task_id: String,
    pub(crate) window: tauri::Window,
}

/// One chosen partition strategy, derived from the remote table's column
/// metadata. `Offset` is the no-good-column fallback.
enum Strategy {
    /// Partition by a time-typed column, sliced into day buckets.
    Time { col: String },
    /// Partition by a monotonically increasing integer column, sliced into
    /// fixed-width id ranges.
    Id { col: String },
    /// No usable column — page through with LIMIT/OFFSET.
    Offset,
}

impl Strategy {
    fn label(&self) -> &'static str {
        match self {
            Strategy::Time { .. } => "时间分区",
            Strategy::Id { .. } => "自增ID分区",
            Strategy::Offset => "OFFSET分批",
        }
    }
}

/// Pick the partition column from cached column metadata.
///
/// Priority: a time-typed column (preferring conventional "created-at" names)
/// → a BIGINT integer column → `Offset` fallback. Deterministic (no LLM): the
/// same schema always yields the same strategy, so incremental runs stay
/// consistent across invocations.
fn pick_strategy(columns: &[ColumnInfo], hint: &Option<String>) -> Strategy {
    let forced = hint.as_deref().unwrap_or("auto").to_lowercase();
    if forced == "none" {
        return Strategy::Offset;
    }

    let is_time = |ty: &str| {
        let t = ty.to_uppercase();
        t.contains("TIMESTAMP") || t.contains("DATETIME") || t == "DATE"
    };
    let is_int = |ty: &str| {
        let t = ty.to_uppercase();
        t == "BIGINT" || t == "INTEGER" || t == "INT" || t == "INT64"
    };

    if forced == "time" || forced == "auto" {
        let time_cols: Vec<&ColumnInfo> = columns.iter().filter(|c| is_time(&c.r#type)).collect();
        if !time_cols.is_empty() {
            // Prefer conventional "row creation time" names so we slice on the
            // column whose advance marks genuinely new rows.
            const PREF: [&str; 8] = [
                "created_at", "create_time", "gmt_create", "create_at",
                "event_time", "updated_at", "update_time", "gmt_modified",
            ];
            let chosen = time_cols
                .iter()
                .min_by_key(|c| PREF.iter().position(|p| c.name.eq_ignore_ascii_case(p)).unwrap_or(usize::MAX))
                .copied()
                .unwrap_or(time_cols[0]);
            return Strategy::Time { col: chosen.name.clone() };
        }
        if forced == "time" {
            return Strategy::Offset;
        }
    }

    if forced == "id" || forced == "auto" {
        let int_cols: Vec<&ColumnInfo> = columns.iter().filter(|c| is_int(&c.r#type)).collect();
        if !int_cols.is_empty() {
            const PREF: [&str; 5] = ["id", "uid", "pk", "rid", "gid"];
            let chosen = int_cols
                .iter()
                .min_by_key(|c| PREF.iter().position(|p| c.name.eq_ignore_ascii_case(p)).unwrap_or(usize::MAX))
                .copied()
                .unwrap_or(int_cols[0]);
            return Strategy::Id { col: chosen.name.clone() };
        }
        if forced == "id" {
            return Strategy::Offset;
        }
    }

    Strategy::Offset
}

impl Tool for MaterializeRemoteTableTool {
    const NAME: &'static str = "materialize_remote_table";
    type Error = ToolError;
    type Args = MaterializeRemoteTableArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "materialize_remote_table".to_string(),
            description: "将指定的外部数据库表/采样表（如 s_db_cdp_message_sending_notification）完整导入为本地 DuckDB 物理表，以实现全量数据的高速本地分析与聚合。大表会自动按时间列或自增ID列分区拉取，并支持进度反馈与超时保护。若该表已物化过且远程有新增数据，可传 incremental=true 只补拉增量。此操作在数据表极大时可能消耗较多网络与存储空间。".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "table_name": { "type": "string", "description": "要进行全量本地物化的采样表或外部表名，例如 s_postgres_users" },
                    "partition_strategy": { "type": "string", "enum": ["auto", "time", "id", "none"], "description": "分区策略：auto(默认,自动选时间/ID列)、time(强制按时间列)、id(强制按自增ID)、none(强制OFFSET分批)。一般用 auto 即可。" },
                    "incremental": { "type": "boolean", "description": "是否只补拉增量（默认false全量）。true 时复用上次物化的分区列，只拉取新增行。需要该表已被分区物化过。" }
                },
                "required": ["table_name"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let table_name = args.table_name.trim();
        if !table_name.chars().all(|c| c.is_alphanumeric() || c == '_') {
            return Err(ToolError("表名包含非法字符，仅允许字母、数字和下划线。".to_string()));
        }

        let incremental = args.incremental.unwrap_or(false);
        let ws_path = self.app_state.workspace_path.lock().await.clone();
        let conn = self.app_state.conn.clone();
        let ih = self.app_state.interrupt_handle.lock().unwrap().clone();
        let table_name_str = table_name.to_string();

        let call_id = next_tool_id("mat");
        emit_tool_call(
            &self.window, &self.task_id, &call_id, "materialize_remote_table",
            json!({ "table_name": table_name, "partition_strategy": args.partition_strategy, "incremental": incremental }),
        );

        let start = std::time::Instant::now();
        let hard_secs = get_query_hard_timeout();

        // 1. Resolve the SourceRecord (exact match, then fuzzy/suffix fallback).
        let source_record_opt = tokio::task::spawn_blocking(move || -> Result<Option<crate::db::SourceRecord>, String> {
            let sqlite = crate::db::get_db_conn()?;
            if let Ok(Some(rec)) = crate::db::get_source_by_table(&sqlite, &ws_path, &table_name_str) {
                return Ok(Some(rec));
            }
            let all_sources = crate::db::list_sources(&sqlite, &ws_path)?;
            for src in all_sources {
                if src.table_name.starts_with("s_") && (src.table_name.ends_with(&format!("_{}", table_name_str)) || src.table_name == table_name_str) {
                    return Ok(Some(src));
                }
                if src.table_name == table_name_str || src.scan_path.ends_with(&format!(".{}", table_name_str)) {
                    return Ok(Some(src));
                }
            }
            Ok(None)
        })
        .await
        .map_err(|e| ToolError(format!("线程执行失败: {e}")))
        .and_then(|res| res.map_err(|e| ToolError(format!("数据库查询失败: {e}"))))?;

        let mut source_record = match source_record_opt {
            Some(r) => r,
            None => {
                let err_msg = format!("未找到该表 '{}' 的注册元数据。确保它是已挂载的外部表。", table_name);
                emit_tool_result(
                    &self.window, &self.task_id, &call_id, "error",
                    err_msg.clone(), None, None, Some(start.elapsed().as_millis() as u64), None,
                );
                return Err(ToolError(err_msg));
            }
        };

        let actual_table_name = source_record.table_name.clone();
        let full_path = source_record.scan_path.clone();

        // 2. Decide the partition strategy. Incremental mode reuses the stored
        //    partition key; full mode picks one from cached column metadata.
        let strategy = if incremental {
            if source_record.partition_keys.is_empty() {
                let err_msg = format!(
                    "无法对「{}」做增量更新：该表此前未进行过分区物化（无已存储的分区列）。请先做一次全量物化（incremental 留空或 false）。",
                    table_name
                );
                emit_tool_result(
                    &self.window, &self.task_id, &call_id, "error",
                    err_msg.clone(), None, None, Some(start.elapsed().as_millis() as u64), None,
                );
                return Err(ToolError(err_msg));
            }
            let key = source_record.partition_keys[0].clone();
            let ty = source_record
                .columns
                .iter()
                .find(|c| c.name.eq_ignore_ascii_case(&key))
                .map(|c| c.r#type.to_uppercase());
            match ty.as_deref() {
                Some(t) if t.contains("TIMESTAMP") || t.contains("DATETIME") || t == "DATE" => Strategy::Time { col: key },
                Some(t) if t == "BIGINT" || t == "INTEGER" || t == "INT" || t == "INT64" => Strategy::Id { col: key },
                _ => Strategy::Offset,
            }
        } else {
            pick_strategy(&source_record.columns, &args.partition_strategy)
        };

        // Persist the chosen partition key so future incremental runs reuse it.
        if let Strategy::Time { col } | Strategy::Id { col } = &strategy {
            if source_record.partition_keys.iter().all(|x| !x.eq_ignore_ascii_case(col)) {
                source_record.partition_keys = vec![col.clone()];
            }
        }

        emit_tool_result(
            &self.window, &self.task_id, &call_id, "running",
            format!(
                "正在将外部表「{}」全量物化到本地（策略：{}{}），数据量较大时可能需要一些时间...",
                table_name,
                strategy.label(),
                if incremental { "，增量模式" } else { "" }
            ),
            None, None, None, None,
        );

        // 3. Drive the (possibly multi-step) materialization on the blocking
        //    pool. Each step emits a `running` progress event. Returns the
        //    final row count of the local table.
        let conn_clone = conn.clone();
        let local_name = actual_table_name.clone();
        let full_path_c = full_path.clone();
        // Capture the label before moving `strategy` into the blocking closure
        // (the summary message below needs it after the closure runs).
        let strategy_label = strategy.label();
        let strat = strategy;
        let window_c = self.window.clone();
        let task_c = self.task_id.clone();
        let call_c = call_id.clone();

        let blocking_fut = tokio::task::spawn_blocking(move || -> Result<i64, String> {
            let guard = conn_clone.blocking_lock();

            if !incremental {
                // Full mode: drop any prior view/table, then create an empty
                // table mirroring the remote schema (zero-row CTAS — cheap).
                let _ = guard.execute(&format!("DROP VIEW IF EXISTS \"{}\";", local_name), []);
                let _ = guard.execute(&format!("DROP TABLE IF EXISTS \"{}\";", local_name), []);
                let create_sql = format!("CREATE TABLE \"{n}\" AS SELECT * FROM {p} WHERE 1=0;", n = local_name, p = full_path_c);
                guard.execute(&create_sql, []).map_err(|e| format!("建表(复制远程表结构)失败: {e}"))?;
            }

            let mut written: i64 = 0;
            match &strat {
                Strategy::Offset => {
                    let mut offset: i64 = 0;
                    loop {
                        let sql = format!(
                            "INSERT INTO \"{n}\" SELECT * FROM {p} LIMIT {lim} OFFSET {off};",
                            n = local_name, p = full_path_c, lim = CHUNK, off = offset
                        );
                        let n = guard.execute(&sql, []).map_err(|e| {
                            format!("拉取批次 offset={off} 失败: {e}。已落盘 {w} 行。", off = offset, e = e, w = written)
                        })? as i64;
                        if n == 0 { break; }
                        written += n;
                        offset += CHUNK;
                        if n < CHUNK { break; }
                        emit_tool_result(&window_c, &task_c, &call_c, "running",
                            format!("物化中（OFFSET分批）：已写入 {w} 行", w = written),
                            None, None, None, None);
                    }
                }
                Strategy::Id { col } => {
                    let (mut lo, max_id) = range_min_max(&guard, &full_path_c, col)?;
                    if incremental {
                        // Resume just past the last already-materialized id.
                        let existing: Option<i64> = guard
                            .query_row(
                                &format!("SELECT MAX(\"{c}\") FROM \"{n}\"", c = col.replace('"', "\"\""), n = local_name),
                                [], |r| r.get::<_, Option<i64>>(0),
                            )
                            .ok()
                            .flatten();
                        if let Some(m) = existing {
                            if m >= lo { lo = m + 1; }
                        }
                    }
                    if max_id >= lo {
                        let total_ids = (max_id - lo + 1).max(1);
                        let mut cur = lo;
                        while cur <= max_id {
                            let hi = cur.saturating_add(CHUNK);
                            let sql = format!(
                                "INSERT INTO \"{n}\" SELECT * FROM {p} WHERE \"{c}\" >= {lo} AND \"{c}\" < {hi};",
                                n = local_name, p = full_path_c, c = col.replace('"', "\"\""), lo = cur, hi = hi
                            );
                            let n = guard.execute(&sql, []).map_err(|e| {
                                format!("拉取ID分区 [{lo},{hi}) 失败: {e}。已落盘 {w} 行。", lo = cur, hi = hi, e = e, w = written)
                            })? as i64;
                            written += n;
                            cur = hi;
                            let done = (cur - lo).min(total_ids);
                            let pct = (done as u64 * 100 / total_ids as u64).min(100);
                            emit_tool_result(&window_c, &task_c, &call_c, "running",
                                format!("物化中（ID分区 {c}）：约 {done}/{total} ({pct}%)", c = col, done = done, total = total_ids, pct = pct),
                                None, None, None, None);
                        }
                    }
                }
                Strategy::Time { col } => {
                    // Day buckets via DuckDB date arithmetic (no Rust date lib).
                    // bounds: [min_day, max_day] as YYYY-MM-DD strings + day span.
                    let (min_day, max_day, total_days) = day_bounds(&guard, &full_path_c, col)?;
                    if let (Some(lo), Some(hi), total) = (min_day, max_day, total_days) {
                        // Iterate integer day offsets; DuckDB builds each boundary.
                        let mut day_idx: i64 = 0;
                        loop {
                            let start_expr = format!("CAST('{lo}' AS DATE) + INTERVAL '{d} days'", lo = lo, d = day_idx);
                            let end_expr = format!("CAST('{lo}' AS DATE) + INTERVAL '{d} days'", lo = lo, d = day_idx + 1);
                            // Stop when this bucket's start is past max_day.
                            let past_max_sql = format!("SELECT CAST(({start}) AS DATE) > CAST('{hi}' AS DATE)", start = start_expr, hi = hi);
                            let past_max: bool = guard.query_row(&past_max_sql, [], |r| r.get::<_, bool>(0)).unwrap_or(true);
                            if past_max {
                                // Last bucket: pull the tail (>= start, no upper bound).
                                let sql = format!(
                                    "INSERT INTO \"{n}\" SELECT * FROM {p} WHERE \"{c}\" >= ({start});",
                                    n = local_name, p = full_path_c, c = col.replace('"', "\"\""), start = start_expr
                                );
                                let n = guard.execute(&sql, []).map_err(|e| {
                                    format!("拉取时间分区 {lo}+{d}天 失败: {e}。已落盘 {w} 行。", lo = lo, d = day_idx, e = e, w = written)
                                })? as i64;
                                written += n;
                                break;
                            }
                            let sql = format!(
                                "INSERT INTO \"{n}\" SELECT * FROM {p} WHERE \"{c}\" >= ({start}) AND \"{c}\" < ({end});",
                                n = local_name, p = full_path_c, c = col.replace('"', "\"\""), start = start_expr, end = end_expr
                            );
                            let n = guard.execute(&sql, []).map_err(|e| {
                                format!("拉取时间分区 {lo}+{d}天 失败: {e}。已落盘 {w} 行。", lo = lo, d = day_idx, e = e, w = written)
                            })? as i64;
                            written += n;
                            day_idx += 1;
                            let done = day_idx as u64;
                            let pct = if total > 0 { (done * 100 / total as u64).min(100) } else { 100 };
                            emit_tool_result(&window_c, &task_c, &call_c, "running",
                                format!("物化中（时间分区 {c}）：{done}/{total} 天 ({pct}%)，已写入 {w} 行",
                                    c = col, done = done, total = total, pct = pct, w = written),
                                None, None, None, None);
                        }
                    }
                }
            }

            // Final row count — DuckDB metadata pushdown makes this cheap.
            let n: i64 = guard
                .query_row(&format!("SELECT count(*) FROM \"{}\"", local_name), [], |r| r.get(0))
                .map_err(|e| format!("统计物化结果行数失败: {e}（已写入约 {w} 行）", w = written))?;
            Ok(n)
        });

        // 4. Apply the hard timeout around the whole materialization.
        let exec_res = if hard_secs > 0 {
            match tokio::time::timeout(std::time::Duration::from_secs(hard_secs), blocking_fut).await {
                Ok(r) => r
                    .map_err(|e| ToolError(format!("线程执行失败: {e}")))
                    .and_then(|res| res.map_err(ToolError)),
                Err(_) => {
                    ih.interrupt();
                    Err(ToolError(format!(
                        "物化已达到最大等待时间（{hard_secs} 秒）被强制终止。可调大 settings.json 的 query_hard_timeout 后重试，或用 incremental=true 续传。"
                    )))
                }
            }
        } else {
            blocking_fut.await
                .map_err(|e| ToolError(format!("线程执行失败: {e}")))
                .and_then(|res| res.map_err(ToolError))
        };

        let elapsed = start.elapsed().as_millis() as u64;

        // On failure, clean up any half-written table (full mode only) so a
        // retry starts clean; incremental mode leaves prior data intact.
        if let Err(ref err) = exec_res {
            if !incremental {
                let conn_drop = conn.clone();
                let drop_name = actual_table_name.clone();
                let _ = tokio::task::spawn_blocking(move || {
                    let guard = conn_drop.blocking_lock();
                    let _ = guard.execute(&format!("DROP TABLE IF EXISTS \"{}\";", drop_name), []);
                }).await;
            }
            emit_tool_result(
                &self.window, &self.task_id, &call_id, "error",
                err.0.clone(), None, None, Some(elapsed), None,
            );
            return Err(err.clone());
        }

        let imported_rows = exec_res.unwrap();

        // 5. Update metadata in SQLite (storage / is_sampled / row counts /
        //    partition_keys are already set on `source_record`).
        let ws_path = self.app_state.workspace_path.lock().await.clone();
        let table_name_clone = actual_table_name.clone();
        let conn_clone = conn.clone();
        let mut updated_record = source_record;
        updated_record.storage = "table".to_string();
        updated_record.is_sampled = false;
        updated_record.row_count = Some(imported_rows);
        updated_record.full_row_count = Some(imported_rows);

        let update_db_res = tokio::task::spawn_blocking(move || -> Result<(), String> {
            let new_cols = {
                let guard = conn_clone.blocking_lock();
                crate::duckdb::schema::describe_view(&guard, &table_name_clone).ok()
            };
            if let Some(cols) = new_cols {
                updated_record.columns = cols;
            }
            let sqlite = crate::db::get_db_conn()?;
            crate::db::upsert_source(&sqlite, &ws_path, &updated_record)?;
            Ok(())
        })
        .await
        .map_err(|e| ToolError(format!("线程执行失败: {e}")))
        .and_then(|res| res.map_err(|e| ToolError(format!("更新元数据失败: {e}"))));

        if let Err(err) = update_db_res {
            emit_tool_result(
                &self.window, &self.task_id, &call_id, "error",
                err.0.clone(), None, None, Some(elapsed), None,
            );
            return Err(err);
        }

        let summary = format!(
            "成功将外部表 {} (本地表名: {}) 完整物化到本地 DuckDB，共导入 {} 行数据（{}{}）。",
            table_name, actual_table_name, imported_rows,
            strategy_label,
            if incremental { "，增量" } else { "" }
        );
        emit_tool_result(
            &self.window, &self.task_id, &call_id, "ok",
            summary.clone(), None, None, Some(elapsed), None,
        );
        Ok(summary)
    }
}

/// `SELECT MIN(col), MAX(col)` for an integer partition column as i64 bounds.
/// Falls back to (0,0) if the remote aggregate fails — the caller then writes
/// nothing, which is safe.
fn range_min_max(guard: &duckdb::Connection, full_path: &str, col: &str) -> Result<(i64, i64), String> {
    let sql = format!("SELECT COALESCE(MIN(\"{c}\"),0), COALESCE(MAX(\"{c}\"),0) FROM {p};", c = col.replace('"', "\"\""), p = full_path);
    guard
        .query_row(&sql, [], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))
        .map_err(|e| format!("查询分区列 {c} 的 MIN/MAX 失败: {e}", c = col))
}

/// Day-bucket bounds for a time-typed partition column. Returns
/// `(min_day_str, max_day_str, day_span)` where each `*_day_str` is `YYYY-MM-DD`
/// and `day_span` is the number of day buckets (max_day - min_day + 1).
/// `(None, None, 0)` means the table is empty / all-null.
fn day_bounds(guard: &duckdb::Connection, full_path: &str, col: &str) -> Result<(Option<String>, Option<String>, i64), String> {
    // CAST to DATE truncates to the day; this also works across TIMESTAMP /
    // DATETIME / DATE column types in DuckDB's scanner.
    let sql = format!(
        "SELECT CAST(CAST(MIN(\"{c}\") AS DATE) AS VARCHAR), CAST(CAST(MAX(\"{c}\") AS DATE) AS VARCHAR), CAST(CAST(MAX(\"{c}\") AS DATE) AS DATE) - CAST(CAST(MIN(\"{c}\") AS DATE) AS DATE) + INTERVAL '1 day' FROM {p};",
        c = col.replace('"', "\"\""), p = full_path
    );
    guard
        .query_row(&sql, [], |r| {
            let lo: Option<String> = r.get(0)?;
            let hi: Option<String> = r.get(1)?;
            // DuckDB returns a DATE - DATE + INTERVAL as an interval; cast the
            // whole expression's day count by reading it back as i64 days.
            // Simpler: re-derive the span in Rust from the two day strings.
            Ok((lo, hi, 0))
        })
        .map_err(|e| format!("查询时间分区列 {c} 的 MIN/MAX 失败: {e}", c = col))
        .and_then(|(lo, hi, _)| {
            let span = match (&lo, &hi) {
                (Some(l), Some(h)) => days_between(l, h),
                _ => 0,
            };
            Ok((lo, hi, span))
        })
}

/// Inclusive day count between two `YYYY-MM-DD` strings, computed without a
/// date library (proleptic Gregorian). Returns 0 if either fails to parse.
fn days_between(lo: &str, hi: &str) -> i64 {
    match (date_to_ord(lo), date_to_ord(hi)) {
        (Some(a), Some(b)) => (b - a + 1).max(0),
        _ => 0,
    }
}

/// Convert a `YYYY-MM-DD` string to a day count since a fixed epoch using the
/// proleptic Gregorian calendar (algorithm: Howard Hinnant's days_from_civil).
fn date_to_ord(s: &str) -> Option<i64> {
    let day = s.get(..10)?;
    let mut parts = day.split('-');
    let y: i64 = parts.next()?.parse().ok()?;
    let m: u32 = parts.next()?.parse().ok()?;
    let d: u32 = parts.next()?.parse().ok()?;
    Some(days_from_civil(y, m, d))
}

/// Howard Hinnant's `days_from_civil` — proleptic Gregorian day number.
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u32; // [0, 399]
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146097 + doe as i64 - 719468
}
