//! Infrastructure tools - process listing and project preflight checks.
//! Generic versions for any user (no CPC-specific paths).

use anyhow::Result;
use serde_json::{json, Value};
use std::process::Stdio;

async fn run_ps(cmd: &str) -> String {
    match tokio::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", cmd])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
    {
        Ok(o) => String::from_utf8_lossy(&o.stdout).trim().to_string(),
        Err(e) => format!("error: {}", e),
    }
}

/// List running processes, optionally filtered by name substring(s).
/// args: { "name_filter": ["chrome", "node"] } — case-insensitive contains match
pub async fn server_health(args: Value) -> Result<Value> {
    let name_filter = args.get("name_filter").and_then(|v| v.as_array());
    let ps = r#"Get-Process | Select-Object ProcessName,Id,@{N='MB';E={[math]::Round($_.WorkingSet64/1MB,1)}},StartTime | ConvertTo-Json"#;
    let output = run_ps(ps).await;
    let parsed: Value = serde_json::from_str(&output).unwrap_or(json!({"raw": output}));

    if let Some(filter) = name_filter {
        let names: Vec<String> = filter
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.to_lowercase()))
            .collect();
        if let Some(arr) = parsed.as_array() {
            let filtered: Vec<&Value> = arr
                .iter()
                .filter(|p| {
                    p["ProcessName"]
                        .as_str()
                        .map(|n| names.iter().any(|f| n.to_lowercase().contains(f)))
                        .unwrap_or(false)
                })
                .collect();
            return Ok(json!({"processes": filtered}));
        }
    }
    Ok(json!({"processes": parsed}))
}

/// Stub fallback resolver — returns no fallback. The Programmer-Wander build
/// doesn't ship with a curated fallback table; users override at integration time.
pub async fn tool_fallback(args: Value) -> Result<Value> {
    let tool = args["tool"].as_str().unwrap_or("");
    Ok(json!({
        "tool": tool,
        "fallback": null,
        "note": "Programmer-Wander does not ship a curated fallback table. Provide your own resolver if needed."
    }))
}

/// Pre-deploy checks for a Cargo project at a user-supplied path.
/// args: { "path": "C:\\path\\to\\project" } — checks Cargo.toml + src/ presence
pub async fn preflight_deploy(args: Value) -> Result<Value> {
    let path = args["path"].as_str().unwrap_or("");
    if path.is_empty() {
        anyhow::bail!("path is required (path to Cargo project root)");
    }

    let path_p = std::path::Path::new(path);
    let cargo_exists = path_p.join("Cargo.toml").exists();
    let src_exists = path_p.join("src").is_dir();

    Ok(json!({
        "path": path,
        "cargo_toml_exists": cargo_exists,
        "src_dir_exists": src_exists,
        "ready_to_build": cargo_exists && src_exists,
    }))
}
