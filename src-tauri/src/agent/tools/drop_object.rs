use serde::{Deserialize, Serialize};
use serde_json::json;
use rig_core::{completion::ToolDefinition, tool::Tool};

use super::super::error::ToolError;
use super::ddl_shared::{sanitize_ddl_ident, DdlToolShared};

#[derive(Deserialize, Serialize)]
pub(crate) struct DropObjectArgs {
    name: String,
}

pub(crate) struct DropObjectTool {
    pub(crate) shared: DdlToolShared,
}

impl Tool for DropObjectTool {
    const NAME: &'static str = "drop_object";
    type Error = ToolError;
    type Args = DropObjectArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "drop_object".to_string(),
            description: "删除指定的表或视图。如果该对象有下游依赖（其他表/视图引用了它），会自动级联删除所有下游依赖（先删下游再删目标）。删除后数据不可恢复，请确认后再使用。".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "要删除的表或视图名" }
                },
                "required": ["name"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let name = sanitize_ddl_ident(args.name.trim())?;

        // 1. Compute the cascade deletion order: all transitive downstreams
        //    (leaves first) + the target last. This way one batch of DROP
        //    statements removes everything without "still referenced" errors.
        let ws_path = self.shared.app_state.workspace_path.lock().await.clone();
        let name_for_cascade = name.clone();
        let cascade = tokio::task::spawn_blocking(move || -> Vec<String> {
            let Ok(sqlite) = crate::db::get_db_conn() else { return vec![name_for_cascade]; };
            crate::fingerprint::cascade_delete_order(&sqlite, &ws_path, &name_for_cascade)
        })
        .await
        .unwrap_or_else(|_| vec![name.clone()]);

        // 2. Determine each object's type (table vs view) so we emit precise
        //    DROP statements instead of blindly trying both.
        let conn = self.shared.app_state.conn.clone();
        let cascade_for_type = cascade.clone();
        let type_map: Vec<(String, bool, bool)> = tokio::task::spawn_blocking(move || {
            let guard = conn.blocking_lock();
            cascade_for_type.iter().map(|n| {
                let is_tbl = guard.query_row(
                    "SELECT count(*) FROM duckdb_tables() WHERE database_name='lake' AND schema_name='main' AND table_name=?",
                    [n], |r| r.get::<_, i64>(0),
                ).unwrap_or(0) > 0;
                let is_vw = guard.query_row(
                    "SELECT count(*) FROM duckdb_views() WHERE database_name='lake' AND schema_name='main' AND view_name=?",
                    [n], |r| r.get::<_, i64>(0),
                ).unwrap_or(0) > 0;
                (n.clone(), is_tbl, is_vw)
            }).collect()
        })
        .await
        .unwrap_or_default();

        let has_downstream = cascade.len() > 1;
        let summary_pending = if has_downstream {
            format!("删除 {} 及其 {} 个下游依赖", name, cascade.len() - 1)
        } else {
            format!("删除对象 {}", name)
        };

        self.shared.run(
            "drop_object", "drop",
            json!({ "name": name.clone() }),
            summary_pending,
            None,
            move || {
                let mut parts = Vec::new();
                for (n, is_tbl, is_vw) in &type_map {
                    if *is_vw { parts.push(format!("DROP VIEW IF EXISTS \"{}\";", n)); }
                    if *is_tbl { parts.push(format!("DROP TABLE IF EXISTS \"{}\";", n)); }
                }
                if parts.is_empty() {
                    format!("-- 「{}」 not found in lake catalog", name)
                } else {
                    parts.join("\n")
                }
            },
        )
        .await
    }
}
