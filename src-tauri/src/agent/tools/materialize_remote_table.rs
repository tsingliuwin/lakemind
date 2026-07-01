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

/// One on-demand partition range to materialize. Exactly one of the time form
/// (`start`/`end`) or the id form (`min`/`max`) is set; the strategy column is
/// chosen by `partition_strategy`. Example time range:
/// `{"start":"2025-06-01","end":"2025-07-01"}`; id range: `{"min":1,"max":50000}`.
#[derive(Deserialize, Serialize, Clone)]
pub(crate) struct PartitionRange {
    /// Time-range lower bound (inclusive), e.g. "2025-06-01". Used with the
    /// `time` strategy. Leave null for id ranges.
    #[serde(default)]
    start: Option<String>,
    /// Time-range upper bound (exclusive), e.g. "2025-07-01".
    #[serde(default)]
    end: Option<String>,
    /// Id-range lower bound (inclusive). Used with the `id` strategy.
    #[serde(default)]
    min: Option<i64>,
    /// Id-range upper bound (inclusive).
    #[serde(default)]
    max: Option<i64>,
}

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
    /// Optional list of partition ranges to materialize on demand (instead of
    /// the whole table). Each range is either a time span (`start`/`end`) or an
    /// id span (`min`/`max`); the strategy column must match. Rows already
    /// materialized are skipped. Leaves the table in `partial` status.
    #[serde(default)]
    partitions: Option<Vec<PartitionRange>>,
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
            description: "将指定的外部数据库表/采样表完整导入为本地 DuckDB 物理表，以实现全量数据的高速本地分析与聚合。大表会自动按时间列或自增ID列分区拉取，支持进度反馈、超时保护与断点续传（中途失败后再次调用自动跳过已物化部分）。支持三种用法：(1) 全量物化（默认）；(2) 增量更新（incremental=true，只补拉远程新增行）；(3) 按需物化（partitions 指定区间，如只物化某个月）。".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "table_name": { "type": "string", "description": "要进行本地物化的采样表或外部表名，例如 s_postgres_users" },
                    "partition_strategy": { "type": "string", "enum": ["auto", "time", "id", "none"], "description": "分区策略：auto(默认,自动选时间/ID列)、time(强制按时间列)、id(强制按自增ID)、none(强制OFFSET分批,不可续传)。一般用 auto 即可。" },
                    "incremental": { "type": "boolean", "description": "是否只补拉增量（默认false全量）。true 时复用上次物化的分区列，只拉取新增行。需要该表已被分区物化过。注意：全量物化中途失败后再次调用会自动续传，无需显式传 incremental。" },
                    "partitions": { "type": "array", "description": "按需物化：只物化指定的分区区间而非整表。每个元素是 {\"start\":\"2025-06-01\",\"end\":\"2025-07-01\"}(时间区间,配合 time 策略) 或 {\"min\":1,\"max\":50000}(ID区间,配合 id 策略)。已物化的部分会自动跳过。按需物化完成后表为 partial 态，聚合仍会被拦截直到全量完成。", "items": { "type": "object" } }
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
        let partitions = args.partitions.clone().unwrap_or_default();
        let on_demand = !partitions.is_empty();
        let ws_path = self.app_state.workspace_path.lock().await.clone();
        let conn = self.app_state.conn.clone();
        let ih = self.app_state.interrupt_handle.lock().unwrap().clone();
        let table_name_str = table_name.to_string();

        let call_id = next_tool_id("mat");
        emit_tool_call(
            &self.window, &self.task_id, &call_id, "materialize_remote_table",
            json!({ "table_name": table_name, "partition_strategy": args.partition_strategy, "incremental": incremental, "partitions": args.partitions }),
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

        // 2. Decide the partition strategy.
        //    - incremental / on-demand: reuse the stored partition key (or infer
        //      from the partitions' shape for on-demand).
        //    - full: pick from cached column metadata.
        //    For on-demand, infer the strategy kind from the partitions' shape:
        //    time ranges (start/end present) → Time; id ranges (min/max) → Id.
        let strategy = if on_demand {
            let has_time = partitions.iter().any(|p| p.start.is_some() || p.end.is_some());
            let has_id = partitions.iter().any(|p| p.min.is_some() || p.max.is_some());
            // Resolve the column: reuse stored partition key if its type matches
            // the requested shape; otherwise pick a fresh one.
            let forced = if has_time {
                Some("time".to_string())
            } else if has_id {
                Some("id".to_string())
            } else {
                None
            };
            // If the stored key already matches the shape, reuse it; else pick.
            let reuse_ok = source_record
                .partition_keys
                .first()
                .and_then(|k| {
                    source_record.columns.iter().find(|c| c.name.eq_ignore_ascii_case(k))
                })
                .map(|c| {
                    let t = c.r#type.to_uppercase();
                    match forced.as_deref() {
                        Some("time") => t.contains("TIMESTAMP") || t.contains("DATETIME") || t == "DATE",
                        Some("id") => t == "BIGINT" || t == "INTEGER" || t == "INT" || t == "INT64",
                        _ => false,
                    }
                })
                .unwrap_or(false);
            if reuse_ok {
                let key = source_record.partition_keys[0].clone();
                match forced.as_deref() {
                    Some("time") => Strategy::Time { col: key },
                    Some("id") => Strategy::Id { col: key },
                    _ => Strategy::Offset,
                }
            } else {
                pick_strategy(&source_record.columns, &forced)
            }
        } else if incremental {
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

        // Persist the chosen partition key so future runs reuse it.
        if let Strategy::Time { col } | Strategy::Id { col } = &strategy {
            if source_record.partition_keys.iter().all(|x| !x.eq_ignore_ascii_case(col)) {
                source_record.partition_keys = vec![col.clone()];
            }
        }

        // Detect resume: in full mode (not incremental, not on-demand), if the
        // local table already exists AND the strategy supports resume (Time/Id),
        // we resume from the existing watermark instead of rebuilding. OFFSET
        // cannot resume, so it rebuilds from scratch.
        let local_exists = matches!(
            source_record.materialize_status.as_deref(),
            Some(crate::db::mat_status::PARTIAL)
        ) && !incremental
            && !on_demand;
        let resume = local_exists && matches!(strategy, Strategy::Time { .. } | Strategy::Id { .. });
        // rebuild = start from a fresh empty table. We rebuild only for a full
        // run that is NOT resuming (resume / incremental / on-demand append to
        // the existing table). OFFSET always rebuilds since it can't resume.
        let rebuild = !incremental && !on_demand && !resume;

        emit_tool_result(
            &self.window, &self.task_id, &call_id, "running",
            format!(
                "正在将外部表「{}」物化到本地（策略：{}{}{}），数据量较大时可能需要一些时间...",
                table_name,
                strategy.label(),
                if incremental { "，增量" }
                    else if on_demand { "，按需" }
                    else if resume { "，断点续传" }
                    else { "" },
                if on_demand { format!("（{} 个区间）", partitions.len()) } else { String::new() },
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

            // Decide whether to (re)build the table fresh.
            //   full + not resuming            → DROP + CREATE empty (rebuild)
            //   full + resuming                → keep existing table (resume)
            //   incremental / on-demand        → keep existing table (append)
            if rebuild {
                let _ = guard.execute(&format!("DROP VIEW IF EXISTS \"{}\";", local_name), []);
                let _ = guard.execute(&format!("DROP TABLE IF EXISTS \"{}\";", local_name), []);
                let create_sql = format!("CREATE TABLE \"{n}\" AS SELECT * FROM {p} WHERE 1=0;", n = local_name, p = full_path_c);
                guard.execute(&create_sql, []).map_err(|e| format!("建表(复制远程表结构)失败: {e}"))?;
            } else {
                // Ensure the table exists (on-demand/incremental/resume on a
                // table that was never created yet). CREATE IF NOT EXISTS isn't
                // supported for CTAS, so probe and create empty if absent.
                let exists: bool = guard
                    .query_row(
                        &format!(
                            "SELECT count(*) > 0 FROM (SELECT 1 FROM duckdb_tables() WHERE database_name='lake' AND schema_name='main' AND table_name='{n}' UNION SELECT 1 FROM duckdb_views() WHERE database_name='lake' AND schema_name='main' AND view_name='{n}')",
                            n = local_name.replace('\'', "")
                        ),
                        [], |r| r.get::<_, bool>(0),
                    )
                    .unwrap_or(false);
                if !exists {
                    let _ = guard.execute(&format!("DROP VIEW IF EXISTS \"{}\";", local_name), []);
                    let create_sql = format!("CREATE TABLE \"{n}\" AS SELECT * FROM {p} WHERE 1=0;", n = local_name, p = full_path_c);
                    guard.execute(&create_sql, []).map_err(|e| format!("建表(复制远程表结构)失败: {e}"))?;
                }
            }

            let mut written: i64 = 0;
            // On-demand: iterate the requested partition ranges only.
            if on_demand {
                match &strat {
                    Strategy::Time { col } => {
                        for (i, pr) in partitions.iter().enumerate() {
                            let lo = pr.start.clone().unwrap_or_default();
                            let hi = pr.end.clone().unwrap_or_default();
                            let sql = if hi.is_empty() {
                                format!("INSERT INTO \"{n}\" SELECT * FROM {p} WHERE \"{c}\" >= '{lo}';",
                                    n = local_name, p = full_path_c, c = col.replace('"', "\"\""), lo = lo)
                            } else {
                                format!("INSERT INTO \"{n}\" SELECT * FROM {p} WHERE \"{c}\" >= '{lo}' AND \"{c}\" < '{hi}';",
                                    n = local_name, p = full_path_c, c = col.replace('"', "\"\""), lo = lo, hi = hi)
                            };
                            let n = guard.execute(&sql, []).map_err(|e| {
                                format!("按需物化区间#{i} [{lo},{hi}) 失败: {e}。已落盘 {w} 行。", i = i, lo = lo, hi = hi, e = e, w = written)
                            })? as i64;
                            written += n;
                            emit_tool_result(&window_c, &task_c, &call_c, "running",
                                format!("按需物化中：区间 {done}/{total} 完成，已写入 {w} 行", done = i + 1, total = partitions.len(), w = written),
                                None, None, None, None);
                        }
                    }
                    Strategy::Id { col } => {
                        for (i, pr) in partitions.iter().enumerate() {
                            let lo = pr.min.unwrap_or(0);
                            let hi = pr.max.unwrap_or(i64::MAX);
                            // Chunk the id range the same way the full id path does.
                            let mut cur = lo;
                            while cur <= hi {
                                let next = cur.saturating_add(CHUNK).min(hi + 1);
                                let sql = format!(
                                    "INSERT INTO \"{n}\" SELECT * FROM {p} WHERE \"{c}\" >= {lo} AND \"{c}\" <= {hi};",
                                    n = local_name, p = full_path_c, c = col.replace('"', "\"\""), lo = cur, hi = next - 1
                                );
                                let n = guard.execute(&sql, []).map_err(|e| {
                                    format!("按需物化区间#{i} [{lo},{hi}] 失败: {e}。已落盘 {w} 行。", i = i, lo = cur, hi = next - 1, e = e, w = written)
                                })? as i64;
                                written += n;
                                cur = next;
                            }
                            emit_tool_result(&window_c, &task_c, &call_c, "running",
                                format!("按需物化中：区间 {done}/{total} 完成，已写入 {w} 行", done = i + 1, total = partitions.len(), w = written),
                                None, None, None, None);
                        }
                    }
                    Strategy::Offset => {
                        return Err("按需物化(partitions)需要时间列或自增ID列，当前为 OFFSET 策略，无法按区间定位。请用 partition_strategy=time 或 id。".to_string());
                    }
                }
            } else {
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
                    // Resume just past the last already-materialized id. This
                    // applies to both incremental AND full-resume (a prior full
                    // run that timed out left partial data behind).
                    if incremental || resume {
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
                    // Time buckets via DuckDB DATE arithmetic. The bucket width
                    // is chosen adaptively so the total number of buckets (and
                    // thus remote scans) stays bounded (~≤120), regardless of
                    // the table's time span: <200d → daily, <3y → weekly,
                    // <10y → monthly, else quarterly. Each bucket is one INSERT
                    // …WHERE >= start AND < end, pushed down to the remote.
                    let (min_day, max_day, day_span) = day_bounds(&guard, &full_path_c, col)?;
                    // Resume / incremental: advance the lower bound to just after
                    // the last already-materialized day, so we only pull newer
                    // buckets. (We assume prior buckets are complete; a full
                    // rebuild is required to repair gaps.)
                    let mut effective_min = min_day.clone();
                    let mut effective_span = day_span;
                    if incremental || resume {
                        if let Some(local_max) = local_max_time(&guard, &local_name, col)? {
                            effective_min = Some(local_max);
                            // Recompute span against the new lower bound.
                            if let (Some(lo), Some(hi)) = (&effective_min, &max_day) {
                                effective_span = days_between(lo, hi);
                            }
                        }
                    }
                    if let (Some(lo), Some(hi)) = (effective_min, max_day) {
                        let (bucket_days, bucket_label) = pick_bucket_width(effective_span);
                        let total_buckets = ((effective_span as f64) / bucket_days as f64).ceil() as u64;
                        let mut bucket_idx: i64 = 0;
                        loop {
                            let offset_days = bucket_idx * bucket_days;
                            // start = min_day + offset; DuckDB: DATE + INTEGER = DATE.
                            let start_expr = format!("CAST('{lo}' AS DATE) + {offset_days}", lo = lo, offset_days = offset_days);
                            // Is this bucket's start already past max_day? (tail bucket)
                            let past_max_sql = format!(
                                "SELECT (CAST('{lo}' AS DATE) + {offset_days}) > CAST('{hi}' AS DATE);",
                                lo = lo, offset_days = offset_days, hi = hi
                            );
                            let past_max: bool = guard
                                .query_row(&past_max_sql, [], |r| r.get::<_, bool>(0))
                                .unwrap_or(true);
                            if past_max {
                                // No rows left in range — we're done.
                                break;
                            }
                            // end = min_day + (offset + bucket_days). Compare
                            // timestamps with the TIMESTAMP-typed `DATE + INTERVAL`
                            // so it matches a TIMESTAMP column like task_time.
                            let next_offset = offset_days + bucket_days;
                            let end_past_max_sql = format!(
                                "SELECT (CAST('{lo}' AS DATE) + {next_offset}) > CAST('{hi}' AS DATE);",
                                lo = lo, next_offset = next_offset, hi = hi
                            );
                            let end_past_max: bool = guard
                                .query_row(&end_past_max_sql, [], |r| r.get::<_, bool>(0))
                                .unwrap_or(true);
                            let n = if end_past_max {
                                // Final bucket: upper-bounded by max, pull to the end.
                                let sql = format!(
                                    "INSERT INTO \"{n}\" SELECT * FROM {p} WHERE \"{c}\" >= (CAST('{lo}' AS DATE) + INTERVAL '{offset_days} days') AND \"{c}\" <= CAST('{hi}' AS DATE);",
                                    n = local_name, p = full_path_c, c = col.replace('"', "\"\""),
                                    lo = lo, offset_days = offset_days, hi = hi
                                );
                                guard.execute(&sql, []).map_err(|e| {
                                    format!("拉取时间分区(bucket {bi}) 失败: {e}。已落盘 {w} 行。", bi = bucket_idx, e = e, w = written)
                                })? as i64
                            } else {
                                let sql = format!(
                                    "INSERT INTO \"{n}\" SELECT * FROM {p} WHERE \"{c}\" >= (CAST('{lo}' AS DATE) + INTERVAL '{offset_days} days') AND \"{c}\" < (CAST('{lo}' AS DATE) + INTERVAL '{next_offset} days');",
                                    n = local_name, p = full_path_c, c = col.replace('"', "\"\""),
                                    lo = lo, offset_days = offset_days, next_offset = next_offset
                                );
                                guard.execute(&sql, []).map_err(|e| {
                                    format!("拉取时间分区(bucket {bi}) 失败: {e}。已落盘 {w} 行。", bi = bucket_idx, e = e, w = written)
                                })? as i64
                            };
                            let _ = start_expr; // kept for clarity; the INSERTs use the INTERVAL form
                            written += n;
                            bucket_idx += 1;
                            let done = bucket_idx as u64;
                            let pct = if total_buckets > 0 {
                                (done * 100 / total_buckets).min(100)
                            } else {
                                100
                            };
                            emit_tool_result(&window_c, &task_c, &call_c, "running",
                                format!("物化中（时间分区 {c}，{bl}）：{done}/{total} ({pct}%)，已写入 {w} 行",
                                    c = col, bl = bucket_label, done = done, total = total_buckets, pct = pct, w = written),
                                None, None, None, None);
                        }
                    }
                }
            }
            } // end else (full / incremental path)

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

        // On failure: decide whether to keep the half-written table for resume.
        //   - OFFSET strategy: cannot resume (rows aren't locatable) → DROP.
        //   - rebuild=true (fresh full run that failed before any data): DROP.
        //   - Time/Id resume/incremental/on-demand: KEEP the partial table and
        //     mark status "partial" so the next call resumes from the watermark.
        //     This is the "断点续传" guarantee.
        let drop_on_fail = matches!(strategy_label, "OFFSET分批") || rebuild;
        if let Err(ref err) = exec_res {
            // Persist a "partial" status when keeping the half-table, so the
            // sample-guard still intercepts aggregations (data is incomplete) and
            // the next materialize call knows to resume.
            if !drop_on_fail {
                let mut partial_rec = source_record.clone();
                partial_rec.materialize_status = Some(crate::db::mat_status::PARTIAL.to_string());
                partial_rec.is_sampled = false;
                partial_rec.storage = "table".to_string();
                let ws_p = self.app_state.workspace_path.lock().await.clone();
                let _ = tokio::task::spawn_blocking(move || -> Result<(), String> {
                    let sqlite = crate::db::get_db_conn()?;
                    let _ = crate::db::upsert_source(&sqlite, &ws_p, &partial_rec);
                    Ok(())
                }).await;
            } else {
                let conn_drop = conn.clone();
                let drop_name = actual_table_name.clone();
                let _ = tokio::task::spawn_blocking(move || {
                    let guard = conn_drop.blocking_lock();
                    let _ = guard.execute(&format!("DROP TABLE IF EXISTS \"{}\";", drop_name), []);
                }).await;
            }
            let hint = if !drop_on_fail {
                " 已保留已物化部分（断点续传），下次调用将自动从断点继续。"
            } else {
                ""
            };
            emit_tool_result(
                &self.window, &self.task_id, &call_id, "error",
                format!("{}{}", err.0, hint), None, None, Some(elapsed), None,
            );
            return Err(err.clone());
        }

        let imported_rows = exec_res.unwrap();

        // Determine final status: full only when NOT on-demand AND NOT leaving
        // a partial (resume completed counts as full; on-demand is always
        // partial since it deliberately materializes a subset).
        let final_status = if on_demand {
            crate::db::mat_status::PARTIAL
        } else {
            crate::db::mat_status::FULL
        };

        // 5. Update metadata in SQLite (storage / is_sampled / row counts /
        //    partition_keys are already set on `source_record`).
        let ws_path = self.app_state.workspace_path.lock().await.clone();
        let table_name_clone = actual_table_name.clone();
        let conn_clone = conn.clone();
        let mut updated_record = source_record;
        updated_record.storage = "table".to_string();
        updated_record.is_sampled = false;
        updated_record.materialize_status = Some(final_status.to_string());
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

/// The day-portion (`YYYY-MM-DD`) of `MAX(col)` in the already-materialized
/// local table, or `None` if the local table is empty / the column is all-null.
/// Used to resume a time-partition pull from where the last run left off.
fn local_max_time(guard: &duckdb::Connection, local_name: &str, col: &str) -> Result<Option<String>, String> {
    let sql = format!(
        "SELECT CAST(CAST(MAX(\"{c}\") AS DATE) AS VARCHAR) FROM \"{n}\";",
        c = col.replace('"', "\"\""), n = local_name.replace('"', "\"\"")
    );
    guard
        .query_row(&sql, [], |r| r.get::<_, Option<String>>(0))
        .map_err(|e| format!("读取本地表 {n} 的 MAX({c}) 失败: {e}", n = local_name, c = col))
}

/// Day-bucket bounds for a time-typed partition column. Returns
/// `(min_day_str, max_day_str, day_span)` where each `*_day_str` is `YYYY-MM-DD`
/// and `day_span` is the number of day buckets (max_day - min_day + 1).
/// `(None, None, 0)` means the table is empty / all-null.
///
/// Only MIN/MAX are fetched remotely (both are aggregates the scanner pushes
/// down); the day span is computed locally in Rust to avoid DuckDB's
/// `DATE - DATE = BIGINT` (which can't be added to an `INTERVAL`).
fn day_bounds(guard: &duckdb::Connection, full_path: &str, col: &str) -> Result<(Option<String>, Option<String>, i64), String> {
    // CAST to DATE truncates to the day; this also works across TIMESTAMP /
    // DATETIME / DATE column types in DuckDB's scanner.
    let sql = format!(
        "SELECT CAST(CAST(MIN(\"{c}\") AS DATE) AS VARCHAR), CAST(CAST(MAX(\"{c}\") AS DATE) AS VARCHAR) FROM {p};",
        c = col.replace('"', "\"\""), p = full_path
    );
    let (lo, hi): (Option<String>, Option<String>) = guard
        .query_row(&sql, [], |r| Ok((r.get(0)?, r.get(1)?)))
        .map_err(|e| format!("查询时间分区列 {c} 的 MIN/MAX 失败: {e}", c = col))?;
    let span = match (&lo, &hi) {
        (Some(l), Some(h)) => days_between(l, h),
        _ => 0,
    };
    Ok((lo, hi, span))
}

/// Choose a bucket width (in days) and a human label for a time-span, so the
/// total number of buckets stays bounded (~≤120). Narrow spans get daily
/// buckets (fine-grained progress); wide spans coarsen to weeks/months to keep
/// the number of remote scans reasonable.
fn pick_bucket_width(day_span: i64) -> (i64, &'static str) {
    if day_span <= 120 {
        (1, "按天")
    } else if day_span <= 365 * 3 {
        (7, "按周")
    } else if day_span <= 365 * 10 {
        (30, "按月")
    } else {
        (90, "按季")
    }
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
