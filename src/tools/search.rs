//! File Search Operations

use anyhow::Result;
use regex::RegexBuilder;
use serde_json::{json, Value};
use std::path::Path;
use tokio::fs;
use tracing::info;

/// Search for files by name or content
pub async fn search(args: Value) -> Result<Value> {
    let path = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
    let pattern = args.get("pattern").and_then(|v| v.as_str()).unwrap_or("");
    let search_type = args.get("search_type").and_then(|v| v.as_str()).unwrap_or("files");
    let file_pattern = args.get("file_pattern").and_then(|v| v.as_str());
    let ignore_case = args.get("ignore_case").and_then(|v| v.as_bool()).unwrap_or(true);
    let max_results = args.get("max_results").and_then(|v| v.as_u64()).unwrap_or(100) as usize;
    let context_lines = args.get("context_lines").and_then(|v| v.as_u64()).unwrap_or(2) as usize;
    
    if pattern.is_empty() {
        anyhow::bail!("pattern is required");
    }
    
    info!("Searching {} for '{}' (type={})", path, pattern, search_type);
    
    let regex = RegexBuilder::new(pattern)
        .case_insensitive(ignore_case)
        .build()
        .ok();
    
    let pattern_lower = pattern.to_lowercase();
    let search_content = search_type == "content";
    
    let mut results = Vec::new();
    search_recursive(
        Path::new(path), 
        &pattern_lower, 
        regex.as_ref(),
        search_content, 
        file_pattern,
        context_lines,
        max_results,
        &mut results
    ).await;
    
    Ok(json!({
        "success": true,
        "pattern": pattern,
        "search_type": search_type,
        "results": results,
        "count": results.len(),
        "truncated": results.len() >= max_results
    }))
}

#[async_recursion::async_recursion]
async fn search_recursive(
    path: &Path,
    pattern: &str,
    regex: Option<&regex::Regex>,
    search_content: bool,
    file_pattern: Option<&str>,
    context_lines: usize,
    max_results: usize,
    results: &mut Vec<Value>
) {
    if results.len() >= max_results {
        return;
    }
    
    let mut dir = match fs::read_dir(path).await {
        Ok(d) => d,
        Err(_) => return,
    };
    
    while let Ok(Some(entry)) = dir.next_entry().await {
        if results.len() >= max_results {
            return;
        }
        
        let entry_path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        
        // Skip hidden and common ignore patterns
        if name.starts_with('.') || 
           name == "node_modules" || 
           name == "target" || 
           name == "__pycache__" ||
           name == ".git" {
            continue;
        }
        
        if entry_path.is_dir() {
            Box::pin(search_recursive(
                &entry_path, 
                pattern, 
                regex,
                search_content, 
                file_pattern,
                context_lines,
                max_results,
                results
            )).await;
        } else {
            // Check file pattern filter
            if let Some(fp) = file_pattern {
                let matches = fp.split('|').any(|p| {
                    let p = p.trim();
                    if p.starts_with("*.") {
                        name.ends_with(&p[1..])
                    } else {
                        name.contains(p)
                    }
                });
                if !matches {
                    continue;
                }
            }
            
            if search_content {
                // Search file content
                if let Ok(content) = fs::read_to_string(&entry_path).await {
                    let matches = find_matches(&content, pattern, regex, context_lines);
                    if !matches.is_empty() {
                        results.push(json!({
                            "path": entry_path.to_string_lossy(),
                            "name": name,
                            "type": "content_match",
                            "matches": matches
                        }));
                    }
                }
            } else {
                // Search file name
                let name_lower = name.to_lowercase();
                let matches = if let Some(re) = regex {
                    re.is_match(&name_lower)
                } else {
                    name_lower.contains(pattern)
                };
                
                if matches {
                    let size = entry_path.metadata().map(|m| m.len()).unwrap_or(0);
                    results.push(json!({
                        "path": entry_path.to_string_lossy(),
                        "name": name,
                        "type": "file_match",
                        "size": size
                    }));
                }
            }
        }
    }
}

fn find_matches(content: &str, pattern: &str, regex: Option<&regex::Regex>, context_lines: usize) -> Vec<Value> {
    let lines: Vec<&str> = content.lines().collect();
    let mut matches = Vec::new();
    
    for (i, line) in lines.iter().enumerate() {
        let line_lower = line.to_lowercase();
        let is_match = if let Some(re) = regex {
            re.is_match(line)
        } else {
            line_lower.contains(pattern)
        };
        
        if is_match {
            // Get context
            let start = i.saturating_sub(context_lines);
            let end = (i + context_lines + 1).min(lines.len());
            let context: Vec<String> = lines[start..end]
                .iter()
                .enumerate()
                .map(|(j, l)| format!("{}: {}", start + j + 1, l))
                .collect();
            
            matches.push(json!({
                "line_number": i + 1,
                "line": line,
                "context": context
            }));
            
            if matches.len() >= 10 {
                break; // Limit matches per file
            }
        }
    }
    
    matches
}
