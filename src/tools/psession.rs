//! Persistent Shell Sessions - real process persistence across calls
//! Supports PowerShell and WSL (bash) backends

use anyhow::Result;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::io::{BufRead, BufReader, Write};
use std::thread;
use once_cell::sync::Lazy;
use tracing::info;

#[cfg(windows)]
use std::os::windows::process::CommandExt;
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

static PSESSIONS: Lazy<Arc<Mutex<HashMap<String, PersistentSession>>>> =
    Lazy::new(|| Arc::new(Mutex::new(HashMap::new())));

struct PersistentSession {
    name: String,
    shell_type: String, // "powershell" or "wsl"
    child: Child,
    output_buffer: Arc<Mutex<Vec<String>>>,
    history: Vec<String>,
    created_at: String,
}

fn start_reader(stream: impl std::io::Read + Send + 'static, buffer: Arc<Mutex<Vec<String>>>) {
    thread::spawn(move || {
        let reader = BufReader::new(stream);
        for line in reader.lines().flatten() {
            buffer.lock().unwrap().push(line);
        }
    });
}

pub async fn create(args: Value) -> Result<Value> {
    let name = args.get("name").and_then(|v| v.as_str()).unwrap_or("default");
    let shell = args.get("shell").and_then(|v| v.as_str()).unwrap_or("powershell");
    let default_cwd = std::env::var("WORKSPACE_PATH").unwrap_or_else(|_| "C:\\".to_string());
    let cwd = args.get("cwd").and_then(|v| v.as_str()).unwrap_or(&default_cwd);

    info!("Creating persistent {} session: {}", shell, name);

    let mut cmd = match shell {
        "wsl" => {
            let mut c = Command::new("wsl");
            c.args(["-d", "Ubuntu-24.04", "--", "bash"]);
            c
        },
        _ => {
            let mut c = Command::new("powershell");
            c.args(["-NoLogo", "-NoProfile", "-Command", "-"]);
            c
        }
    };

    cmd.current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);

    let mut child = cmd.spawn()
        .map_err(|e| anyhow::anyhow!("Failed to spawn {}: {}", shell, e))?;

    let stdout = child.stdout.take()
        .ok_or_else(|| anyhow::anyhow!("Failed to take stdout"))?;

    let buffer = Arc::new(Mutex::new(Vec::new()));
    start_reader(stdout, buffer.clone());

    // Also capture stderr into the same buffer
    if let Some(stderr) = child.stderr.take() {
        start_reader(stderr, buffer.clone());
    }

    thread::sleep(std::time::Duration::from_millis(200));

    let session_id = format!("{}_{}", shell, name);
    let created = chrono::Local::now().to_rfc3339();

    let mut sessions = PSESSIONS.lock().unwrap();
    sessions.insert(session_id.clone(), PersistentSession {
        name: name.to_string(),
        shell_type: shell.to_string(),
        child,
        output_buffer: buffer,
        history: Vec::new(),
        created_at: created.clone(),
    });

    Ok(json!({
        "session_id": session_id,
        "shell": shell,
        "name": name,
        "created_at": created
    }))
}

pub async fn run(args: Value) -> Result<Value> {
    let session_id = args.get("session_id").and_then(|v| v.as_str()).unwrap_or("");
    let command = args.get("command").and_then(|v| v.as_str()).unwrap_or("");
    let timeout_secs = args.get("timeout_secs").and_then(|v| v.as_u64()).unwrap_or(30);

    if session_id.is_empty() || command.is_empty() {
        anyhow::bail!("session_id and command are required");
    }

    info!("psession_run [{}]: {}", session_id, &command[..command.len().min(80)]);

    let mut sessions = PSESSIONS.lock().unwrap();
    let session = sessions.get_mut(session_id)
        .ok_or_else(|| anyhow::anyhow!("Session not found: {}", session_id))?;

    // Record pre-command buffer position
    let start_pos = session.output_buffer.lock().unwrap().len();

    // Write command to stdin
    let marker = format!("__DONE_{}__", uuid::Uuid::new_v4().to_string().get(..8).unwrap_or("00000000"));
    let stdin = session.child.stdin.as_mut()
        .ok_or_else(|| anyhow::anyhow!("stdin not available"))?;

    let full_cmd = if session.shell_type == "wsl" {
        format!("{}\necho {}\n", command, marker)
    } else {
        format!("{}\nWrite-Output '{}'\n", command, marker)
    };

    stdin.write_all(full_cmd.as_bytes())
        .map_err(|e| anyhow::anyhow!("Write failed: {}", e))?;
    stdin.flush()
        .map_err(|e| anyhow::anyhow!("Flush failed: {}", e))?;

    session.history.push(command.to_string());

    // Wait for marker with timeout
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);
    let mut output_lines = Vec::new();
    let mut found_marker = false;

    // Drop the sessions lock while waiting
    let buffer = session.output_buffer.clone();
    drop(sessions);

    loop {
        if std::time::Instant::now() > deadline {
            break;
        }

        {
            let buf = buffer.lock().unwrap();
            let current_len = buf.len();
            if current_len > start_pos {
                for i in start_pos..current_len {
                    if buf[i].contains(&marker) {
                        found_marker = true;
                        // Collect lines between start_pos and marker
                        output_lines = buf[start_pos..i].to_vec();
                        break;
                    }
                }
                if found_marker { break; }
            }
        }

        thread::sleep(std::time::Duration::from_millis(50));
    }

    if !found_marker {
        // Grab whatever we have
        let buf = buffer.lock().unwrap();
        if buf.len() > start_pos {
            output_lines = buf[start_pos..].to_vec();
        }
    }

    Ok(json!({
        "session_id": session_id,
        "output": output_lines.join("\n"),
        "lines": output_lines.len(),
        "completed": found_marker,
        "timed_out": !found_marker
    }))
}

pub async fn destroy(args: Value) -> Result<Value> {
    let session_id = args.get("session_id").and_then(|v| v.as_str()).unwrap_or("");
    if session_id.is_empty() {
        anyhow::bail!("session_id is required");
    }

    let mut sessions = PSESSIONS.lock().unwrap();
    if let Some(mut session) = sessions.remove(session_id) {
        let _ = session.child.kill();
        Ok(json!({"destroyed": session_id}))
    } else {
        Ok(json!({"error": format!("Session not found: {}", session_id)}))
    }
}

pub async fn list(_args: Value) -> Result<Value> {
    let sessions = PSESSIONS.lock().unwrap();
    let list: Vec<Value> = sessions.iter().map(|(id, s)| {
        json!({
            "session_id": id,
            "name": s.name,
            "shell": s.shell_type,
            "history_count": s.history.len(),
            "buffer_lines": s.output_buffer.lock().unwrap().len(),
            "created_at": s.created_at,
        })
    }).collect();

    Ok(json!({"sessions": list, "count": list.len()}))
}

pub async fn read_output(args: Value) -> Result<Value> {
    let session_id = args.get("session_id").and_then(|v| v.as_str()).unwrap_or("");
    let tail_n = args.get("tail").and_then(|v| v.as_u64()).unwrap_or(20) as usize;

    if session_id.is_empty() {
        anyhow::bail!("session_id is required");
    }

    let sessions = PSESSIONS.lock().unwrap();
    let session = sessions.get(session_id)
        .ok_or_else(|| anyhow::anyhow!("Session not found: {}", session_id))?;

    let buf = session.output_buffer.lock().unwrap();
    let total = buf.len();
    let start = if total > tail_n { total - tail_n } else { 0 };

    Ok(json!({
        "session_id": session_id,
        "total_lines": total,
        "tail": buf[start..].join("\n"),
    }))
}

pub async fn history(args: Value) -> Result<Value> {
    let session_id = args.get("session_id").and_then(|v| v.as_str()).unwrap_or("");
    if session_id.is_empty() {
        anyhow::bail!("session_id is required");
    }

    let sessions = PSESSIONS.lock().unwrap();
    let session = sessions.get(session_id)
        .ok_or_else(|| anyhow::anyhow!("Session not found: {}", session_id))?;

    Ok(json!({
        "session_id": session_id,
        "history": session.history,
        "count": session.history.len()
    }))
}
