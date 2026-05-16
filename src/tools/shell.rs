//! Shell Execution

use anyhow::Result;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::process::Command;
use tokio::sync::Mutex;
use tokio::time::timeout;
use tracing::info;
use once_cell::sync::Lazy;

const DEFAULT_TIMEOUT: u64 = 30;

// Session storage
static SESSIONS: Lazy<Arc<Mutex<HashMap<String, Session>>>> = 
    Lazy::new(|| Arc::new(Mutex::new(HashMap::new())));

struct Session {
    cwd: String,
    history: Vec<HistoryEntry>,
}

#[derive(Clone)]
struct HistoryEntry {
    command: String,
    exit_code: Option<i32>,
    timestamp: u64,
}

/// Execute shell command
pub async fn execute(args: Value) -> Result<Value> {
    let command = args.get("command").and_then(|v| v.as_str()).unwrap_or("");
    let timeout_secs = args.get("timeout").and_then(|v| v.as_u64()).unwrap_or(DEFAULT_TIMEOUT);
    let session_id = args.get("session_id").and_then(|v| v.as_str());
    
    if command.is_empty() {
        anyhow::bail!("command is required");
    }
    
    info!("Executing: {}", &command[..command.len().min(80)]);
    
    // Get working directory from session if provided
    let cwd = if let Some(sid) = session_id {
        let sessions = SESSIONS.lock().await;
        sessions.get(sid).map(|s| s.cwd.clone())
    } else {
        None
    };
    
    let mut cmd = Command::new("cmd");
    cmd.args(["/C", command])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    
    if let Some(dir) = &cwd {
        cmd.current_dir(dir);
    }
    
    let result = timeout(Duration::from_secs(timeout_secs), cmd.output()).await;
    
    match result {
        Ok(Ok(output)) => {
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            let exit_code = output.status.code();
            let success = exit_code == Some(0);
            
            // Record in session history
            if let Some(sid) = session_id {
                let mut sessions = SESSIONS.lock().await;
                if let Some(session) = sessions.get_mut(sid) {
                    session.history.push(HistoryEntry {
                        command: command.to_string(),
                        exit_code,
                        timestamp: std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_secs())
                            .unwrap_or(0),
                    });
                }
            }
            
            Ok(json!({
                "success": success,
                "stdout": stdout,
                "stderr": stderr,
                "exit_code": exit_code,
                "runtime": "completed"
            }))
        }
        Ok(Err(e)) => Ok(json!({
            "success": false,
            "error": format!("Execute failed: {}", e)
        })),
        Err(_) => Ok(json!({
            "success": false,
            "error": format!("Command timed out after {}s", timeout_secs)
        })),
    }
}

/// Execute chain of commands, stop on first failure
pub async fn chain(args: Value) -> Result<Value> {
    let commands: Vec<&str> = args.get("commands")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();
    let session_id = args.get("session_id").and_then(|v| v.as_str()).unwrap_or("default");
    let stop_on_error = args.get("stop_on_error").and_then(|v| v.as_bool()).unwrap_or(true);
    
    if commands.is_empty() {
        anyhow::bail!("commands array is required");
    }
    
    let mut results = Vec::new();
    let mut all_success = true;
    let mut failed_at: Option<usize> = None;
    
    for (i, cmd) in commands.iter().enumerate() {
        let result = execute(json!({
            "command": cmd,
            "session_id": session_id
        })).await?;
        
        let success = result.get("success").and_then(|v| v.as_bool()).unwrap_or(false);
        results.push(result);
        
        if !success {
            all_success = false;
            failed_at = Some(i);
            if stop_on_error {
                break;
            }
        }
    }
    
    Ok(json!({
        "success": all_success,
        "results": results,
        "failed_at": failed_at,
        "commands_run": results.len()
    }))
}

/// Create terminal session
pub async fn create_session(args: Value) -> Result<Value> {
    let name = args.get("name").and_then(|v| v.as_str());
    let cwd = args.get("cwd").and_then(|v| v.as_str())
        .unwrap_or("C:\\Users\\josep");
    
    let session_id = name.map(String::from)
        .unwrap_or_else(|| format!("session_{:08x}", rand::random::<u32>()));
    
    let mut sessions = SESSIONS.lock().await;
    sessions.insert(session_id.clone(), Session {
        cwd: cwd.to_string(),
        history: Vec::new(),
    });
    
    info!("Created session: {}", session_id);
    
    Ok(json!({
        "success": true,
        "session_id": session_id,
        "cwd": cwd
    }))
}

/// List active sessions
pub async fn list_sessions() -> Result<Value> {
    let sessions = SESSIONS.lock().await;
    let list: Vec<Value> = sessions.iter()
        .map(|(id, s)| json!({
            "session_id": id,
            "cwd": s.cwd,
            "history_count": s.history.len()
        }))
        .collect();
    
    Ok(json!({
        "success": true,
        "sessions": list,
        "count": list.len()
    }))
}

/// Destroy session
pub async fn destroy_session(args: Value) -> Result<Value> {
    let session_id = args.get("session_id").and_then(|v| v.as_str()).unwrap_or("");
    
    if session_id.is_empty() {
        anyhow::bail!("session_id is required");
    }
    
    let mut sessions = SESSIONS.lock().await;
    if sessions.remove(session_id).is_some() {
        Ok(json!({
            "success": true,
            "session_id": session_id
        }))
    } else {
        Ok(json!({
            "success": false,
            "error": "Session not found"
        }))
    }
}

// Environment variables per session
static SESSION_ENV: Lazy<Arc<Mutex<HashMap<String, HashMap<String, String>>>>> = 
    Lazy::new(|| Arc::new(Mutex::new(HashMap::new())));

// Predefined shortcuts
fn get_shortcuts() -> HashMap<&'static str, Vec<&'static str>> {
    let mut shortcuts = HashMap::new();
    shortcuts.insert("git_commit_push", vec!["git add -A", "git commit -m \"$message\"", "git push"]);
    shortcuts.insert("npm_build_deploy", vec!["npm install", "npm run build", "npm run deploy"]);
    shortcuts.insert("pip_install_freeze", vec!["pip install $packages", "pip freeze > requirements.txt"]);
    shortcuts.insert("python_venv_activate", vec!["python -m venv .venv", ".venv\\Scripts\\activate"]);
    shortcuts
}

/// Set environment variable in session
pub async fn set_env(args: Value) -> Result<Value> {
    let session_id = args.get("session_id").and_then(|v| v.as_str()).unwrap_or("default");
    let key = args.get("key").and_then(|v| v.as_str()).unwrap_or("");
    let value = args.get("value").and_then(|v| v.as_str()).unwrap_or("");
    
    if key.is_empty() {
        anyhow::bail!("key is required");
    }
    
    let mut env_map = SESSION_ENV.lock().await;
    let session_env = env_map.entry(session_id.to_string()).or_insert_with(HashMap::new);
    session_env.insert(key.to_string(), value.to_string());
    
    Ok(json!({
        "success": true,
        "session_id": session_id,
        "key": key,
        "value": value
    }))
}

/// Get environment variable from session
pub async fn get_env(args: Value) -> Result<Value> {
    let session_id = args.get("session_id").and_then(|v| v.as_str()).unwrap_or("default");
    let key = args.get("key").and_then(|v| v.as_str());
    
    let env_map = SESSION_ENV.lock().await;
    
    if let Some(k) = key {
        // Get specific key
        let session_value = env_map.get(session_id)
            .and_then(|m| m.get(k))
            .cloned();
        let value = session_value.or_else(|| std::env::var(k).ok());
        
        Ok(json!({
            "success": true,
            "key": k,
            "value": value
        }))
    } else {
        // Get all session env vars
        let vars: HashMap<String, String> = env_map.get(session_id)
            .cloned()
            .unwrap_or_default();
        
        Ok(json!({
            "success": true,
            "session_id": session_id,
            "env": vars
        }))
    }
}

/// Get command history for session
pub async fn history(args: Value) -> Result<Value> {
    let session_id = args.get("session_id").and_then(|v| v.as_str()).unwrap_or("default");
    let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(10) as usize;
    
    let sessions = SESSIONS.lock().await;
    
    if let Some(session) = sessions.get(session_id) {
        let history: Vec<Value> = session.history.iter()
            .rev()
            .take(limit)
            .map(|h| json!({
                "command": h.command,
                "exit_code": h.exit_code,
                "timestamp": h.timestamp
            }))
            .collect();
        
        Ok(json!({
            "success": true,
            "session_id": session_id,
            "history": history,
            "count": history.len()
        }))
    } else {
        Ok(json!({
            "success": false,
            "error": "Session not found"
        }))
    }
}

/// Read recent output from session (placeholder - real impl would buffer output)
pub async fn read_output(args: Value) -> Result<Value> {
    let session_id = args.get("session_id").and_then(|v| v.as_str()).unwrap_or("default");
    let lines = args.get("lines").and_then(|v| v.as_u64()).unwrap_or(50);
    
    // In real implementation, would maintain output buffer per session
    // For now, return last command output if available
    let sessions = SESSIONS.lock().await;
    
    if let Some(session) = sessions.get(session_id) {
        let last_commands: Vec<&str> = session.history.iter()
            .rev()
            .take(5)
            .map(|h| h.command.as_str())
            .collect();
        
        Ok(json!({
            "success": true,
            "session_id": session_id,
            "note": "Output buffering not implemented - showing recent commands",
            "recent_commands": last_commands,
            "requested_lines": lines
        }))
    } else {
        Ok(json!({
            "success": false,
            "error": "Session not found"
        }))
    }
}

/// Run predefined shortcut
pub async fn shortcut(args: Value) -> Result<Value> {
    let session_id = args.get("session_id").and_then(|v| v.as_str()).unwrap_or("default");
    let shortcut_name = args.get("shortcut_name").and_then(|v| v.as_str()).unwrap_or("");
    let params = &args["params"];
    
    let shortcuts = get_shortcuts();
    
    if let Some(commands) = shortcuts.get(shortcut_name) {
        // Substitute parameters
        let substituted: Vec<String> = commands.iter()
            .map(|cmd| {
                let mut result = cmd.to_string();
                if let Some(obj) = params.as_object() {
                    for (key, value) in obj {
                        let placeholder = format!("${}", key);
                        if let Some(v) = value.as_str() {
                            result = result.replace(&placeholder, v);
                        }
                    }
                }
                result
            })
            .collect();
        
        // Execute as chain
        chain(json!({
            "session_id": session_id,
            "commands": substituted,
            "stop_on_error": true
        })).await
    } else {
        Ok(json!({
            "success": false,
            "error": format!("Unknown shortcut: {}", shortcut_name)
        }))
    }
}

/// List available shortcuts
pub async fn list_shortcuts() -> Result<Value> {
    let shortcuts = get_shortcuts();
    
    let list: Vec<Value> = shortcuts.iter()
        .map(|(name, cmds)| json!({
            "name": name,
            "commands": cmds,
            "description": format!("{} step workflow", cmds.len())
        }))
        .collect();
    
    Ok(json!({
        "success": true,
        "shortcuts": list,
        "count": list.len()
    }))
}


/// Save session state to checkpoint file for crash recovery
pub async fn session_checkpoint(args: Value) -> Result<Value> {
    let session_id = args.get("session_id").and_then(|v| v.as_str()).unwrap_or("default");
    let default_path = format!("C:/temp/session_{}.checkpoint", session_id);
    let checkpoint_path = args.get("checkpoint_path")
        .and_then(|v| v.as_str())
        .unwrap_or(&default_path);

    let sessions = SESSIONS.lock().await;
    let session = match sessions.get(session_id) {
        Some(s) => s,
        None => return Ok(json!({"error": format!("Session '{}' not found", session_id)})),
    };

    // Get environment for this session
    let env_map = SESSION_ENV.lock().await;
    let env = env_map.get(session_id).cloned().unwrap_or_default();

    // Build checkpoint data
    let checkpoint = json!({
        "session_id": session_id,
        "cwd": session.cwd,
        "env": env,
        "history": session.history.iter().map(|h| &h.command).collect::<Vec<_>>(),
        "saved_at": chrono::Utc::now().to_rfc3339(),
    });

    // Ensure directory exists
    if let Some(parent) = std::path::Path::new(checkpoint_path).parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    match std::fs::write(checkpoint_path, serde_json::to_string_pretty(&checkpoint)?) {
        Ok(_) => Ok(json!({
            "success": true,
            "checkpoint_path": checkpoint_path,
            "session_id": session_id,
            "commands_saved": session.history.len()
        })),
        Err(e) => Ok(json!({"error": format!("Failed to write checkpoint: {}", e)})),
    }
}

/// Recover session from checkpoint file
pub async fn session_recover_from_file(args: Value) -> Result<Value> {
    let checkpoint_path = match args.get("checkpoint_path").and_then(|v| v.as_str()) {
        Some(p) => p,
        None => return Ok(json!({"error": "checkpoint_path required"})),
    };

    // Read checkpoint file
    let checkpoint_data = match std::fs::read_to_string(checkpoint_path) {
        Ok(data) => data,
        Err(e) => return Ok(json!({"error": format!("Failed to read checkpoint: {}", e)})),
    };

    let checkpoint: Value = match serde_json::from_str(&checkpoint_data) {
        Ok(v) => v,
        Err(e) => return Ok(json!({"error": format!("Invalid checkpoint format: {}", e)})),
    };

    let session_id = checkpoint["session_id"].as_str().unwrap_or("recovered").to_string();
    let cwd = checkpoint["cwd"].as_str().unwrap_or("C:\\").to_string();

    // Restore environment
    let mut env: HashMap<String, String> = HashMap::new();
    if let Some(saved_env) = checkpoint["env"].as_object() {
        for (k, v) in saved_env {
            if let Some(val) = v.as_str() {
                env.insert(k.clone(), val.to_string());
            }
        }
    }

    // Restore history
    let mut history: Vec<HistoryEntry> = Vec::new();
    if let Some(saved_history) = checkpoint["history"].as_array() {
        for item in saved_history {
            if let Some(cmd) = item.as_str() {
                history.push(HistoryEntry {
                    command: cmd.to_string(),
                    exit_code: None,
                    timestamp: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs(),
                });
            }
        }
    }

    // Create new session with restored state
    let mut sessions = SESSIONS.lock().await;
    sessions.insert(session_id.clone(), Session {
        cwd: cwd.clone(),
        history: history.clone(),
    });

    // Restore environment
    let mut env_map = SESSION_ENV.lock().await;
    env_map.insert(session_id.clone(), env);

    Ok(json!({
        "success": true,
        "session_id": session_id,
        "recovered_from": checkpoint_path,
        "saved_at": checkpoint["saved_at"],
        "commands_restored": history.len(),
        "cwd": cwd
    }))
}

pub async fn powershell(args: Value) -> Result<Value> {
    let command = args.get("command").and_then(|v| v.as_str()).unwrap_or("");
    let timeout_secs = args.get("timeout_secs").and_then(|v| v.as_u64()).unwrap_or(30);

    if command.is_empty() {
        anyhow::bail!("command is required");
    }

    info!("PowerShell: {}", &command[..command.len().min(80)]);

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(timeout_secs),
        tokio::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", command])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
    ).await;

    match result {
        Ok(Ok(output)) => {
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            Ok(json!({
                "exit_code": output.status.code().unwrap_or(-1),
                "stdout": stdout.trim(),
                "stderr": stderr.trim(),
                "success": output.status.success()
            }))
        },
        Ok(Err(e)) => Ok(json!({"error": e.to_string()})),
        Err(_) => Ok(json!({"error": format!("Timed out after {}s", timeout_secs)})),
    }
}

pub async fn md2docx(args: Value) -> Result<Value> {
    let input = args.get("input").and_then(|v| v.as_str()).unwrap_or("");
    let output = args.get("output").and_then(|v| v.as_str()).unwrap_or("");

    if input.is_empty() || output.is_empty() {
        anyhow::bail!("input and output are required");
    }

    let result = tokio::process::Command::new("pandoc")
        .args([input, "-o", output])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await;

    match result {
        Ok(out) => {
            if out.status.success() {
                Ok(json!({"success": true, "output": output}))
            } else {
                Ok(json!({"error": String::from_utf8_lossy(&out.stderr).to_string()}))
            }
        },
        Err(e) => Ok(json!({"error": e.to_string()})),
    }
}

pub async fn session_cd(args: Value) -> Result<Value> {
    let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
    let session_id = args.get("session_id").and_then(|v| v.as_str());
    
    if path.is_empty() {
        anyhow::bail!("path is required");
    }
    
    let resolved = std::path::Path::new(path);
    if !resolved.exists() {
        anyhow::bail!("Directory does not exist: {}", path);
    }
    
    if let Some(sid) = session_id {
        let mut sessions = SESSIONS.lock().await;
        if let Some(session) = sessions.get_mut(sid) {
            session.cwd = path.to_string();
            return Ok(json!({"success": true, "session_id": sid, "cwd": path}));
        }
        anyhow::bail!("Session not found: {}", sid);
    }
    
    Ok(json!({"error": "session_id is required"}))
}

pub async fn shortcut_chain(args: Value) -> Result<Value> {
    let shortcuts = args.get("shortcuts").and_then(|v| v.as_array());
    let stop_on_error = args.get("stop_on_error").and_then(|v| v.as_bool()).unwrap_or(true);
    
    let shortcuts = match shortcuts {
        Some(s) => s,
        None => anyhow::bail!("shortcuts array is required"),
    };
    
    let mut results = Vec::new();
    for (i, shortcut_val) in shortcuts.iter().enumerate() {
        let shortcut_name = shortcut_val.as_str().unwrap_or("");
        if shortcut_name.is_empty() { continue; }
        
        let result = shortcut(json!({"name": shortcut_name})).await;
        match result {
            Ok(val) => {
                let success = !val.get("error").is_some();
                results.push(json!({"index": i, "shortcut": shortcut_name, "result": val, "success": success}));
                if !success && stop_on_error {
                    return Ok(json!({"completed": i, "total": shortcuts.len(), "stopped_on_error": true, "results": results}));
                }
            },
            Err(e) => {
                results.push(json!({"index": i, "shortcut": shortcut_name, "error": e.to_string(), "success": false}));
                if stop_on_error {
                    return Ok(json!({"completed": i, "total": shortcuts.len(), "stopped_on_error": true, "results": results}));
                }
            }
        }
    }
    
    Ok(json!({"completed": shortcuts.len(), "total": shortcuts.len(), "results": results}))
}
