//! System Operations (clipboard, processes, screenshots, system info, resource monitoring)

use anyhow::Result;
use once_cell::sync::Lazy;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::os::windows::process::CommandExt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::process::Command;
use tokio::sync::RwLock;
use tracing::info;

/// Get system information
pub async fn get_info() -> Result<Value> {
    let hostname = std::env::var("COMPUTERNAME").unwrap_or_default();
    let user = std::env::var("USERNAME").unwrap_or_default();
    let home = std::env::var("USERPROFILE").unwrap_or_default();

    // Get memory info via PowerShell
    let mem_output = Command::new("powershell")
        .args(["-Command", "(Get-CimInstance Win32_OperatingSystem | Select-Object FreePhysicalMemory,TotalVisibleMemorySize | ConvertTo-Json)"])
        .output()
        .await
        .ok();

    let (free_mem, total_mem) = if let Some(out) = mem_output {
        let json_str = String::from_utf8_lossy(&out.stdout);
        if let Ok(v) = serde_json::from_str::<Value>(&json_str) {
            let free = v
                .get("FreePhysicalMemory")
                .and_then(|v| v.as_u64())
                .unwrap_or(0)
                * 1024;
            let total = v
                .get("TotalVisibleMemorySize")
                .and_then(|v| v.as_u64())
                .unwrap_or(0)
                * 1024;
            (free, total)
        } else {
            (0, 0)
        }
    } else {
        (0, 0)
    };

    // Get CPU info
    let cpu_output = Command::new("powershell")
        .args(["-Command", "(Get-CimInstance Win32_Processor).Name"])
        .output()
        .await
        .ok();

    let cpu_name = cpu_output
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();

    Ok(json!({
        "success": true,
        "hostname": hostname,
        "user": user,
        "home": home,
        "os": "Windows",
        "arch": std::env::consts::ARCH,
        "cpu": cpu_name,
        "memory": {
            "total_bytes": total_mem,
            "free_bytes": free_mem,
            "used_percent": if total_mem > 0 {
                ((total_mem - free_mem) as f64 / total_mem as f64 * 100.0).round()
            } else { 0.0 }
        },
        "server": "antigravity-rs",
        "version": "1.0.0"
    }))
}

/// Read clipboard
pub async fn clipboard_read() -> Result<Value> {
    match arboard::Clipboard::new().and_then(|mut cb| cb.get_text()) {
        Ok(content) => Ok(json!({
            "success": true,
            "content": content
        })),
        Err(e) => Ok(json!({
            "success": false,
            "error": format!("Clipboard read failed: {}", e)
        })),
    }
}

/// Write to clipboard
pub async fn clipboard_write(args: Value) -> Result<Value> {
    let content = args.get("content").and_then(|v| v.as_str()).unwrap_or("");

    match arboard::Clipboard::new().and_then(|mut cb| cb.set_text(content.to_string())) {
        Ok(()) => Ok(json!({
            "success": true,
            "size": content.len()
        })),
        Err(e) => Ok(json!({
            "success": false,
            "error": format!("Clipboard write failed: {}", e)
        })),
    }
}

/// List processes
pub async fn list_processes(args: Value) -> Result<Value> {
    let filter = args.get("filter_name").and_then(|v| v.as_str());

    let ps_cmd = if let Some(f) = filter {
        format!("Get-Process | Where-Object {{$_.Name -like '*{}*'}} | Select-Object Id,Name,CPU,WorkingSet64 -First 50 | ConvertTo-Json", f)
    } else {
        "Get-Process | Select-Object Id,Name,CPU,WorkingSet64 -First 50 | ConvertTo-Json"
            .to_string()
    };

    let output = Command::new("powershell")
        .args(["-Command", &ps_cmd])
        .output()
        .await?;

    let json_str = String::from_utf8_lossy(&output.stdout);
    let processes: Value = serde_json::from_str(&json_str).unwrap_or(json!([]));

    // Normalize to array (single result comes as object)
    let processes = if processes.is_array() {
        processes
    } else if processes.is_object() {
        json!([processes])
    } else {
        json!([])
    };

    Ok(json!({
        "success": true,
        "processes": processes,
        "count": processes.as_array().map(|a| a.len()).unwrap_or(0)
    }))
}

/// Kill process by PID
pub async fn kill_process(args: Value) -> Result<Value> {
    let pid = args.get("pid").and_then(|v| v.as_u64()).unwrap_or(0);

    if pid == 0 {
        anyhow::bail!("pid is required");
    }

    let output = Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/F"])
        .output()
        .await?;

    if output.status.success() {
        info!("Killed process {}", pid);
        Ok(json!({
            "success": true,
            "pid": pid
        }))
    } else {
        Ok(json!({
            "success": false,
            "error": String::from_utf8_lossy(&output.stderr).to_string()
        }))
    }
}

// ============================================================================
// RESOURCE MONITORING
// ============================================================================

/// Resource watch state
static RESOURCE_WATCHES: Lazy<RwLock<HashMap<String, ResourceWatch>>> =
    Lazy::new(|| RwLock::new(HashMap::new()));

static RESOURCE_ALERTS: Lazy<RwLock<Vec<Value>>> = Lazy::new(|| RwLock::new(Vec::new()));

struct ResourceWatch {
    id: String,
    thresholds: Value,
    interval_seconds: u64,
    running: Arc<AtomicBool>,
}

/// Start watching system resources
pub async fn watch_resources(args: Value) -> Result<Value> {
    let thresholds = args.get("thresholds").cloned().unwrap_or(json!({
        "cpu": 80,
        "memory": 90,
        "disk": 95
    }));
    let interval_seconds = args["interval_seconds"].as_u64().unwrap_or(60);

    let watch_id = format!("watch_{}", chrono::Utc::now().timestamp_millis());
    let running = Arc::new(AtomicBool::new(true));

    // Store watch info
    {
        let mut watches = RESOURCE_WATCHES.write().await;
        watches.insert(
            watch_id.clone(),
            ResourceWatch {
                id: watch_id.clone(),
                thresholds: thresholds.clone(),
                interval_seconds,
                running: running.clone(),
            },
        );
    }

    // Spawn monitoring task
    let watch_id_clone = watch_id.clone();
    let thresholds_clone = thresholds.clone();

    tokio::spawn(async move {
        while running.load(Ordering::SeqCst) {
            // Check resources
            if let Ok(info) = get_info().await {
                let cpu = info["memory"]["used_percent"].as_f64().unwrap_or(0.0);
                let mem_used = 100.0
                    - (info["memory"]["free_bytes"].as_f64().unwrap_or(0.0)
                        / info["memory"]["total_bytes"].as_f64().unwrap_or(1.0)
                        * 100.0);

                // Check CPU threshold
                if let Some(cpu_thresh) = thresholds_clone["cpu"].as_f64() {
                    if cpu > cpu_thresh {
                        let mut alerts = RESOURCE_ALERTS.write().await;
                        alerts.push(json!({
                            "watch_id": watch_id_clone,
                            "type": "cpu",
                            "value": cpu,
                            "threshold": cpu_thresh,
                            "timestamp": chrono::Utc::now().to_rfc3339()
                        }));
                        if alerts.len() > 1000 {
                            alerts.remove(0);
                        }
                    }
                }

                // Check memory threshold
                if let Some(mem_thresh) = thresholds_clone["memory"].as_f64() {
                    if mem_used > mem_thresh {
                        let mut alerts = RESOURCE_ALERTS.write().await;
                        alerts.push(json!({
                            "watch_id": watch_id_clone,
                            "type": "memory",
                            "value": mem_used,
                            "threshold": mem_thresh,
                            "timestamp": chrono::Utc::now().to_rfc3339()
                        }));
                        if alerts.len() > 1000 {
                            alerts.remove(0);
                        }
                    }
                }
            }

            tokio::time::sleep(tokio::time::Duration::from_secs(interval_seconds)).await;
        }
    });

    Ok(json!({
        "success": true,
        "watch_id": watch_id,
        "thresholds": thresholds,
        "interval_seconds": interval_seconds
    }))
}

/// Stop resource watch
pub async fn stop_resource_watch(args: Value) -> Result<Value> {
    let watch_id = args["watch_id"].as_str().unwrap_or("");

    let mut watches = RESOURCE_WATCHES.write().await;

    if let Some(watch) = watches.remove(watch_id) {
        watch.running.store(false, Ordering::SeqCst);
        return Ok(json!({
            "success": true,
            "watch_id": watch_id,
            "stopped": true
        }));
    }

    Ok(json!({
        "success": false,
        "error": format!("Watch {} not found", watch_id)
    }))
}

/// Get resource alerts
pub async fn get_resource_alerts(args: Value) -> Result<Value> {
    let watch_id = args["watch_id"].as_str();
    let limit = args["limit"].as_u64().unwrap_or(50) as usize;

    let alerts = RESOURCE_ALERTS.read().await;

    let filtered: Vec<&Value> = alerts
        .iter()
        .rev()
        .filter(|a| watch_id.map_or(true, |id| a["watch_id"].as_str() == Some(id)))
        .take(limit)
        .collect();

    Ok(json!({
        "success": true,
        "alerts": filtered,
        "count": filtered.len()
    }))
}

/// List active resource watches
pub async fn list_resource_watches() -> Result<Value> {
    let watches = RESOURCE_WATCHES.read().await;

    let watch_list: Vec<Value> = watches
        .values()
        .map(|w| {
            json!({
                "watch_id": w.id,
                "thresholds": w.thresholds,
                "interval_seconds": w.interval_seconds,
                "running": w.running.load(Ordering::SeqCst)
            })
        })
        .collect();

    Ok(json!({
        "success": true,
        "watches": watch_list,
        "count": watch_list.len()
    }))
}

/// Test TCP connectivity to a host:port
pub async fn port_check(args: Value) -> Result<Value> {
    let host = args
        .get("host")
        .and_then(|v| v.as_str())
        .unwrap_or("127.0.0.1");
    let port = match args.get("port").and_then(|v| v.as_u64()) {
        Some(p) => p as u16,
        None => anyhow::bail!("port required"),
    };
    let timeout_ms = args
        .get("timeout_ms")
        .and_then(|v| v.as_u64())
        .unwrap_or(2000);

    let addr = format!("{}:{}", host, port);
    let socket_addr: std::net::SocketAddr = addr
        .parse()
        .map_err(|e| anyhow::anyhow!("Invalid address {}: {}", addr, e))?;

    let timeout_dur = std::time::Duration::from_millis(timeout_ms);

    // Run blocking TCP connect in spawn_blocking
    let host_owned = host.to_string();
    tokio::task::spawn_blocking(move || {
        let start = std::time::Instant::now();
        match std::net::TcpStream::connect_timeout(&socket_addr, timeout_dur) {
            Ok(_) => {
                let elapsed_ms = start.elapsed().as_millis();
                Ok(json!({
                    "open": true,
                    "host": host_owned,
                    "port": port,
                    "connect_time_ms": elapsed_ms,
                }))
            }
            Err(e) => {
                let elapsed_ms = start.elapsed().as_millis();
                Ok(json!({
                    "open": false,
                    "host": host_owned,
                    "port": port,
                    "error": e.to_string(),
                    "elapsed_ms": elapsed_ms,
                }))
            }
        }
    })
    .await?
}

const CREATE_NO_WINDOW: u32 = 0x08000000;

/// Return last N lines of a file plus current byte offset
pub fn tail_file(args: &Value) -> Value {
    use std::io::{Read, Seek, SeekFrom};

    let path = match args.get("path").and_then(|v| v.as_str()) {
        Some(p) => p,
        None => return json!({"error": "Missing 'path' parameter"}),
    };
    let max_lines = args.get("lines").and_then(|v| v.as_u64()).unwrap_or(50) as usize;
    let since_bytes = args
        .get("since_bytes")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);

    let mut file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) => return json!({"error": format!("Cannot open file: {}", e)}),
    };

    let total_bytes = match file.metadata() {
        Ok(m) => m.len(),
        Err(e) => return json!({"error": format!("Cannot read metadata: {}", e)}),
    };

    if since_bytes > 0 {
        if since_bytes >= total_bytes {
            return json!({
                "lines": [],
                "byte_offset": total_bytes,
                "total_bytes": total_bytes,
                "new_content": false
            });
        }
        if let Err(e) = file.seek(SeekFrom::Start(since_bytes)) {
            return json!({"error": format!("Seek failed: {}", e)});
        }
        let mut new_data = String::new();
        if let Err(e) = file.read_to_string(&mut new_data) {
            return json!({"error": format!("Read failed: {}", e)});
        }
        let lines: Vec<&str> = new_data.lines().collect();
        let tail: Vec<&str> = if lines.len() > max_lines {
            lines[lines.len() - max_lines..].to_vec()
        } else {
            lines
        };
        return json!({
            "lines": tail,
            "byte_offset": total_bytes,
            "total_bytes": total_bytes,
            "new_content": true
        });
    }

    let read_size: u64 = (64 * 1024).min(total_bytes);
    let start_pos = total_bytes.saturating_sub(read_size);
    if let Err(e) = file.seek(SeekFrom::Start(start_pos)) {
        return json!({"error": format!("Seek failed: {}", e)});
    }
    let mut buf = String::new();
    if let Err(e) = file.read_to_string(&mut buf) {
        return json!({"error": format!("Read failed: {}", e)});
    }
    let lines: Vec<&str> = buf.lines().collect();
    let tail: Vec<&str> = if lines.len() > max_lines {
        lines[lines.len() - max_lines..].to_vec()
    } else {
        lines
    };
    json!({
        "lines": tail,
        "byte_offset": total_bytes,
        "total_bytes": total_bytes,
        "new_content": true
    })
}

/// Show a silent Windows toast notification
pub fn notify(args: &Value) -> Value {
    let title = args
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    let body = args
        .get("body")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    let icon = args.get("icon").and_then(|v| v.as_str()).unwrap_or("info");
    let duration_ms = args
        .get("duration_ms")
        .and_then(|v| v.as_u64())
        .unwrap_or(5000)
        .max(1);

    if title.is_empty() || body.is_empty() {
        return json!({"error": "Both title and body are required"});
    }
    if !matches!(icon, "info" | "warning" | "error") {
        return json!({"error": "icon must be one of: info, warning, error"});
    }

    let display_title = match icon {
        "warning" => format!("[Warning] {title}"),
        "error" => format!("[Error] {title}"),
        _ => format!("[Info] {title}"),
    };
    let toast_duration = if duration_ms > 7_000 { "long" } else { "short" };

    let script = r#"
$ErrorActionPreference = 'Stop'
$toastDuration = if ([int]$env:MCP_NOTIFY_DURATION_MS -gt 7000) { 'long' } else { 'short' }
if (Get-Command New-BurntToastNotification -ErrorAction SilentlyContinue) {
    New-BurntToastNotification -Text $env:MCP_NOTIFY_TITLE, $env:MCP_NOTIFY_BODY -Silent | Out-Null
    Write-Output 'burnttoast'
    return
}
Add-Type -AssemblyName System.Runtime.WindowsRuntime | Out-Null
[Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] > $null
[Windows.Data.Xml.Dom.XmlDocument, Windows.Data.Xml.Dom.XmlDocument, ContentType = WindowsRuntime] > $null
$titleEscaped = [System.Security.SecurityElement]::Escape($env:MCP_NOTIFY_TITLE)
$bodyEscaped = [System.Security.SecurityElement]::Escape($env:MCP_NOTIFY_BODY)
$xml = @"
<toast duration="$toastDuration">
  <visual>
    <binding template="ToastGeneric">
      <text>$titleEscaped</text>
      <text>$bodyEscaped</text>
    </binding>
  </visual>
  <audio silent="true"/>
</toast>
"@
$doc = [Windows.Data.Xml.Dom.XmlDocument]::new()
$doc.LoadXml($xml)
$toast = [Windows.UI.Notifications.ToastNotification]::new($doc)
$appId = '{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\WindowsPowerShell\v1.0\powershell.exe'
try {
    [Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier($appId).Show($toast)
} catch {
    [Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier().Show($toast)
}
Write-Output 'winrt'
"#;

    match std::process::Command::new("powershell")
        .args(["-NoProfile", "-Command", script])
        .env("MCP_NOTIFY_TITLE", &display_title)
        .env("MCP_NOTIFY_BODY", body)
        .env("MCP_NOTIFY_DURATION_MS", duration_ms.to_string())
        .creation_flags(CREATE_NO_WINDOW)
        .output()
    {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            let backend = stdout.lines().last().unwrap_or("powershell").trim();
            let success = output.status.success();

            if success {
                json!({
                    "success": true,
                    "backend": backend,
                    "title": display_title,
                    "body": body,
                    "icon": icon,
                    "duration_ms": duration_ms,
                    "toast_duration": toast_duration,
                    "silent": true
                })
            } else {
                json!({
                    "error": stderr.trim(),
                    "stdout": stdout.trim()
                })
            }
        }
        Err(e) => {
            json!({"error": format!("{}", e)})
        }
    }
}

/// Take a screenshot for troubleshooting. Returns path + metadata only (no raw bytes).
/// Refuses if the resulting file exceeds 1MB.
pub fn screenshot(args: &Value) -> Value {
    let save_path = args.get("save_path").and_then(|v| v.as_str());
    let quality = args.get("quality").and_then(|v| v.as_u64()).unwrap_or(60) as u8;
    let scale = args.get("scale").and_then(|v| v.as_f64()).unwrap_or(0.75);

    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let path = save_path
        .map(String::from)
        .unwrap_or_else(|| format!("C:\\temp\\screenshot_{}.jpg", timestamp));

    if let Some(parent) = std::path::Path::new(&path).parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let ps_script = format!(
        r#"
Add-Type -AssemblyName System.Windows.Forms,System.Drawing
$screen = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
$bitmap = New-Object System.Drawing.Bitmap($screen.Width, $screen.Height)
$g = [System.Drawing.Graphics]::FromImage($bitmap)
$g.CopyFromScreen($screen.Location, [System.Drawing.Point]::Empty, $screen.Size)
$w = [int]($screen.Width * {scale})
$h = [int]($screen.Height * {scale})
$scaled = New-Object System.Drawing.Bitmap($w, $h)
$gs = [System.Drawing.Graphics]::FromImage($scaled)
$gs.DrawImage($bitmap, 0, 0, $w, $h)
$enc = [System.Drawing.Imaging.ImageCodecInfo]::GetImageEncoders() | Where-Object {{ $_.MimeType -eq 'image/jpeg' }}
$ep = New-Object System.Drawing.Imaging.EncoderParameters(1)
$ep.Param[0] = New-Object System.Drawing.Imaging.EncoderParameter([System.Drawing.Imaging.Encoder]::Quality, {quality})
$scaled.Save('{path}', $enc, $ep)
$scaled.Dispose(); $bitmap.Dispose()
"#,
        scale = scale,
        quality = quality,
        path = path.replace('\\', "\\\\")
    );

    let output = std::process::Command::new("powershell")
        .args(["-NoProfile", "-Command", &ps_script])
        .output();

    match output {
        Ok(o) if o.status.success() => {
            let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            if size > 1_048_576 {
                let _ = std::fs::remove_file(&path);
                return json!({"error": format!("Screenshot too large ({} bytes). Lower quality or scale.", size)});
            }
            json!({"success": true, "path": path, "size_bytes": size, "quality": quality, "scale": scale})
        }
        Ok(o) => json!({"error": String::from_utf8_lossy(&o.stderr).trim().to_string()}),
        Err(e) => json!({"error": e.to_string()}),
    }
}
