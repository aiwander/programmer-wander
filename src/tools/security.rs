//! Security tools - check commands and audit logging
//! Ported from local's security module

use serde_json::{json, Value};
use anyhow::Result;
use std::fs::OpenOptions;
use std::io::Write;
use chrono::Local;

const DANGEROUS_PATTERNS: &[(&str, &str)] = &[
    ("rm -rf /", "Recursive delete of root filesystem"),
    ("rm -rf /*", "Recursive delete of all root contents"),
    ("del /s /q c:\\windows", "Delete Windows system files"),
    ("del /s /q c:\\program", "Delete Program Files"),
    ("format c:", "Format system drive"),
    ("rd /s /q c:\\", "Remove entire C: drive"),
    (":(){:|:&};:", "Fork bomb (bash)"),
    ("while(1){start powershell}", "Fork bomb (PowerShell)"),
    ("reg delete hklm", "Delete system registry keys"),
    ("reg delete hkcu\\software\\microsoft", "Delete critical user registry"),
    ("cipher /w:", "Secure wipe (often ransomware)"),
    ("-encodedcommand", "Obfuscated PowerShell (malware pattern)"),
    ("invoke-webrequest.*|iex", "Download and execute pattern"),
    ("bootrec /fixmbr", "Modify master boot record"),
    ("bcdedit /delete", "Delete boot configuration"),
];

const CONTEXT_PATTERNS: &[(&str, &str)] = &[
    ("rm -rf", "Recursive delete - check target path"),
    ("del /s", "Recursive delete - check target path"),
    ("rd /s", "Remove directory recursively - check target path"),
    ("rmdir /s", "Remove directory recursively - check target path"),
];

const SAFE_PATHS: &[&str] = &[
    "./", ".\\", "node_modules", "target/", "target\\",
    "dist/", "dist\\", "build/", "build\\",
    "__pycache__", ".cache", "temp/", "temp\\", "tmp/", "tmp\\",
];

const AUDIT_LOG: &str = "C:\\temp\\mcp_security_audit.log";

pub fn check_command_safety(command: &str) -> (bool, Option<String>, &'static str) {
    let cmd_lower = command.to_lowercase();
    for (pattern, reason) in DANGEROUS_PATTERNS {
        if cmd_lower.contains(&pattern.to_lowercase()) {
            return (false, Some(format!("BLOCKED: {} - {}", pattern, reason)), "critical");
        }
    }
    for (pattern, reason) in CONTEXT_PATTERNS {
        if cmd_lower.contains(&pattern.to_lowercase()) {
            if !SAFE_PATHS.iter().any(|s| cmd_lower.contains(s)) {
                return (false, Some(format!("WARNING: {} - verify target is safe", reason)), "warning");
            }
        }
    }
    (true, None, "safe")
}

pub fn audit_log_entry(command: &str, result: &str, severity: &str) {
    let timestamp = Local::now().format("%Y-%m-%d %H:%M:%S");
    let entry = format!("[{}] [{}] {} | {}\n", timestamp, severity, result, command);
    let _ = std::fs::create_dir_all("C:\\temp");
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(AUDIT_LOG) {
        let _ = file.write_all(entry.as_bytes());
    }
}

pub async fn check_command(args: Value) -> Result<Value> {
    let command = args["command"].as_str().unwrap_or("");
    let (is_safe, warning, severity) = check_command_safety(command);
    Ok(json!({
        "safe": is_safe,
        "severity": severity,
        "warning": warning,
        "command": command
    }))
}

pub async fn audit_log(args: Value) -> Result<Value> {
    let lines = args["lines"].as_u64().unwrap_or(20) as usize;
    match std::fs::read_to_string(AUDIT_LOG) {
        Ok(content) => {
            let entries: Vec<&str> = content.lines().rev().take(lines).collect();
            Ok(json!({ "entries": entries, "count": entries.len(), "log_path": AUDIT_LOG }))
        },
        Err(_) => Ok(json!({ "entries": [], "count": 0, "note": "No audit log yet" }))
    }
}
