use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use crate::model::ColumnInfo;

/// Get the root OKF directory: `<ws_path>/okf`
pub fn get_okf_dir(ws_path: &str) -> PathBuf {
    Path::new(ws_path).join("okf")
}

/// Ensure that all standard OKF subdirectories exist
pub fn ensure_okf_dirs(ws_path: &str) -> Result<PathBuf, String> {
    let okf_dir = get_okf_dir(ws_path);
    let subdirs = ["sources", "pipelines/specific", "pipelines/generic", "tables", "views", "concepts"];
    for sub in &subdirs {
        let path = okf_dir.join(sub);
        if !path.exists() {
            fs::create_dir_all(&path).map_err(|e| format!("无法创建目录 {:?}: {}", path, e))?;
        }
    }
    Ok(okf_dir)
}

/// Get current ISO 8601 timestamp string
fn current_timestamp() -> String {
    // Basic UTC timestamp format
    if let Ok(now) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        let secs = now.as_secs();
        let days = secs / 86400;
        let mut year = 1970;
        let mut days_rem = days;
        // Simplified leap year calculations
        loop {
            let leap = (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0);
            let size = if leap { 366 } else { 365 };
            if days_rem >= size {
                days_rem -= size;
                year += 1;
            } else {
                break;
            }
        }
        format!("{:04}-01-01T00:00:00Z", year) // Fallback stable string format
    } else {
        "2026-06-28T00:00:00Z".to_string()
    }
}

/// Write/update the source document under `sources/<table_name>.md`
pub fn write_source_okf(
    ws_path: &str,
    table_name: &str,
    label: &str,
    file_path: &str,
    file_size: i64,
    file_mtime: i64,
    columns: &[ColumnInfo],
    row_count: Option<i64>,
) -> Result<(), String> {
    let okf_dir = ensure_okf_dirs(ws_path)?;
    let file_path_buf = okf_dir.join("sources").join(format!("{}.md", table_name));
    
    let extension = Path::new(file_path)
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("unknown")
        .to_uppercase();

    let row_count_str = row_count
        .map(|c| c.to_string())
        .unwrap_or_else(|| "未知".to_string());

    let body = format!(
        "---\n\
        type: Raw Data Source\n\
        title: {}\n\
        resource: file://{}\n\
        fingerprint: {}:{}\n\
        tags: [raw]\n\
        timestamp: {}\n\
        ---\n\n\
        # 物理属性\n\
        - 格式：{}\n\
        - 大小：{} 字节\n\
        - 列数：{}\n\
        - 期望导入的物理表：`{}`\n\
        - 行数估算：{}\n\n\
        # 探索备注\n\
        本源数据文件自动注册于系统，物理表名映射为 `{}`。\n",
        label, file_path, file_mtime, file_size, current_timestamp(),
        extension, file_size, columns.len(), table_name, row_count_str, table_name
    );

    fs::write(&file_path_buf, body).map_err(|e| format!("写入 source OKF 失败: {}", e))?;
    Ok(())
}

/// Write/update the cleaning pipeline under `pipelines/specific/<table_name>_ingest.md`
pub fn write_pipeline_okf(
    ws_path: &str,
    table_name: &str,
    label: &str,
    file_path: &str,
    storage: &str,
) -> Result<(), String> {
    let okf_dir = ensure_okf_dirs(ws_path)?;
    let file_path_buf = okf_dir.join("pipelines").join("specific").join(format!("{}_ingest.md", table_name));

    let body = format!(
        "---\n\
        type: Specific Ingestion Recipe\n\
        title: {} 导入清洗配方\n\
        source: sources/{}\n\
        target_table: tables/{}\n\
        timestamp: {}\n\
        ---\n\n\
        # 导入 SQL 语句\n\
        ```sql\n\
        -- 自动生成的 DuckDB 数据加载指令\n\
        CREATE OR REPLACE TABLE \"{}\" AS SELECT * FROM '{}';\n\
        ```\n\n\
        # 配方配置参数\n\
        - 物理格式: {}\n\
        - 文件路径: {}\n\n\
        # 异常排障记录\n\
        - 暂无报错记录，自动导入成功。\n",
        label, table_name, table_name, current_timestamp(),
        table_name, file_path.replace('\'', "''"), storage, file_path
    );

    fs::write(&file_path_buf, body).map_err(|e| format!("写入 pipeline OKF 失败: {}", e))?;
    Ok(())
}

/// Write/update physical table metadata under `tables/<table_name>.md`
pub fn write_table_okf(
    ws_path: &str,
    table_name: &str,
    columns: &[ColumnInfo],
    row_count: Option<i64>,
) -> Result<(), String> {
    let okf_dir = ensure_okf_dirs(ws_path)?;
    let file_path_buf = okf_dir.join("tables").join(format!("{}.md", table_name));

    let mut schema_table = String::new();
    schema_table.push_str("| 字段名 | 物理类型 | 业务释义 | 数据约束 |\n");
    schema_table.push_str("|---|---|---|---|\n");
    for col in columns {
        let is_nullable = if col.null { "" } else { "NOT NULL" };
        schema_table.push_str(&format!(
            "| `{}` | {} | | {} |\n",
            col.name, col.r#type, is_nullable
        ));
    }

    let row_count_str = row_count
        .map(|c| c.to_string())
        .unwrap_or_else(|| "未知".to_string());

    let body = format!(
        "---\n\
        type: DuckDB Table\n\
        title: {} 物理数据表\n\
        resource: duckdb:///main/{}\n\
        timestamp: {}\n\
        ---\n\n\
        # 物理画像\n\
        - 行数估算: {}\n\n\
        # 字段 Schema\n\
        {}\n\n\
        # 数据质量校验规则\n\
        - 暂无约束规则。\n\n\
        # 关联关系\n\
        - 暂无关联表（请手动在此编辑，例如 `- customer_id 关联 [客户表](/tables/customers.md) 的 customer_id 字段`）。\n",
        table_name, table_name, current_timestamp(), row_count_str, schema_table
    );

    fs::write(&file_path_buf, body).map_err(|e| format!("写入 table OKF 失败: {}", e))?;
    Ok(())
}

/// Write/update logical view metadata under `views/<view_name>.md`
pub fn write_view_okf(
    ws_path: &str,
    view_name: &str,
    select_sql: &str,
    columns: &[ColumnInfo],
) -> Result<(), String> {
    let okf_dir = ensure_okf_dirs(ws_path)?;
    let file_path_buf = okf_dir.join("views").join(format!("{}.md", view_name));

    let upstreams = crate::fingerprint::extract_upstreams(select_sql);
    let deps_links: Vec<String> = upstreams
        .iter()
        .map(|name| {
            if name.starts_with("v_") || name.starts_with("tmp_v_") {
                format!("views/{}", name)
            } else {
                format!("tables/{}", name)
            }
        })
        .collect();
    let deps_str = serde_json::to_string(&deps_links).unwrap_or_else(|_| "[]".to_string());

    let mut schema_table = String::new();
    schema_table.push_str("| 字段名 | 物理类型 | 业务释义 | 数据约束 |\n");
    schema_table.push_str("|---|---|---|---|\n");
    for col in columns {
        let is_nullable = if col.null { "" } else { "NOT NULL" };
        schema_table.push_str(&format!(
            "| `{}` | {} | | {} |\n",
            col.name, col.r#type, is_nullable
        ));
    }

    let body = format!(
        "---\n\
        type: DuckDB View\n\
        title: {} 逻辑视图\n\
        dependencies: {}\n\
        timestamp: {}\n\
        ---\n\n\
        # 视图 SQL 定义\n\
        ```sql\n\
        {}\n\
        ```\n\n\
        # 字段 Schema\n\
        {}\n\n\
        # 依赖血缘\n\
        上游数据表：{}\n",
        view_name, deps_str, current_timestamp(), select_sql, schema_table,
        upstreams.iter().map(|n| format!("`{}`", n)).collect::<Vec<_>>().join(", ")
    );

    fs::write(&file_path_buf, body).map_err(|e| format!("写入 view OKF 失败: {}", e))?;
    Ok(())
}

/// Delete related OKF files for a table or view when it is deleted/dropped
pub fn delete_okf_files(ws_path: &str, name: &str) -> Result<(), String> {
    let okf_dir = get_okf_dir(ws_path);
    let files = [
        okf_dir.join("sources").join(format!("{}.md", name)),
        okf_dir.join("pipelines").join("specific").join(format!("{}_ingest.md", name)),
        okf_dir.join("tables").join(format!("{}.md", name)),
        okf_dir.join("views").join(format!("{}.md", name)),
    ];
    for f in &files {
        if f.exists() {
            let _ = fs::remove_file(f);
        }
    }
    Ok(())
}

/// Parse specific block section out of OKF Markdown document by header
pub fn read_okf_block(
    ws_path: &str,
    category: &str,
    name: &str,
    heading: &str,
) -> Result<String, String> {
    let okf_dir = get_okf_dir(ws_path);
    let file_path = okf_dir.join(category).join(format!("{}.md", name));
    if !file_path.exists() {
        return Err(format!("文件不存在: {:?}", file_path));
    }

    let content = fs::read_to_string(&file_path)
        .map_err(|e| format!("读取文件失败: {}", e))?;

    let lines: Vec<&str> = content.lines().collect();
    let mut block_content = Vec::new();
    let mut recording = false;

    for line in lines {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            let _level = trimmed.chars().take_while(|&c| c == '#').count();
            let heading_text = trimmed.trim_start_matches('#').trim();
            if recording {
                // Stopped by next heading of equal or higher importance (or any heading for simplicity)
                break;
            }
            if heading_text.eq_ignore_ascii_case(heading) {
                recording = true;
                continue;
            }
        }
        if recording {
            block_content.push(line);
        }
    }

    if recording {
        Ok(block_content.join("\n").trim().to_string())
    } else {
        Err(format!("未找到标题为 '{}' 的板块", heading))
    }
}

/// Rewrite a specific block heading section in an OKF markdown document
pub fn write_okf_block(
    ws_path: &str,
    category: &str,
    name: &str,
    heading: &str,
    new_content: &str,
) -> Result<(), String> {
    let okf_dir = get_okf_dir(ws_path);
    let file_path = okf_dir.join(category).join(format!("{}.md", name));
    if !file_path.exists() {
        return Err(format!("文件不存在: {:?}", file_path));
    }

    let content = fs::read_to_string(&file_path)
        .map_err(|e| format!("读取文件失败: {}", e))?;

    let lines: Vec<&str> = content.lines().collect();
    let mut new_lines: Vec<String> = Vec::new();
    let mut skipped = false;
    let mut written = false;

    for line in lines {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            let heading_text = trimmed.trim_start_matches('#').trim();
            if skipped {
                // Ended the skipped block, resume recording
                skipped = false;
            }
            if heading_text.eq_ignore_ascii_case(heading) {
                new_lines.push(line.to_string()); // Keep the header line
                new_lines.push(new_content.to_string()); // Insert new content
                skipped = true;
                written = true;
                continue;
            }
        }
        if !skipped {
            new_lines.push(line.to_string());
        }
    }

    if !written {
        // Heading not found, append to end
        new_lines.push("".to_string());
        new_lines.push(format!("# {}", heading));
        new_lines.push(new_content.to_string());
    }

    fs::write(&file_path, new_lines.join("\n")).map_err(|e| format!("写入文件失败: {}", e))?;
    Ok(())
}

/// Scan specific table/view OKF to parse comments and relations
pub fn parse_column_semantics(
    ws_path: &str,
    table_name: &str,
) -> (Option<String>, HashMap<String, String>, Vec<String>) {
    let mut desc = None;
    let mut col_comments = HashMap::new();
    let mut relations = Vec::new();

    // Check tables/ first, then views/
    let okf_dir = get_okf_dir(ws_path);
    let mut file_path = okf_dir.join("tables").join(format!("{}.md", table_name));
    if !file_path.exists() {
        file_path = okf_dir.join("views").join(format!("{}.md", table_name));
    }
    if !file_path.exists() {
        return (desc, col_comments, relations);
    }

    let Ok(content) = fs::read_to_string(&file_path) else {
        return (desc, col_comments, relations);
    };

    // Find title in YAML frontmatter
    for line in content.lines() {
        if line.starts_with("title:") {
            desc = Some(line.trim_start_matches("title:").trim().to_string());
            break;
        }
    }

    // Parse sections
    let lines: Vec<&str> = content.lines().collect();
    let mut current_heading = "";
    for line in lines {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            current_heading = trimmed.trim_start_matches('#').trim();
            continue;
        }

        if current_heading == "字段 Schema" || current_heading == "Column Schema" {
            // Parse Markdown Table: | column | type | meaning | constraints |
            if trimmed.starts_with('|') && !trimmed.contains("---|---") && !trimmed.contains("字段名") && !trimmed.contains("Column") {
                let parts: Vec<&str> = trimmed.split('|').map(|p| p.trim()).collect();
                if parts.len() >= 4 {
                    let col_name = parts[1].trim_matches('`').trim().to_string();
                    let meaning = parts[3].to_string();
                    if !col_name.is_empty() && !meaning.is_empty() {
                        col_comments.insert(col_name, meaning);
                    }
                }
            }
        } else if current_heading == "关联关系" || current_heading == "Relationships" {
            if trimmed.starts_with('-') || trimmed.starts_with('*') {
                let rel = trimmed.trim_start_matches(|c| c == '-' || c == '*' || c == ' ').to_string();
                if !rel.is_empty() {
                    relations.push(rel);
                }
            }
        }
    }

    (desc, col_comments, relations)
}

/// Helper to parse a single YAML frontmatter field value
pub fn parse_yaml_field(content: &str, field: &str) -> Option<String> {
    let mut in_yaml = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed == "---" {
            if in_yaml {
                break;
            } else {
                in_yaml = true;
                continue;
            }
        }
        if in_yaml {
            let parts: Vec<&str> = trimmed.splitn(2, ':').collect();
            if parts.len() == 2 && parts[0].trim().eq_ignore_ascii_case(field) {
                return Some(parts[1].trim().to_string());
            }
        }
    }
    None
}

/// Retroactively generate OKF markdown files for existing SQLite object defs.
pub fn ensure_all_object_okf(ws_path: &str) {
    let Ok(sqlite) = crate::db::get_db_conn() else { return };
    let Ok(objs) = crate::db::list_object_defs(&sqlite, ws_path) else { return };
    for obj in objs {
        if obj.kind == "table" {
            let file_path = get_okf_dir(ws_path).join("tables").join(format!("{}.md", obj.table_name));
            if !file_path.exists() {
                let _ = write_table_okf(ws_path, &obj.table_name, &obj.columns, obj.row_count);
            }
        } else {
            let file_path = get_okf_dir(ws_path).join("views").join(format!("{}.md", obj.table_name));
            if !file_path.exists() {
                let _ = write_view_okf(ws_path, &obj.table_name, &obj.select_sql, &obj.columns);
            }
        }
    }
}

/// Generate a concise summary of all registered OKF knowledge to serve as the agent's active memory context.
pub fn get_okf_memory_summary(ws_path: &str) -> String {
    ensure_all_object_okf(ws_path);
    let okf_dir = get_okf_dir(ws_path);
    let mut summary = String::new();

    // 1. Process sources
    let sources_dir = okf_dir.join("sources");
    if sources_dir.exists() {
        let mut sources_info = Vec::new();
        if let Ok(entries) = fs::read_dir(sources_dir) {
            for entry in entries.flatten() {
                if entry.path().extension().and_then(|e| e.to_str()) == Some("md") {
                    let name = entry.path().file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
                    if let Ok(content) = fs::read_to_string(entry.path()) {
                        let title = parse_yaml_field(&content, "title").unwrap_or_else(|| name.clone());
                        let desc = parse_yaml_field(&content, "description").unwrap_or_default();
                        sources_info.push(format!("- **源文件原始视图 `{}`** (业务名称: {}): {}", name, title, desc));
                    }
                }
            }
        }
        if !sources_info.is_empty() {
            summary.push_str("# 已在湖仓中注册的原始数据源 (sources)\n");
            summary.push_str(&sources_info.join("\n"));
            summary.push_str("\n\n");
        }
    }

    // 2. Process tables
    let tables_dir = okf_dir.join("tables");
    if tables_dir.exists() {
        let mut tables_info = Vec::new();
        if let Ok(entries) = fs::read_dir(tables_dir) {
            for entry in entries.flatten() {
                if entry.path().extension().and_then(|e| e.to_str()) == Some("md") {
                    let name = entry.path().file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
                    if let Ok(content) = fs::read_to_string(entry.path()) {
                        let title = parse_yaml_field(&content, "title").unwrap_or_else(|| name.clone());
                        let desc = parse_yaml_field(&content, "description").unwrap_or_default();
                        
                        let (_, col_comments, relations) = parse_column_semantics(ws_path, &name);
                        let mut table_detail = format!("### 数据物理表 `{}` (业务名称: {})\n", name, title);
                        if !desc.is_empty() {
                            table_detail.push_str(&format!("> {}\n", desc));
                        }
                        
                        if !col_comments.is_empty() {
                            table_detail.push_str("- 字段定义与业务释义:\n");
                            for (col, comment) in col_comments {
                                table_detail.push_str(&format!("  - `{}`: {}\n", col, comment));
                            }
                        }
                        if !relations.is_empty() {
                            table_detail.push_str("- 表关联关系:\n");
                            for rel in relations {
                                table_detail.push_str(&format!("  - {}\n", rel));
                            }
                        }
                        tables_info.push(table_detail);
                    }
                }
            }
        }
        if !tables_info.is_empty() {
            summary.push_str("# 物理表及其字段与关系记忆 (tables)\n");
            summary.push_str(&tables_info.join("\n"));
            summary.push_str("\n");
        }
    }

    // 3. Process views
    let views_dir = okf_dir.join("views");
    if views_dir.exists() {
        let mut views_info = Vec::new();
        if let Ok(entries) = fs::read_dir(views_dir) {
            for entry in entries.flatten() {
                if entry.path().extension().and_then(|e| e.to_str()) == Some("md") {
                    let name = entry.path().file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
                    if let Ok(content) = fs::read_to_string(entry.path()) {
                        let title = parse_yaml_field(&content, "title").unwrap_or_else(|| name.clone());
                        let desc = parse_yaml_field(&content, "description").unwrap_or_default();
                        
                        let (_, col_comments, relations) = parse_column_semantics(ws_path, &name);
                        let mut view_detail = format!("### 逻辑分层视图 `{}` (业务名称: {})\n", name, title);
                        if !desc.is_empty() {
                            view_detail.push_str(&format!("> {}\n", desc));
                        }
                        
                        if !col_comments.is_empty() {
                            view_detail.push_str("- 字段定义与业务释义:\n");
                            for (col, comment) in col_comments {
                                view_detail.push_str(&format!("  - `{}`: {}\n", col, comment));
                            }
                        }
                        if !relations.is_empty() {
                            view_detail.push_str("- 视图关联关系:\n");
                            for rel in relations {
                                view_detail.push_str(&format!("  - {}\n", rel));
                            }
                        }
                        views_info.push(view_detail);
                    }
                }
            }
        }
        if !views_info.is_empty() {
            summary.push_str("# 逻辑视图及其业务逻辑与释义记忆 (views)\n");
            summary.push_str(&views_info.join("\n"));
            summary.push_str("\n");
        }
    }

    summary
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_okf_block_read_write() {
        let temp = std::env::temp_dir().join("okf_test_ws");
        let ws = temp.to_str().unwrap();
        let _ = fs::create_dir_all(temp.join("okf").join("tables"));
        
        let file_path = temp.join("okf").join("tables").join("test_table.md");
        let content = "\
---
type: DuckDB Table
title: Test Table
description: Important sales info
---

# 字段 Schema
| 字段名 | 类型 | 允许空 | 释义 |
|---|---|---|---|
| revenue | DOUBLE | YES | 销售额金额 |

# 关联关系
- 通过 order_id 关联 v_orders.id
";
        
        let _ = fs::write(&file_path, content);
        
        let block = read_okf_block(ws, "tables", "test_table", "关联关系").unwrap();
        assert!(block.contains("order_id"));

        write_okf_block(ws, "tables", "test_table", "关联关系", "- 关联修改项").unwrap();
        
        let block_updated = read_okf_block(ws, "tables", "test_table", "关联关系").unwrap();
        assert_eq!(block_updated, "- 关联修改项");

        // Test memory summary parser
        let summary = get_okf_memory_summary(ws);
        assert!(summary.contains("test_table"));
        assert!(summary.contains("Test Table"));
        assert!(summary.contains("Important sales info"));
        assert!(summary.contains("revenue"));

        let _ = fs::remove_dir_all(temp);
    }
}
