//! Smart routing - Auto-picks best tool for the job
//! Reduces tool selection errors by routing based on command/file analysis

use super::{file, shell, transform};
use anyhow::Result;
use serde_json::{json, Value};

/// Smart command execution - routes to best executor
pub async fn smart_exec(args: Value) -> Result<Value> {
    let command = args.get("command").and_then(|v| v.as_str()).unwrap_or("");
    let cwd = args.get("cwd").and_then(|v| v.as_str());
    let needs_env = args
        .get("needs_env")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    if command.is_empty() {
        return Ok(json!({"error": "command is required"}));
    }

    // Detect PowerShell syntax
    let needs_powershell = command.contains("$")
        || command.contains("Get-")
        || command.contains("Set-")
        || command.contains("New-Item")
        || command.contains("Remove-Item")
        || command.contains("Where-Object")
        || command.contains("-ErrorAction")
        || command.contains("Select-Object")
        || command.contains("Format-Table")
        || command.contains("ConvertTo-")
        || command.contains("ConvertFrom-");

    // Detect session needs
    let needs_session = needs_env
        || cwd.is_some()
        || command.contains("cargo ")
        || command.contains("npm ")
        || command.contains("pip ")
        || command.starts_with("cd ")
        || command.contains(" && cd ")
        || command.starts_with("set ")
        || command.starts_with("export ");

    let route: &str;

    if needs_session {
        route = "term_session_run";
        // Ensure session exists
        let session_name = "smart_default";
        let create_args = if let Some(dir) = cwd {
            json!({"name": session_name, "cwd": dir})
        } else {
            json!({"name": session_name})
        };
        let _ = shell::create_session(create_args).await;

        // Run in session
        let result = shell::execute(json!({
            "command": command,
            "session_id": session_name
        }))
        .await?;

        return Ok(json!({
            "routed_to": route,
            "result": result
        }));
    } else if needs_powershell {
        route = "powershell";
        // Escape quotes and wrap for PowerShell
        let ps_command = format!(
            "powershell -NoProfile -Command \"{}\"",
            command.replace("\"", "\\\"")
        );
        let result = shell::execute(json!({"command": ps_command})).await?;

        return Ok(json!({
            "routed_to": route,
            "result": result
        }));
    } else {
        route = "term_run";
        let result = shell::execute(json!({"command": command})).await?;

        return Ok(json!({
            "routed_to": route,
            "result": result
        }));
    }
}

/// Smart file read - routes to best reader
pub async fn smart_read(args: Value) -> Result<Value> {
    let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
    let find = args.get("find").and_then(|v| v.as_str());
    let lines = args.get("lines").and_then(|v| v.as_str());
    let compare_to = args.get("compare_to").and_then(|v| v.as_str());

    if path.is_empty() {
        return Ok(json!({"error": "path is required"}));
    }

    let route: &str;

    if let Some(pattern) = find {
        // Grep mode
        route = "term_grep";
        let result = transform::grep(json!({
            "path": path,
            "pattern": pattern,
            "context": 2
        }))
        .await?;

        return Ok(json!({
            "routed_to": route,
            "result": result
        }));
    } else if let Some(range) = lines {
        // Line extraction mode
        route = "term_extract_lines";
        let parts: Vec<&str> = range.split(':').collect();
        if parts.len() == 2 {
            let start: i64 = parts[0].parse().unwrap_or(1);
            let end: i64 = parts[1].parse().unwrap_or(-1);
            let result = transform::extract_lines(json!({
                "path": path,
                "start": start,
                "end": end
            }))
            .await?;

            return Ok(json!({
                "routed_to": route,
                "result": result
            }));
        } else {
            return Ok(json!({"error": "lines format: 'start:end' e.g. '50:100'"}));
        }
    } else if let Some(other) = compare_to {
        // Diff mode
        route = "diff_files";
        let result = transform::diff_files(json!({
            "file_a": path,
            "file_b": other
        }))
        .await?;

        return Ok(json!({
            "routed_to": route,
            "result": result
        }));
    } else {
        // Default: basic file read with optional truncation
        route = "term_read_file";
        let max_kb = args.get("max_kb").and_then(|v| v.as_u64()).unwrap_or(100);

        // Read file with size check
        let result = file::read_file(json!({
            "path": path,
            "max_bytes": max_kb * 1024
        }))
        .await?;

        return Ok(json!({
            "routed_to": route,
            "result": result
        }));
    }
}
