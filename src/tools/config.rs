//! Configuration and recovery tools

use anyhow::Result;
use once_cell::sync::Lazy;
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

/// Global config state
static CONFIG: Lazy<Mutex<Value>> = Lazy::new(|| {
    Mutex::new(json!({
        "blocked_commands": ["rm -rf /", "format", "del /s /q c:\\"],
        "default_shell": "powershell",
        "allowed_directories": [],
        "file_read_line_limit": 750,
        "file_write_line_limit": 2500,
        "telemetry_enabled": false
    }))
});

/// Usage statistics
static USAGE_STATS: Lazy<Mutex<Value>> = Lazy::new(|| {
    Mutex::new(json!({
        "total_calls": 0,
        "calls_by_tool": {},
        "start_time": chrono::Utc::now().to_rfc3339()
    }))
});

/// Recent tool calls
static RECENT_CALLS: Lazy<Mutex<Vec<Value>>> = Lazy::new(|| Mutex::new(Vec::new()));

/// Recovery data
static RECOVERY_DATA: Lazy<Mutex<Value>> = Lazy::new(|| {
    Mutex::new(json!({
        "sessions": [],
        "checkpoints": []
    }))
});

/// Get server configuration
pub async fn get_config() -> Result<Value> {
    let config = CONFIG.lock().unwrap();
    Ok(json!({
        "success": true,
        "config": config.clone()
    }))
}

/// Set configuration value
pub async fn set_config(args: Value) -> Result<Value> {
    let key = args["key"].as_str().unwrap_or("");
    let value = &args["value"];

    let mut config = CONFIG.lock().unwrap();

    match key {
        "blocked_commands" | "allowed_directories" => {
            if value.is_array() {
                config[key] = value.clone();
            }
        }
        "default_shell" => {
            if let Some(s) = value.as_str() {
                config[key] = json!(s);
            }
        }
        "file_read_line_limit" | "file_write_line_limit" => {
            if let Some(n) = value.as_i64() {
                config[key] = json!(n);
            }
        }
        "telemetry_enabled" => {
            if let Some(b) = value.as_bool() {
                config[key] = json!(b);
            }
        }
        _ => {
            return Ok(json!({
                "success": false,
                "error": format!("Unknown config key: {}", key)
            }));
        }
    }

    Ok(json!({
        "success": true,
        "key": key,
        "value": config[key].clone()
    }))
}

/// Reload configuration from disk
pub async fn reload_config() -> Result<Value> {
    // Look for config file
    let config_paths = [
        PathBuf::from("antigravity_config.json"),
        PathBuf::from(std::env::var("USERPROFILE").unwrap_or_default())
            .join("antigravity_config.json"),
    ];

    for path in &config_paths {
        if path.exists() {
            if let Ok(content) = fs::read_to_string(path) {
                if let Ok(new_config) = serde_json::from_str::<Value>(&content) {
                    let mut config = CONFIG.lock().unwrap();
                    *config = new_config;
                    return Ok(json!({
                        "success": true,
                        "source": path.to_string_lossy(),
                        "config": config.clone()
                    }));
                }
            }
        }
    }

    Ok(json!({
        "success": false,
        "error": "No config file found"
    }))
}

/// Get tool usage statistics
pub async fn get_usage_stats() -> Result<Value> {
    let stats = USAGE_STATS.lock().unwrap();
    Ok(json!({
        "success": true,
        "stats": stats.clone()
    }))
}

/// Record a tool call (internal use)
#[allow(dead_code)] // Planned metrics feature
pub fn record_tool_call(tool_name: &str, args: &Value, result: &Value) {
    // Update stats
    {
        let mut stats = USAGE_STATS.lock().unwrap();
        let total = stats["total_calls"].as_i64().unwrap_or(0) + 1;
        stats["total_calls"] = json!(total);

        let tool_count = stats["calls_by_tool"][tool_name].as_i64().unwrap_or(0) + 1;
        stats["calls_by_tool"][tool_name] = json!(tool_count);
    }

    // Add to recent calls
    {
        let mut calls = RECENT_CALLS.lock().unwrap();
        calls.push(json!({
            "tool": tool_name,
            "args": args.clone(),
            "success": result["success"].as_bool().unwrap_or(true),
            "timestamp": chrono::Utc::now().to_rfc3339()
        }));

        // Keep only last 100
        if calls.len() > 100 {
            calls.remove(0);
        }
    }
}

/// Get recent tool calls
pub async fn get_recent_calls(args: Value) -> Result<Value> {
    let max_results = args["max_results"].as_i64().unwrap_or(50) as usize;
    let tool_name = args["tool_name"].as_str();

    let calls = RECENT_CALLS.lock().unwrap();
    let filtered: Vec<&Value> = calls
        .iter()
        .rev()
        .filter(|c| tool_name.map_or(true, |t| c["tool"].as_str() == Some(t)))
        .take(max_results)
        .collect();

    Ok(json!({
        "success": true,
        "count": filtered.len(),
        "calls": filtered
    }))
}

/// Check recovery status
pub async fn recovery_status() -> Result<Value> {
    let recovery = RECOVERY_DATA.lock().unwrap();

    Ok(json!({
        "success": true,
        "recoverable_sessions": recovery["sessions"].as_array().map(|a| a.len()).unwrap_or(0),
        "pending_checkpoints": recovery["checkpoints"].as_array().map(|a| a.len()).unwrap_or(0),
        "data": recovery.clone()
    }))
}

/// Recover a crashed session
pub async fn recover_session(args: Value) -> Result<Value> {
    let session_id = args["session_id"].as_str().unwrap_or("");

    let recovery = RECOVERY_DATA.lock().unwrap();

    if let Some(sessions) = recovery["sessions"].as_array() {
        for session in sessions {
            if session["session_id"].as_str() == Some(session_id) {
                return Ok(json!({
                    "success": true,
                    "session": session.clone()
                }));
            }
        }
    }

    Ok(json!({
        "success": false,
        "error": format!("Session {} not found", session_id)
    }))
}

/// Resume interrupted operation
pub async fn resume_operation(args: Value) -> Result<Value> {
    let checkpoint_id = args["checkpoint_id"].as_str().unwrap_or("");

    let recovery = RECOVERY_DATA.lock().unwrap();

    if let Some(checkpoints) = recovery["checkpoints"].as_array() {
        for cp in checkpoints {
            if cp["checkpoint_id"].as_str() == Some(checkpoint_id) {
                return Ok(json!({
                    "success": true,
                    "checkpoint": cp.clone()
                }));
            }
        }
    }

    Ok(json!({
        "success": false,
        "error": format!("Checkpoint {} not found", checkpoint_id)
    }))
}

/// Clear all recovery data
pub async fn clear_recovery() -> Result<Value> {
    let mut recovery = RECOVERY_DATA.lock().unwrap();
    *recovery = json!({
        "sessions": [],
        "checkpoints": []
    });

    Ok(json!({
        "success": true,
        "message": "Recovery data cleared"
    }))
}

/// Save session for recovery (internal use)
#[allow(dead_code)] // Planned recovery feature
pub fn save_session_recovery(session_id: &str, shell: &str, cwd: &str, history: &[String]) {
    let mut recovery = RECOVERY_DATA.lock().unwrap();

    let session_data = json!({
        "session_id": session_id,
        "shell": shell,
        "cwd": cwd,
        "history": history,
        "saved_at": chrono::Utc::now().to_rfc3339()
    });

    if let Some(sessions) = recovery["sessions"].as_array_mut() {
        // Replace existing or add new
        let pos = sessions
            .iter()
            .position(|s| s["session_id"].as_str() == Some(session_id));
        if let Some(idx) = pos {
            sessions[idx] = session_data;
        } else {
            sessions.push(session_data);
        }
    }
}

/// Create checkpoint for long operation (internal use)
#[allow(dead_code)] // Planned checkpoint feature
pub fn create_checkpoint(operation: &str, progress: i32, total: i32, context: &Value) -> String {
    let checkpoint_id = format!("cp_{}", chrono::Utc::now().timestamp_millis());

    let mut recovery = RECOVERY_DATA.lock().unwrap();

    let checkpoint = json!({
        "checkpoint_id": checkpoint_id,
        "operation": operation,
        "progress": progress,
        "total": total,
        "context": context.clone(),
        "created_at": chrono::Utc::now().to_rfc3339()
    });

    if let Some(checkpoints) = recovery["checkpoints"].as_array_mut() {
        checkpoints.push(checkpoint);
    }

    checkpoint_id
}

// ============================================================================
// MCP CONFIG VALIDATION
// ============================================================================

/// Validate MCP configuration file
pub async fn validate_mcp_config(args: Value) -> Result<Value> {
    let config_path = args["config_path"].as_str();

    // Auto-detect config location
    let path = if let Some(p) = config_path {
        PathBuf::from(p)
    } else {
        let user_profile = std::env::var("APPDATA").unwrap_or_default();
        PathBuf::from(user_profile)
            .join("Claude")
            .join("claude_desktop_config.json")
    };

    if !path.exists() {
        return Ok(json!({
            "success": false,
            "error": format!("Config file not found: {}", path.display()),
            "searched_path": path.to_string_lossy()
        }));
    }

    // Read and parse
    let content = match fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) => {
            return Ok(json!({
                "success": false,
                "error": format!("Failed to read config: {}", e),
                "path": path.to_string_lossy()
            }))
        }
    };

    let config: Value = match serde_json::from_str(&content) {
        Ok(c) => c,
        Err(e) => {
            return Ok(json!({
                "success": false,
                "error": format!("Invalid JSON: {}", e),
                "path": path.to_string_lossy()
            }))
        }
    };

    let mut issues: Vec<String> = Vec::new();
    let mut servers_found: Vec<String> = Vec::new();
    let mut commands_checked: Vec<Value> = Vec::new();

    // Check structure
    if let Some(servers) = config.get("mcpServers").and_then(|s| s.as_object()) {
        for (name, server_config) in servers {
            servers_found.push(name.clone());

            // Check command exists
            if let Some(cmd) = server_config.get("command").and_then(|c| c.as_str()) {
                let cmd_path = PathBuf::from(cmd);
                let exists = cmd_path.exists();

                commands_checked.push(json!({
                    "server": name,
                    "command": cmd,
                    "exists": exists
                }));

                if !exists {
                    issues.push(format!("Server '{}': command not found: {}", name, cmd));
                }
            } else {
                issues.push(format!("Server '{}': missing 'command' field", name));
            }
        }
    } else {
        issues.push("Missing 'mcpServers' object".to_string());
    }

    Ok(json!({
        "success": issues.is_empty(),
        "path": path.to_string_lossy(),
        "servers_found": servers_found,
        "servers_count": servers_found.len(),
        "commands_checked": commands_checked,
        "issues": issues,
        "valid": issues.is_empty()
    }))
}

// ============================================================================
// IDE IMPORT (VS Code / Cursor)
// ============================================================================

/// Get VS Code settings path
#[allow(dead_code)]
fn get_vscode_settings_path() -> PathBuf {
    let user_profile = std::env::var("APPDATA").unwrap_or_default();
    PathBuf::from(user_profile)
        .join("Code")
        .join("User")
        .join("settings.json")
}

/// Get Cursor settings path  
#[allow(dead_code)]
fn get_cursor_settings_path() -> PathBuf {
    let user_profile = std::env::var("APPDATA").unwrap_or_default();
    PathBuf::from(user_profile)
        .join("Cursor")
        .join("User")
        .join("settings.json")
}

/// Import VS Code settings
#[allow(dead_code)]
pub async fn import_vscode_settings(args: Value) -> Result<Value> {
    let target_path = args["target_path"].as_str();
    let merge = args["merge"].as_bool().unwrap_or(true);

    let source_path = get_vscode_settings_path();

    if !source_path.exists() {
        return Ok(json!({
            "success": false,
            "error": "VS Code settings not found",
            "searched_path": source_path.to_string_lossy()
        }));
    }

    let content = fs::read_to_string(&source_path)?;
    let settings: Value = serde_json::from_str(&content)?;

    // If target specified, save there
    if let Some(target) = target_path {
        let target_path = PathBuf::from(target);

        let final_content = if merge && target_path.exists() {
            // Merge with existing
            let existing = fs::read_to_string(&target_path)?;
            let mut existing_settings: Value = serde_json::from_str(&existing)?;

            if let (Some(existing_obj), Some(new_obj)) =
                (existing_settings.as_object_mut(), settings.as_object())
            {
                for (k, v) in new_obj {
                    existing_obj.insert(k.clone(), v.clone());
                }
            }
            serde_json::to_string_pretty(&existing_settings)?
        } else {
            serde_json::to_string_pretty(&settings)?
        };

        fs::write(&target_path, &final_content)?;

        return Ok(json!({
            "success": true,
            "source": source_path.to_string_lossy(),
            "target": target_path.to_string_lossy(),
            "merged": merge,
            "settings_count": settings.as_object().map(|o| o.len()).unwrap_or(0)
        }));
    }

    // Just return the settings
    Ok(json!({
        "success": true,
        "source": source_path.to_string_lossy(),
        "settings": settings,
        "settings_count": settings.as_object().map(|o| o.len()).unwrap_or(0)
    }))
}

/// Import Cursor settings
#[allow(dead_code)]
pub async fn import_cursor_settings(args: Value) -> Result<Value> {
    let target_path = args["target_path"].as_str();
    let merge = args["merge"].as_bool().unwrap_or(true);

    let source_path = get_cursor_settings_path();

    if !source_path.exists() {
        return Ok(json!({
            "success": false,
            "error": "Cursor settings not found",
            "searched_path": source_path.to_string_lossy()
        }));
    }

    let content = fs::read_to_string(&source_path)?;
    let settings: Value = serde_json::from_str(&content)?;

    if let Some(target) = target_path {
        let target_path = PathBuf::from(target);

        let final_content = if merge && target_path.exists() {
            let existing = fs::read_to_string(&target_path)?;
            let mut existing_settings: Value = serde_json::from_str(&existing)?;

            if let (Some(existing_obj), Some(new_obj)) =
                (existing_settings.as_object_mut(), settings.as_object())
            {
                for (k, v) in new_obj {
                    existing_obj.insert(k.clone(), v.clone());
                }
            }
            serde_json::to_string_pretty(&existing_settings)?
        } else {
            serde_json::to_string_pretty(&settings)?
        };

        fs::write(&target_path, &final_content)?;

        return Ok(json!({
            "success": true,
            "source": source_path.to_string_lossy(),
            "target": target_path.to_string_lossy(),
            "merged": merge,
            "settings_count": settings.as_object().map(|o| o.len()).unwrap_or(0)
        }));
    }

    Ok(json!({
        "success": true,
        "source": source_path.to_string_lossy(),
        "settings": settings,
        "settings_count": settings.as_object().map(|o| o.len()).unwrap_or(0)
    }))
}

/// Get list of installed extensions
#[allow(dead_code)]
pub async fn import_extensions_list(args: Value) -> Result<Value> {
    let source = args["source"].as_str().unwrap_or("vscode");

    let (cmd, name) = match source {
        "cursor" => ("cursor", "Cursor"),
        _ => ("code", "VS Code"),
    };

    let output = tokio::process::Command::new(cmd)
        .args(["--list-extensions"])
        .output()
        .await;

    match output {
        Ok(out) if out.status.success() => {
            let extensions: Vec<String> = String::from_utf8_lossy(&out.stdout)
                .lines()
                .map(|s| s.to_string())
                .collect();

            Ok(json!({
                "success": true,
                "source": name,
                "extensions": extensions,
                "count": extensions.len()
            }))
        }
        Ok(out) => Ok(json!({
            "success": false,
            "error": String::from_utf8_lossy(&out.stderr).to_string(),
            "source": name
        })),
        Err(e) => Ok(json!({
            "success": false,
            "error": format!("{} not found or not in PATH: {}", cmd, e),
            "source": name
        })),
    }
}

/// Create MCP configuration file
#[allow(dead_code)]
pub async fn create_mcp_config(args: Value) -> Result<Value> {
    let servers = &args["servers"];
    let output_path = args["output_path"]
        .as_str()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("mcp_config.json"));

    let config = json!({
        "mcpServers": servers
    });

    let content = serde_json::to_string_pretty(&config)?;

    // Create parent dirs if needed
    if let Some(parent) = output_path.parent() {
        let _ = fs::create_dir_all(parent);
    }

    fs::write(&output_path, &content)?;

    Ok(json!({
        "success": true,
        "path": output_path.to_string_lossy(),
        "servers_count": servers.as_object().map(|o| o.len()).unwrap_or(0)
    }))
}
