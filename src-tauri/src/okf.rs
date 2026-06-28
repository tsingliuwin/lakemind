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

/// Extract the text block of a heading from the file content
pub fn parse_okf_block_from_content(content: &str, heading: &str) -> Option<String> {
    let lines: Vec<&str> = content.lines().collect();
    let mut block_content = Vec::new();
    let mut recording = false;

    for line in lines {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            let heading_text = trimmed.trim_start_matches('#').trim();
            if recording {
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
        Some(block_content.join("\n").trim().to_string())
    } else {
        None
    }
}

/// Parse specific block section out of OKF Markdown document by header
pub fn read_okf_block(
    ws_path: &str,
    category: &str,
    name: &str,
    heading: &str,
) -> Result<String, String> {
    let okf_dir = get_okf_dir(ws_path);
    let mut file_path = okf_dir.join(category).join(format!("{}.md", name));
    if !file_path.exists() {
        let candidates = [
            okf_dir.join("tables").join(format!("{}.md", name)),
            okf_dir.join("views").join(format!("{}.md", name)),
            okf_dir.join("sources").join(format!("{}.md", name)),
        ];
        let mut found = false;
        for c in &candidates {
            if c.exists() {
                file_path = c.clone();
                found = true;
                break;
            }
        }
        if !found {
            return Err(format!("文件不存在: {:?}", file_path));
        }
    }

    let content = fs::read_to_string(&file_path)
        .map_err(|e| format!("读取文件失败: {}", e))?;

    parse_okf_block_from_content(&content, heading)
        .ok_or_else(|| format!("未找到标题为 '{}' 的板块", heading))
}

pub fn write_okf_block(
    ws_path: &str,
    category: &str,
    name: &str,
    heading: &str,
    new_content: &str,
) -> Result<(), String> {
    let okf_dir = get_okf_dir(ws_path);
    let mut file_path = okf_dir.join(category).join(format!("{}.md", name));
    if !file_path.exists() {
        let candidates = [
            okf_dir.join("tables").join(format!("{}.md", name)),
            okf_dir.join("views").join(format!("{}.md", name)),
            okf_dir.join("sources").join(format!("{}.md", name)),
        ];
        for c in &candidates {
            if c.exists() {
                file_path = c.clone();
                break;
            }
        }
    }

    let content = if file_path.exists() {
        fs::read_to_string(&file_path).map_err(|e| format!("读取文件失败: {}", e))?
    } else {
        if let Some(parent) = file_path.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("创建目录失败: {}", e))?;
        }
        let doc_type = match category {
            "tables" => "DuckDB Table",
            "views" => "DuckDB View",
            "sources" => "Data Source",
            "concepts" => "Business Concept",
            _ => "Concept",
        };
        format!(
            "---\ntype: {}\ntitle: {}\ndescription: 自动初始化的 OKF 文档\n---\n",
            doc_type, name
        )
    };

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
        // Detect preferred heading level from file style
        let mut level = if category == "concepts" { 2 } else { 1 };
        for line in &new_lines {
            let trimmed = line.trim();
            if trimmed.starts_with('#') {
                let current_level = trimmed.chars().take_while(|&c| c == '#').count();
                if current_level > 1 {
                    level = current_level;
                    break;
                }
            }
        }
        let prefix = "#".repeat(level);

        // Heading not found, append to end
        new_lines.push("".to_string());
        new_lines.push(format!("{} {}", prefix, heading));
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

    // Check tables/ first, then views/, then sources/
    let okf_dir = get_okf_dir(ws_path);
    let mut file_path = okf_dir.join("tables").join(format!("{}.md", table_name));
    if !file_path.exists() {
        file_path = okf_dir.join("views").join(format!("{}.md", table_name));
    }
    if !file_path.exists() {
        file_path = okf_dir.join("sources").join(format!("{}.md", table_name));
    }
    if !file_path.exists() {
        return (desc, col_comments, relations);
    }

    let Ok(content) = fs::read_to_string(&file_path) else {
        return (desc, col_comments, relations);
    };

    // Find title in YAML frontmatter
    desc = parse_yaml_field(&content, "title");

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

/// Retroactively generate OKF markdown files for existing SQLite object defs and sources.
pub fn ensure_all_object_okf(ws_path: &str) {
    let Ok(sqlite) = crate::db::get_db_conn() else { return };
    
    // 1. Process sources
    if let Ok(sources) = crate::db::list_sources(&sqlite, ws_path) {
        for src in sources {
            let source_path = get_okf_dir(ws_path).join("sources").join(format!("{}.md", src.table_name));
            if !source_path.exists() {
                let _ = write_source_okf(
                    ws_path,
                    &src.table_name,
                    &src.label,
                    &src.file_path,
                    src.file_size,
                    src.file_mtime,
                    &src.columns,
                    src.row_count,
                );
            }
            let pipeline_path = get_okf_dir(ws_path).join("pipelines").join("specific").join(format!("{}_ingest.md", src.table_name));
            if !pipeline_path.exists() {
                let _ = write_pipeline_okf(
                    ws_path,
                    &src.table_name,
                    &src.label,
                    &src.file_path,
                    &src.storage,
                );
            }
        }
    }

    // 2. Process table/view object defs
    if let Ok(objs) = crate::db::list_object_defs(&sqlite, ws_path) {
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
                        let mut desc = parse_yaml_field(&content, "description").unwrap_or_default();
                        if desc.is_empty() {
                            if let Some(note) = parse_okf_block_from_content(&content, "探索备注") {
                                desc = note;
                            } else if let Some(biz_desc) = parse_okf_block_from_content(&content, "业务描述") {
                                desc = biz_desc;
                            }
                        }
                        sources_info.push(format!("- **源文件原始视图 `{}`** (业务名称: {}): {}", name, title, desc.replace('\n', " ")));
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
                        let mut desc = parse_yaml_field(&content, "description").unwrap_or_default();
                        if desc.is_empty() {
                            if let Some(biz_desc) = parse_okf_block_from_content(&content, "业务描述") {
                                desc = biz_desc;
                            } else if let Some(note) = parse_okf_block_from_content(&content, "探索备注") {
                                desc = note;
                            }
                        }
                        
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
                        let mut desc = parse_yaml_field(&content, "description").unwrap_or_default();
                        if desc.is_empty() {
                            if let Some(biz_desc) = parse_okf_block_from_content(&content, "业务描述") {
                                desc = biz_desc;
                            } else if let Some(note) = parse_okf_block_from_content(&content, "探索备注") {
                                desc = note;
                            }
                        }
                        
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

    // 4. Process concepts
    let concepts_dir = okf_dir.join("concepts");
    if concepts_dir.exists() {
        let mut concepts_info = Vec::new();
        if let Ok(entries) = fs::read_dir(concepts_dir) {
            for entry in entries.flatten() {
                if entry.path().extension().and_then(|e| e.to_str()) == Some("md") {
                    let name = entry.path().file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
                    if let Ok(content) = fs::read_to_string(entry.path()) {
                        let title = parse_yaml_field(&content, "title").unwrap_or_else(|| name.clone());
                        let desc = parse_yaml_field(&content, "description").unwrap_or_default();
                        
                        let mut concept_detail = format!("### 业务通用定义 `{}` (业务名称: {})\n", name, title);
                        if !desc.is_empty() {
                            concept_detail.push_str(&format!("> {}\n", desc));
                        }
                        
                        let mut current_heading = String::new();
                        let mut block_content = Vec::new();
                        let mut intro_content = Vec::new();
                        let mut in_yaml = false;
                        
                        for line in content.lines() {
                            let trimmed = line.trim();
                            if trimmed == "---" {
                                in_yaml = !in_yaml;
                                continue;
                            }
                            if in_yaml {
                                continue;
                            }
                            if trimmed.starts_with('#') {
                                let heading_text = trimmed.trim_start_matches('#').trim();
                                if heading_text.eq_ignore_ascii_case(&title) || heading_text.eq_ignore_ascii_case(&name) {
                                    continue;
                                }
                                if !current_heading.is_empty() {
                                    if !block_content.is_empty() {
                                        concept_detail.push_str(&format!("- **{}**:\n  {}\n", current_heading, block_content.join("\n  ")));
                                    }
                                } else {
                                    if !intro_content.is_empty() && desc.is_empty() {
                                        concept_detail.push_str(&format!("> {}\n", intro_content.join("\n> ")));
                                    }
                                }
                                current_heading = heading_text.to_string();
                                block_content.clear();
                            } else {
                                if !trimmed.is_empty() {
                                    if current_heading.is_empty() {
                                        intro_content.push(trimmed.to_string());
                                    } else {
                                        block_content.push(trimmed.to_string());
                                    }
                                }
                            }
                        }
                        if !current_heading.is_empty() && !block_content.is_empty() {
                            concept_detail.push_str(&format!("- **{}**:\n  {}\n", current_heading, block_content.join("\n  ")));
                        } else if current_heading.is_empty() && !intro_content.is_empty() && desc.is_empty() {
                            concept_detail.push_str(&format!("> {}\n", intro_content.join("\n> ")));
                        }
                        
                        concepts_info.push(concept_detail);
                    }
                }
            }
        }
        if !concepts_info.is_empty() {
            summary.push_str("# 业务通用概念与全局定义 (concepts)\n");
            summary.push_str(&concepts_info.join("\n"));
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

        // Test fallback to find it when wrong category is passed (e.g. "views")
        let block_fallback = read_okf_block(ws, "views", "test_table", "关联关系").unwrap();
        assert_eq!(block_fallback, "- 关联修改项");

        write_okf_block(ws, "views", "test_table", "关联关系", "- 关联修改项2").unwrap();
        let block_updated_fallback = read_okf_block(ws, "tables", "test_table", "关联关系").unwrap();
        assert_eq!(block_updated_fallback, "- 关联修改项2");

        // Test dynamic heading level detection
        let level2_file = temp.join("okf").join("tables").join("l2_table.md");
        let l2_content = "\
---
title: L2 Table
type: DuckDB Table
---
# L2 Table
## 业务描述
说明
";
        let _ = fs::write(&level2_file, l2_content);
        write_okf_block(ws, "tables", "l2_table", "新板块", "新内容").unwrap();
        let l2_file_content = fs::read_to_string(&level2_file).unwrap();
        assert!(l2_file_content.contains("## 新板块"));
        assert!(!l2_file_content.lines().any(|l| l.trim() == "# 新板块"));

        // Test business description parsing from markdown headers for sources
        let _ = fs::create_dir_all(temp.join("okf").join("sources"));
        let source_file = temp.join("okf").join("sources").join("test_source.md");
        let source_content = "\
---
title: 原始销售数据
type: Data Source
---
# 探索备注
这是从销售系统中导出的2026年数据。
";
        let _ = fs::write(&source_file, source_content);

        // Test concept intro and robust heading parsing
        let _ = fs::create_dir_all(temp.join("okf").join("concepts"));
        let concept_file_path = temp.join("okf").join("concepts").join("new_concept2.md");
        let concept_content = "\
---
title: 测试概念2
type: Business Concept
---
# 测试概念2
这是测试概念的引言部分，说明了公司的主要愿景。

## 详细板块
这是板块内容。
";
        let _ = fs::write(&concept_file_path, concept_content);

        // Test memory summary parser
        let summary = get_okf_memory_summary(ws);
        assert!(summary.contains("test_table"));
        assert!(summary.contains("Test Table"));
        assert!(summary.contains("Important sales info"));
        assert!(summary.contains("revenue"));
        
        // Assertions for raw source description parsing
        assert!(summary.contains("这是从销售系统中导出的2026年数据。"));
        
        // Assertions for concept intro and sections parsing
        assert!(summary.contains("这是测试概念的引言部分，说明了公司的主要愿景。"));
        assert!(summary.contains("详细板块"));
        assert!(summary.contains("这是板块内容。"));

        let _ = fs::remove_dir_all(temp);
    }
}
