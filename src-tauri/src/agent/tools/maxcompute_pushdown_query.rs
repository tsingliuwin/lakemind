use serde::{Deserialize, Serialize};
use serde_json::json;
use rig_core::{completion::ToolDefinition, tool::Tool};

use super::super::error::ToolError;
use super::super::events::{emit_tool_call, emit_tool_result, next_tool_id};
use crate::state::AppState;

/// Aggregate pushdown to MaxCompute via the dbx JDBC sidecar. Used when
/// `sample_guard` intercepts an aggregation over a sampled/partial maxcompute
/// table - there's no DuckDB `{kind}_query` function for MaxCompute, so the
/// pushdown goes through the sidecar (≤10000 result rows, fine for aggregates).
#[derive(Deserialize, Serialize)]
pub(crate) struct MaxcomputePushdownQueryArgs {
    table_name: String,
    sql: String,
}

pub(crate) struct MaxcomputePushdownQueryTool {
    pub(crate) app_state: AppState,
    pub(crate) task_id: String,
    pub(crate) window: tauri::Window,
}

impl Tool for MaxcomputePushdownQueryTool {
    const NAME: &'static str = "maxcompute_pushdown_query";
    type Error = ToolError;
    type Args = MaxcomputePushdownQueryArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "maxcompute_pushdown_query".to_string(),
            description: "对 MaxCompute（ODPS）外部表做聚合下推：把 SQL 下推到远程 MaxCompute 执行，只拉回结果行（≤1 万行）。用于在本地采样/部分物化的 maxcompute 表上做聚合时避免指标失真——sample_guard 拦截后改用本工具。FROM 用该表在远程的 project.table 全限定名（通过 describe_table 或 list_tables 查看远程表名，拦截消息也会给出）。".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "table_name": { "type": "string", "description": "本地已注册的 maxcompute 源表名（用于定位连接配置）" },
                    "sql": { "type": "string", "description": "要下推到 MaxCompute 执行的 SQL。FROM 用远程全限定名 project.table（通过 describe_table 查看）" }
                },
                "required": ["table_name", "sql"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let table_name = args.table_name.trim().to_string();
        let sql = args.sql.trim().to_string();
        if table_name.is_empty() || sql.is_empty() {
            return Err(ToolError("table_name 和 sql 不能为空".to_string()));
        }
        let ws_path = self.app_state.workspace_path.lock().await.clone();
        let call_id = next_tool_id("mcq");
        emit_tool_call(
            &self.window, &self.task_id, &call_id, "maxcompute_pushdown_query",
            json!({ "table_name": table_name, "sql": sql }),
        );
        let start = std::time::Instant::now();

        let table_for_resolve = table_name.clone();
        let sql_for_run = sql.clone();
        let result: Result<(String, crate::model::SqlResult, usize), String> = tokio::task::spawn_blocking(move || -> Result<(String, crate::model::SqlResult, usize), String> {
            let sqlite = crate::db::get_db_conn()?;
            // 1. resolve the source record (to find the connection) by table_name
            let rec = if let Ok(Some(r)) = crate::db::get_source_by_table(&sqlite, &ws_path, &table_for_resolve) {
                r
            } else {
                let all = crate::db::list_sources(&sqlite, &ws_path)?;
                all.into_iter().find(|s| {
                    s.table_name == table_for_resolve
                        || (s.table_name.starts_with("s_") && s.table_name.ends_with(&format!("_{}", table_for_resolve)))
                }).ok_or_else(|| format!("未找到表 '{table_for_resolve}' 的注册元数据"))?
            };
            if rec.kind != "maxcompute" {
                return Err(format!("表 '{table_for_resolve}' 不是 maxcompute 源（kind={}），无法用本工具下推", rec.kind));
            }
            // file_path = "maxcompute://{conn_id}/{project}/{table}" -> conn_id is segment 2
            let conn_id = rec.file_path.split('/').nth(2)
                .ok_or_else(|| format!("无法从 file_path 解析 connection_id: {}", rec.file_path))?
                .to_string();
            let conn_record = crate::db::get_db_connection(&sqlite, &conn_id)?
                .ok_or_else(|| format!("未找到连接 {conn_id}"))?;

            // 2. resolve sidecar paths + driver jars + run the pushdown
            let paths = crate::external::paths::SidecarPaths::get()?;
            let opts = conn_record.maxcompute_opts();
            let jars = paths.driver_jars(&opts.driver_coord)?;
            let launcher = paths.dbx_launcher()?;
            let mut sc = crate::external::jdbc_sidecar::JdbcSidecar::spawn(&launcher)?;
            let conn_obj = crate::external::jdbc_sidecar::build_maxcompute_connection(&conn_record, &jars)?;
            let (columns, rows) = sc.execute_query(&conn_obj, &sql_for_run, 10_000)?;
            sc.close();

            // Build the structured SqlResult for the frontend UI + a markdown
            // table string for the LLM context.
            let n = rows.len();
            let res = crate::model::SqlResult {
                columns: columns.clone(),
                column_types: vec![String::new(); columns.len()],
                rows: rows.clone(),
                row_count: n,
                truncated: false,
                elapsed_ms: 0,
            };
            let mut out = String::new();
            out.push_str(&columns.join(" | "));
            out.push('\n');
            out.push_str(&columns.iter().map(|_| "---").collect::<Vec<_>>().join(" | "));
            out.push('\n');
            for row in &rows {
                let cells: Vec<String> = row.iter().map(|v| match v {
                    serde_json::Value::Null => "NULL".to_string(),
                    serde_json::Value::String(s) => s.clone(),
                    other => other.to_string(),
                }).collect();
                out.push_str(&cells.join(" | "));
                out.push('\n');
            }
            Ok((out, res, n))
        })
        .await
        .map_err(|e| ToolError(format!("线程执行失败: {e}")))?;

        let elapsed = start.elapsed().as_millis() as u64;
        match result {
            Ok((out, res, n)) => {
                let summary = format!("下推查询成功，返回 {} 行（{} 列）", n, res.columns.len());
                emit_tool_result(&self.window, &self.task_id, &call_id, "ok",
                    summary, Some(sql.clone()), Some(res), Some(elapsed), None);
                Ok(out)
            }
            Err(e) => {
                emit_tool_result(&self.window, &self.task_id, &call_id, "error",
                    e.clone(), None, None, Some(elapsed), None);
                Err(ToolError(e))
            }
        }
    }
}
