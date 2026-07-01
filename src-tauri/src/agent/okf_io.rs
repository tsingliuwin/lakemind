//! OKF knowledge-base file helpers used by the `tidy_okf_knowledge` tool:
//! parsing LLM-emitted file blocks, recursive copy/delete, and the safe
//! backup → rewrite → rollback routine.

use std::fs;
use std::path::Path;

/// Parse ```` ```okf-file ````-fenced blocks emitted by the LLM into
/// `(relative_path, content)` pairs. Used when reorganizing the OKF knowledge
/// base: each block's first line is `FILE: <path>`, the rest is the file body.
pub(super) fn parse_okf_files(output: &str) -> Vec<(String, String)> {
    let mut files = Vec::new();
    let marker = "```okf-file";
    let end_marker = "```";

    let mut cur = output;
    while let Some(start_idx) = cur.find(marker) {
        let content_start = start_idx + marker.len();
        let after_start = &cur[content_start..];
        if let Some(end_idx) = after_start.find(end_marker) {
            let block = &after_start[..end_idx];
            let block_trimmed = block.trim();
            if block_trimmed.starts_with("FILE:") {
                if let Some(line_end) = block_trimmed.find('\n') {
                    let file_line = &block_trimmed[..line_end];
                    let file_path = file_line.trim_start_matches("FILE:").trim().to_string();
                    let file_content = block_trimmed[line_end..].trim().to_string();
                    if !file_path.is_empty() && !file_content.is_empty() {
                        files.push((file_path, file_content));
                    }
                }
            }
            cur = &after_start[end_idx + end_marker.len()..];
        } else {
            break;
        }
    }
    files
}

fn copy_dir_all(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        if ty.is_dir() {
            copy_dir_all(&entry.path(), &dst.join(entry.file_name()))?;
        } else {
            fs::copy(entry.path(), dst.join(entry.file_name()))?;
        }
    }
    Ok(())
}

fn delete_md_files(dir: &Path) -> std::io::Result<()> {
    if !dir.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        if ty.is_dir() {
            delete_md_files(&entry.path())?;
            if fs::read_dir(entry.path())?.next().is_none() {
                let _ = fs::remove_dir(entry.path());
            }
        } else if entry.path().extension().and_then(|s| s.to_str()) == Some("md") {
            fs::remove_file(entry.path())?;
        }
    }
    Ok(())
}

/// Backup the OKF directory, then rewrite it with `new_files`. On any write
/// failure, roll back from the backup so the knowledge base is never left in a
/// partially-written state.
pub(super) fn backup_and_rewrite_okf(okf_dir: &Path, new_files: &[(String, String)]) -> Result<String, String> {
    let backup_dir = okf_dir.parent().unwrap_or(okf_dir).join("okf_backup_temp");
    if backup_dir.exists() {
        let _ = fs::remove_dir_all(&backup_dir);
    }
    if okf_dir.exists() {
        copy_dir_all(okf_dir, &backup_dir).map_err(|e| format!("备份失败: {}", e))?;
    }
    let res: Result<usize, String> = (|| {
        if okf_dir.exists() {
            delete_md_files(okf_dir).map_err(|e| e.to_string())?;
        }
        let mut written_count = 0;
        for (rel_path, content) in new_files {
            if rel_path.contains("..") || rel_path.starts_with('/') {
                return Err(format!("非法的文件路径: {}", rel_path));
            }
            let full_path = okf_dir.join(rel_path);
            if let Some(parent) = full_path.parent() {
                fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            fs::write(&full_path, content).map_err(|e| e.to_string())?;
            written_count += 1;
        }
        Ok(written_count)
    })();
    match res {
        Ok(written_count) => {
            if backup_dir.exists() {
                let _ = fs::remove_dir_all(&backup_dir);
            }
            Ok(format!("成功自动整理和提炼了本地知识库。重构写入了 {} 个干净的 OKF 知识文件。", written_count))
        }
        Err(e) => {
            if backup_dir.exists() {
                if okf_dir.exists() {
                    let _ = fs::remove_dir_all(okf_dir);
                }
                let _ = copy_dir_all(&backup_dir, okf_dir);
                let _ = fs::remove_dir_all(&backup_dir);
            }
            Err(format!("自动整理知识库失败，已安全回滚到整理前的状态。错误: {}", e))
        }
    }
}
