use serde::{Deserialize, Serialize};
use serde_json::json;
use rig_core::{completion::ToolDefinition, tool::Tool};

use super::super::error::ToolError;
use super::ddl_shared::{sanitize_ddl_ident, DdlToolShared};

#[derive(Deserialize, Serialize)]
pub(crate) struct CreateViewArgs {
    name: String,
    select_sql: String,
}

pub(crate) struct CreateViewTool {
    pub(crate) shared: DdlToolShared,
}

impl Tool for CreateViewTool {
    const NAME: &'static str = "create_view";
    type Error = ToolError;
    type Args = CreateViewArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "create_view".to_string(),
            description: "创建一个虚拟视图（零拷贝，不物化数据）来封装加工逻辑。传入新视图名与一条 SELECT 语句。若同名对象已存在会先删除再创建。命名建议用 v_ 前缀（最终视图）或 tmp_v_ 前缀（中间视图）。".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "新视图名（遵循命名规范，如 v_sales、tmp_v_filtered）" },
                    "select_sql": { "type": "string", "description": "定义该视图的 SELECT 查询语句" }
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
        let summary_pending = format!("创建视图 {}", name);
        self.shared.run(
            "create_view", "cv",
            json!({ "name": name, "select_sql": select_sql }),
            summary_pending,
            Some((name.clone(), select_sql.clone(), "view".to_string())),
            move || format!(
                "DROP VIEW IF EXISTS \"{name}\";\nDROP TABLE IF EXISTS \"{name}\";\nCREATE VIEW \"{name}\" AS {select_sql};",
                name = name,
                select_sql = select_sql
            ),
        )
        .await
    }
}
