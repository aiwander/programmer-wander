//! File Operations

use anyhow::Result;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader};
use std::path::Path;
use tokio::fs;
use tracing::info;

/// Read file with search/lines/max_kb support (enhanced to match mcp-windows raw_read)
pub async fn read_file(args: Value) -> Result<Value> {
    let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
    let search = args.get("search").and_then(|v| v.as_str());
    let lines_param = args.get("lines").and_then(|v| v.as_str());
    let max_kb = args.get("max_kb").and_then(|v| v.as_i64()).unwrap_or(100);
    // Legacy offset/length support
    let offset = args.get("offset").and_then(|v| v.as_i64()).unwrap_or(0);
    let length = args.get("length").and_then(|v| v.as_i64()).unwrap_or(-1);

    if path.is_empty() {
        anyhow::bail!("path is required");
    }

    let file_path = Path::new(path);
    if !file_path.exists() {
        anyhow::bail!("File not found: {}", path);
    }

    let metadata = std::fs::metadata(path)?;
    let file_size = metadata.len();
    let file_kb = file_size / 1024;

    // SEARCH MODE: grep for pattern
    if let Some(pattern) = search {
        let file = std::fs::File::open(path)?;
        let reader = BufReader::new(file);
        let pattern_lower = pattern.to_lowercase();
        let mut matches: Vec<String> = Vec::new();
        let mut total_lines = 0;

        for (i, line) in reader.lines().enumerate() {
            total_lines = i + 1;
            if let Ok(text) = line {
                if text.to_lowercase().contains(&pattern_lower) {
                    matches.push(format!("{}:{}", i + 1, text));
                }
            }
            if matches.len() >= 100 {
                matches.push("[...truncated at 100 matches]".to_string());
                break;
            }
        }

        if matches.is_empty() {
            return Ok(json!(format!(
                "[NO MATCHES] '{}' not found in {} lines",
                pattern, total_lines
            )));
        } else {
            return Ok(json!(format!(
                "[{} matches in {} lines]\n{}",
                matches.len(),
                total_lines,
                matches.join("\n")
            )));
        }
    }

    // LINES MODE: extract specific line range like "50:100"
    if let Some(range) = lines_param {
        let parts: Vec<&str> = range.split(':').collect();
        if parts.len() != 2 {
            anyhow::bail!("lines format: 'start:end' e.g. '50:100'");
        }

        let start: usize = parts[0].parse().unwrap_or(1);
        let end: usize = parts[1].parse().unwrap_or(50);

        if start < 1 || end < start {
            anyhow::bail!("Invalid line range");
        }

        let file = std::fs::File::open(path)?;
        let reader = BufReader::new(file);
        let mut result: Vec<String> = Vec::new();
        let mut total_lines = 0;

        for (i, line) in reader.lines().enumerate() {
            let line_num = i + 1;
            total_lines = line_num;

            if line_num >= start && line_num <= end {
                if let Ok(text) = line {
                    result.push(format!("{}:{}", line_num, text));
                }
            }

            if line_num > end {
                break;
            }
        }

        return Ok(json!(format!(
            "[Lines {}-{} of {}]\n{}",
            start,
            end.min(total_lines),
            total_lines,
            result.join("\n")
        )));
    }

    // FULL READ with size check and legacy offset/length support
    let content = fs::read_to_string(path).await?;
    let lines: Vec<&str> = content.lines().collect();
    let total_lines = lines.len();

    // Handle legacy offset/length params
    if offset != 0 || length > 0 {
        let result = if offset < 0 {
            let start = (total_lines as i64 + offset).max(0) as usize;
            lines[start..].join("\n")
        } else {
            let start = (offset as usize).min(total_lines);
            let end = if length < 0 {
                total_lines
            } else {
                (start + length as usize).min(total_lines)
            };
            lines[start..end].join("\n")
        };

        return Ok(json!({
            "success": true,
            "content": result,
            "line_count": total_lines,
            "lines_returned": result.lines().count(),
            "truncated": length > 0 && (offset as usize + length as usize) < total_lines,
            "path": path
        }));
    }

    // Auto-truncate large files
    if file_kb > max_kb as u64 {
        let chars_limit = (max_kb * 1024) as usize;
        let truncated: String = content.chars().take(chars_limit).collect();
        let shown_lines = truncated.lines().count();
        return Ok(json!(format!(
            "{}\n\n[TRUNCATED: {}KB file, showed {}/{} lines. Use search='pattern' or lines='start:end' for specific content]",
            truncated, file_kb, shown_lines, total_lines
        )));
    }

    Ok(json!({
        "success": true,
        "content": content,
        "line_count": total_lines,
        "path": path
    }))
}

/// Write file with auto-directory creation
pub async fn write_file(args: Value) -> Result<Value> {
    let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
    let content = args.get("content").and_then(|v| v.as_str()).unwrap_or("");
    let mode = args
        .get("mode")
        .and_then(|v| v.as_str())
        .unwrap_or("rewrite");

    if path.is_empty() {
        anyhow::bail!("path is required");
    }

    // Create parent directories
    if let Some(parent) = Path::new(path).parent() {
        fs::create_dir_all(parent).await?;
    }

    if mode == "append" {
        let existing = fs::read_to_string(path).await.unwrap_or_default();
        fs::write(path, format!("{}{}", existing, content)).await?;
    } else {
        fs::write(path, content).await?;
    }

    info!("Wrote {} bytes to {}", content.len(), path);

    Ok(json!({
        "success": true,
        "path": path,
        "lines_written": content.lines().count(),
        "size": content.len(),
        "mode": mode
    }))
}

/// Edit file with string replacement
pub async fn edit_block(args: Value) -> Result<Value> {
    let path = args.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
    let old_str = args
        .get("old_string")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let new_str = args
        .get("new_string")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let expected = args
        .get("expected_replacements")
        .and_then(|v| v.as_i64())
        .unwrap_or(1);

    if path.is_empty() || old_str.is_empty() {
        anyhow::bail!("file_path and old_string are required");
    }

    let content = fs::read_to_string(path).await?;
    let count = content.matches(old_str).count();

    if count == 0 {
        // Find close matches for better error message
        let lines: Vec<&str> = content.lines().collect();
        let mut close_matches = Vec::new();

        for (i, line) in lines.iter().enumerate() {
            if line.contains(&old_str[..old_str.len().min(20).max(5)]) {
                close_matches.push(json!({
                    "line": i + 1,
                    "content": line.chars().take(100).collect::<String>()
                }));
                if close_matches.len() >= 3 {
                    break;
                }
            }
        }

        return Ok(json!({
            "success": false,
            "error": "String not found in file",
            "close_matches": close_matches
        }));
    }

    if expected > 0 && count != expected as usize {
        return Ok(json!({
            "success": false,
            "error": format!("Expected {} replacements, found {}", expected, count)
        }));
    }

    let new_content = content.replace(old_str, new_str);
    fs::write(path, &new_content).await?;

    info!("Edited {} with {} replacements", path, count);

    Ok(json!({
        "success": true,
        "path": path,
        "replacements": count,
        "size": new_content.len()
    }))
}

/// Copy file
pub async fn copy_file(args: Value) -> Result<Value> {
    let src = args.get("source").and_then(|v| v.as_str()).unwrap_or("");
    let dst = args
        .get("destination")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if src.is_empty() || dst.is_empty() {
        anyhow::bail!("source and destination are required");
    }

    if let Some(parent) = Path::new(dst).parent() {
        fs::create_dir_all(parent).await?;
    }

    let bytes = fs::copy(src, dst).await?;

    Ok(json!({
        "success": true,
        "source": src,
        "destination": dst,
        "bytes": bytes
    }))
}

/// Move/rename file
pub async fn move_file(args: Value) -> Result<Value> {
    let src = args.get("source").and_then(|v| v.as_str()).unwrap_or("");
    let dst = args
        .get("destination")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if src.is_empty() || dst.is_empty() {
        anyhow::bail!("source and destination are required");
    }

    if let Some(parent) = Path::new(dst).parent() {
        fs::create_dir_all(parent).await?;
    }

    fs::rename(src, dst).await?;

    Ok(json!({
        "success": true,
        "source": src,
        "destination": dst
    }))
}

/// Get file metadata
pub async fn get_file_info(args: Value) -> Result<Value> {
    let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");

    if path.is_empty() {
        anyhow::bail!("path is required");
    }

    let meta = fs::metadata(path).await?;
    let modified = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs());

    Ok(json!({
        "success": true,
        "path": path,
        "size": meta.len(),
        "is_file": meta.is_file(),
        "is_dir": meta.is_dir(),
        "modified_unix": modified,
        "readonly": meta.permissions().readonly()
    }))
}

/// Create directory recursively
pub async fn create_directory(args: Value) -> Result<Value> {
    let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");

    if path.is_empty() {
        anyhow::bail!("path is required");
    }

    fs::create_dir_all(path).await?;

    Ok(json!({
        "success": true,
        "path": path
    }))
}

/// List directory contents
pub async fn list_directory(args: Value) -> Result<Value> {
    let path = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
    let depth = args.get("depth").and_then(|v| v.as_u64()).unwrap_or(2) as u32;

    async fn list_recursive(path: &Path, depth: u32, current: u32) -> Vec<Value> {
        let mut entries = Vec::new();

        if let Ok(mut dir) = fs::read_dir(path).await {
            while let Ok(Some(entry)) = dir.next_entry().await {
                let entry_path = entry.path();
                let name = entry.file_name().to_string_lossy().to_string();

                // Skip hidden files and node_modules
                if name.starts_with('.') || name == "node_modules" {
                    continue;
                }

                let is_dir = entry_path.is_dir();
                let mut item = json!({
                    "name": name,
                    "path": entry_path.to_string_lossy(),
                    "type": if is_dir { "dir" } else { "file" }
                });

                if !is_dir {
                    if let Ok(meta) = entry_path.metadata() {
                        item["size"] = json!(meta.len());
                    }
                }

                if is_dir && current < depth {
                    let children = Box::pin(list_recursive(&entry_path, depth, current + 1)).await;
                    if !children.is_empty() {
                        item["children"] = json!(children);
                    }
                }

                entries.push(item);
            }
        }

        // Sort: directories first, then by name
        entries.sort_by(|a, b| {
            let a_dir = a.get("type").and_then(|v| v.as_str()) == Some("dir");
            let b_dir = b.get("type").and_then(|v| v.as_str()) == Some("dir");
            match (a_dir, b_dir) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => a
                    .get("name")
                    .and_then(|v| v.as_str())
                    .cmp(&b.get("name").and_then(|v| v.as_str())),
            }
        });

        entries
    }

    let entries = list_recursive(Path::new(path), depth, 0).await;

    Ok(json!({
        "success": true,
        "path": path,
        "entries": entries,
        "total_entries": entries.len()
    }))
}

pub async fn append_file(args: Value) -> Result<Value> {
    let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
    let content = args.get("content").and_then(|v| v.as_str()).unwrap_or("");

    if path.is_empty() {
        anyhow::bail!("path is required");
    }

    use std::fs::OpenOptions;
    use std::io::Write;

    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    file.write_all(content.as_bytes())?;

    Ok(json!({"success": true, "path": path, "bytes_appended": content.len()}))
}
