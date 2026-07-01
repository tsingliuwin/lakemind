use serde::{Deserialize, Serialize};
use serde_json::json;
use rig_core::{completion::ToolDefinition, tool::Tool};

use super::super::error::ToolError;
use super::ddl_shared::{sanitize_ddl_ident, DdlToolShared};

#[derive(Deserialize, Serialize)]
pub(crate) struct CreateTableArgs {
    name: String,
    select_sql: String,
}

pub(crate) struct CreateTableTool {
    pub(crate) shared: DdlToolShared,
}

impl Tool for CreateTableTool {
    const NAME: &'static str = "create_table";
    type Error = ToolError;
    type Args = CreateTableArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "create_table".to_string(),
            description: "创建一张物理表（物化存储）来持久化加工后的数据。传入新表名与一条 SELECT 语句，结果会被物化写入。若同名表已存在会先删除再创建。命名建议用 t_ 前缀（最终表）或 tmp_ 前缀（中间表）。".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "新表名（遵循命名规范，如 t_sales、tmp_joined）" },
                    "select_sql": { "type": "string", "description": "用于生成该表的 SELECT 查询语句" }
                },
                "required": ["name", "select_sql"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let name = sanitize_ddl_ident(args.name.trim())?;
        let select_sql = args.select_sql.trim().to_string();
        if select_sql.is_empty() {
            return Err(ToolError("select_sql 不能为空".to_string()));
        }
        let summary_pending = format!("创建物理表 {}", name);
        self.shared.run(
            "create_table", "ct",
            json!({ "name": name, "select_sql": select_sql }),
            summary_pending,
            Some((name.clone(), select_sql.clone(), "table".to_string())),
            move || format!(
                "DROP TABLE IF EXISTS \"{name}\";\nCREATE TABLE \"{name}\" AS {select_sql};",
                name = name,
                select_sql = select_sql
            ),
        )
        .await
    }
}
