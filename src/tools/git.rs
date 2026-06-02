//! Git Operations

use anyhow::Result;
use serde_json::{json, Value};
use std::process::Stdio;
use tokio::process::Command;

/// Get git status
pub async fn status(args: Value) -> Result<Value> {
    let repo_path = args
        .get("repo_path")
        .and_then(|v| v.as_str())
        .unwrap_or(".");

    // Get branch
    let branch_output = Command::new("git")
        .args(["branch", "--show-current"])
        .current_dir(repo_path)
        .output()
        .await?;
    let branch = String::from_utf8_lossy(&branch_output.stdout)
        .trim()
        .to_string();

    // Get status
    let status_output = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(repo_path)
        .output()
        .await?;

    let status_str = String::from_utf8_lossy(&status_output.stdout);

    let mut modified = Vec::new();
    let mut staged = Vec::new();
    let mut untracked = Vec::new();

    for line in status_str.lines() {
        if line.len() < 3 {
            continue;
        }
        let status = &line[0..2];
        let file = line[3..].to_string();

        match status.chars().next() {
            Some('M') | Some('A') | Some('D') | Some('R') => staged.push(file.clone()),
            _ => {}
        }
        match status.chars().nth(1) {
            Some('M') => modified.push(file.clone()),
            Some('?') => untracked.push(file),
            _ => {}
        }
    }

    // Get ahead/behind
    let ahead_behind = Command::new("git")
        .args(["rev-list", "--left-right", "--count", "HEAD...@{upstream}"])
        .current_dir(repo_path)
        .output()
        .await
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());

    let (ahead, behind) = if let Some(ab) = ahead_behind {
        let parts: Vec<&str> = ab.split_whitespace().collect();
        (
            parts.first().and_then(|s| s.parse().ok()).unwrap_or(0),
            parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0),
        )
    } else {
        (0, 0)
    };

    Ok(json!({
        "success": true,
        "branch": branch,
        "modified": modified,
        "staged": staged,
        "untracked": untracked,
        "ahead": ahead,
        "behind": behind,
        "clean": modified.is_empty() && staged.is_empty() && untracked.is_empty()
    }))
}

/// Get git diff
pub async fn diff(args: Value) -> Result<Value> {
    let repo_path = args
        .get("repo_path")
        .and_then(|v| v.as_str())
        .unwrap_or(".");
    let staged = args
        .get("staged")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let file = args.get("file").and_then(|v| v.as_str());

    let mut cmd_args = vec!["diff"];
    if staged {
        cmd_args.push("--cached");
    }
    if let Some(f) = file {
        cmd_args.push("--");
        cmd_args.push(f);
    }

    let output = Command::new("git")
        .args(&cmd_args)
        .current_dir(repo_path)
        .output()
        .await?;

    let diff_str = String::from_utf8_lossy(&output.stdout).to_string();

    // Count additions/deletions
    let additions = diff_str
        .lines()
        .filter(|l| l.starts_with('+') && !l.starts_with("+++"))
        .count();
    let deletions = diff_str
        .lines()
        .filter(|l| l.starts_with('-') && !l.starts_with("---"))
        .count();

    Ok(json!({
        "success": true,
        "diff": diff_str,
        "additions": additions,
        "deletions": deletions,
        "staged": staged
    }))
}

/// Create commit
pub async fn commit(args: Value) -> Result<Value> {
    let repo_path = args
        .get("repo_path")
        .and_then(|v| v.as_str())
        .unwrap_or(".");
    let message = args.get("message").and_then(|v| v.as_str());
    let files: Option<Vec<&str>> = args
        .get("files")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect());

    // Stage files if specified
    if let Some(files) = files {
        for file in files {
            Command::new("git")
                .args(["add", file])
                .current_dir(repo_path)
                .output()
                .await?;
        }
    } else {
        // Stage all
        Command::new("git")
            .args(["add", "-A"])
            .current_dir(repo_path)
            .output()
            .await?;
    }

    // Commit
    let mut commit_args = vec!["commit"];
    if let Some(msg) = message {
        commit_args.push("-m");
        commit_args.push(msg);
    } else {
        commit_args.push("--allow-empty-message");
        commit_args.push("-m");
        commit_args.push("");
    }

    let output = Command::new("git")
        .args(&commit_args)
        .current_dir(repo_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await?;

    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        Ok(json!({
            "success": true,
            "output": stdout,
            "message": message
        }))
    } else {
        Ok(json!({
            "success": false,
            "error": String::from_utf8_lossy(&output.stderr).to_string()
        }))
    }
}

/// Push to remote
pub async fn push(args: Value) -> Result<Value> {
    let repo_path = args
        .get("repo_path")
        .and_then(|v| v.as_str())
        .unwrap_or(".");
    let remote = args
        .get("remote")
        .and_then(|v| v.as_str())
        .unwrap_or("origin");
    let branch = args.get("branch").and_then(|v| v.as_str());

    let mut cmd_args = vec!["push", remote];
    if let Some(b) = branch {
        cmd_args.push(b);
    }

    let output = Command::new("git")
        .args(&cmd_args)
        .current_dir(repo_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await?;

    if output.status.success() {
        Ok(json!({
            "success": true,
            "remote": remote,
            "output": String::from_utf8_lossy(&output.stderr).to_string()
        }))
    } else {
        Ok(json!({
            "success": false,
            "error": String::from_utf8_lossy(&output.stderr).to_string()
        }))
    }
}

/// Pull from remote
pub async fn pull(args: Value) -> Result<Value> {
    let repo_path = args
        .get("repo_path")
        .and_then(|v| v.as_str())
        .unwrap_or(".");
    let remote = args
        .get("remote")
        .and_then(|v| v.as_str())
        .unwrap_or("origin");

    let output = Command::new("git")
        .args(["pull", remote])
        .current_dir(repo_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await?;

    if output.status.success() {
        Ok(json!({
            "success": true,
            "output": String::from_utf8_lossy(&output.stdout).to_string()
        }))
    } else {
        Ok(json!({
            "success": false,
            "error": String::from_utf8_lossy(&output.stderr).to_string()
        }))
    }
}

/// Get commit log
pub async fn log(args: Value) -> Result<Value> {
    let repo_path = args
        .get("repo_path")
        .and_then(|v| v.as_str())
        .unwrap_or(".");
    let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(10);

    let output = Command::new("git")
        .args([
            "log",
            &format!("-{}", limit),
            "--pretty=format:%H|%h|%an|%ae|%at|%s",
        ])
        .current_dir(repo_path)
        .output()
        .await?;

    let log_str = String::from_utf8_lossy(&output.stdout);
    let commits: Vec<Value> = log_str
        .lines()
        .filter(|l| !l.is_empty())
        .map(|line| {
            let parts: Vec<&str> = line.splitn(6, '|').collect();
            json!({
                "hash": parts.first().unwrap_or(&""),
                "short_hash": parts.get(1).unwrap_or(&""),
                "author": parts.get(2).unwrap_or(&""),
                "email": parts.get(3).unwrap_or(&""),
                "timestamp": parts.get(4).and_then(|s| s.parse::<u64>().ok()),
                "message": parts.get(5).unwrap_or(&"")
            })
        })
        .collect();

    Ok(json!({
        "success": true,
        "commits": commits,
        "count": commits.len()
    }))
}

/// List/create/delete branches
pub async fn branch(args: Value) -> Result<Value> {
    let repo_path = args
        .get("repo_path")
        .and_then(|v| v.as_str())
        .unwrap_or(".");
    let name = args.get("name").and_then(|v| v.as_str());
    let delete = args
        .get("delete")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    if let Some(branch_name) = name {
        if delete {
            let output = Command::new("git")
                .args(["branch", "-d", branch_name])
                .current_dir(repo_path)
                .output()
                .await?;

            return Ok(json!({
                "success": output.status.success(),
                "action": "deleted",
                "branch": branch_name,
                "error": if output.status.success() { None } else { Some(String::from_utf8_lossy(&output.stderr).to_string()) }
            }));
        } else {
            let output = Command::new("git")
                .args(["branch", branch_name])
                .current_dir(repo_path)
                .output()
                .await?;

            return Ok(json!({
                "success": output.status.success(),
                "action": "created",
                "branch": branch_name,
                "error": if output.status.success() { None } else { Some(String::from_utf8_lossy(&output.stderr).to_string()) }
            }));
        }
    }

    // List branches
    let output = Command::new("git")
        .args(["branch", "-a"])
        .current_dir(repo_path)
        .output()
        .await?;

    let branches: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|l| l.trim_start_matches("* ").trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();

    let current = Command::new("git")
        .args(["branch", "--show-current"])
        .current_dir(repo_path)
        .output()
        .await?;

    Ok(json!({
        "success": true,
        "branches": branches,
        "current": String::from_utf8_lossy(&current.stdout).trim()
    }))
}

/// Switch branch or restore file
pub async fn checkout(args: Value) -> Result<Value> {
    let repo_path = args
        .get("repo_path")
        .and_then(|v| v.as_str())
        .unwrap_or(".");
    let branch_name = args.get("branch").and_then(|v| v.as_str());
    let file = args.get("file").and_then(|v| v.as_str());
    let create = args
        .get("create")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    if let Some(f) = file {
        // Restore file
        let output = Command::new("git")
            .args(["checkout", "--", f])
            .current_dir(repo_path)
            .output()
            .await?;

        return Ok(json!({
            "success": output.status.success(),
            "action": "restored",
            "file": f
        }));
    }

    if let Some(b) = branch_name {
        let mut cmd_args = vec!["checkout"];
        if create {
            cmd_args.push("-b");
        }
        cmd_args.push(b);

        let output = Command::new("git")
            .args(&cmd_args)
            .current_dir(repo_path)
            .output()
            .await?;

        return Ok(json!({
            "success": output.status.success(),
            "action": if create { "created_and_switched" } else { "switched" },
            "branch": b,
            "error": if output.status.success() { None } else { Some(String::from_utf8_lossy(&output.stderr).to_string()) }
        }));
    }

    Ok(json!({
        "success": false,
        "error": "No branch or file specified"
    }))
}

/// Manage git stash
pub async fn stash(args: Value) -> Result<Value> {
    let repo_path = args
        .get("repo_path")
        .and_then(|v| v.as_str())
        .unwrap_or(".");
    let action = args
        .get("action")
        .and_then(|v| v.as_str())
        .unwrap_or("push");
    let message = args.get("message").and_then(|v| v.as_str());

    match action {
        "push" => {
            let mut cmd_args = vec!["stash", "push"];
            if let Some(msg) = message {
                cmd_args.push("-m");
                cmd_args.push(msg);
            }

            let output = Command::new("git")
                .args(&cmd_args)
                .current_dir(repo_path)
                .output()
                .await?;

            Ok(json!({
                "success": output.status.success(),
                "action": "push",
                "output": String::from_utf8_lossy(&output.stdout).to_string()
            }))
        }
        "pop" => {
            let output = Command::new("git")
                .args(["stash", "pop"])
                .current_dir(repo_path)
                .output()
                .await?;

            Ok(json!({
                "success": output.status.success(),
                "action": "pop",
                "output": String::from_utf8_lossy(&output.stdout).to_string()
            }))
        }
        "list" => {
            let output = Command::new("git")
                .args(["stash", "list"])
                .current_dir(repo_path)
                .output()
                .await?;

            let stashes: Vec<String> = String::from_utf8_lossy(&output.stdout)
                .lines()
                .map(|l| l.to_string())
                .collect();

            Ok(json!({
                "success": true,
                "action": "list",
                "stashes": stashes,
                "count": stashes.len()
            }))
        }
        "drop" => {
            let output = Command::new("git")
                .args(["stash", "drop"])
                .current_dir(repo_path)
                .output()
                .await?;

            Ok(json!({
                "success": output.status.success(),
                "action": "drop",
                "output": String::from_utf8_lossy(&output.stdout).to_string()
            }))
        }
        _ => Ok(json!({
            "success": false,
            "error": format!("Unknown action: {}", action)
        })),
    }
}

/// Get structured diff summary for AI commit messages
pub async fn diff_summary(args: Value) -> Result<Value> {
    let repo_path = args
        .get("repo_path")
        .and_then(|v| v.as_str())
        .unwrap_or(".");

    // Get stat summary
    let stat_output = Command::new("git")
        .args(["diff", "--stat", "--cached"])
        .current_dir(repo_path)
        .output()
        .await?;

    let stat_str = String::from_utf8_lossy(&stat_output.stdout).to_string();

    // Get file list with status
    let files_output = Command::new("git")
        .args(["diff", "--name-status", "--cached"])
        .current_dir(repo_path)
        .output()
        .await?;

    let files_str = String::from_utf8_lossy(&files_output.stdout);

    let mut added = Vec::new();
    let mut modified = Vec::new();
    let mut deleted = Vec::new();

    for line in files_str.lines() {
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() >= 2 {
            let file = parts[1].to_string();
            match parts[0] {
                "A" => added.push(file),
                "M" => modified.push(file),
                "D" => deleted.push(file),
                _ => modified.push(file),
            }
        }
    }

    // Group by extension
    let mut by_extension: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    for file in added.iter().chain(modified.iter()) {
        let ext = std::path::Path::new(file)
            .extension()
            .map(|e| e.to_string_lossy().to_string())
            .unwrap_or_else(|| "no_extension".to_string());
        by_extension.entry(ext).or_default().push(file.clone());
    }

    Ok(json!({
        "success": true,
        "summary": {
            "added_files": added.len(),
            "modified_files": modified.len(),
            "deleted_files": deleted.len(),
            "total_files": added.len() + modified.len() + deleted.len()
        },
        "files": {
            "added": added,
            "modified": modified,
            "deleted": deleted
        },
        "by_extension": by_extension,
        "stat": stat_str
    }))
}

/// Create .agent/workflows/*.md file
#[allow(dead_code)]
pub async fn create_workflow(args: Value) -> Result<Value> {
    let repo_path = args
        .get("repo_path")
        .and_then(|v| v.as_str())
        .unwrap_or(".");
    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("workflow");
    let description = args
        .get("description")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let steps: Vec<String> = args
        .get("steps")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    let workflow_dir = std::path::Path::new(repo_path)
        .join(".agent")
        .join("workflows");
    std::fs::create_dir_all(&workflow_dir)?;

    let mut content = format!("# {}\n\n", name);
    if !description.is_empty() {
        content.push_str(&format!("{}\n\n", description));
    }
    content.push_str("## Steps\n\n");
    for (i, step) in steps.iter().enumerate() {
        content.push_str(&format!("{}. {}\n", i + 1, step));
    }

    let file_path = workflow_dir.join(format!("{}.md", name.replace(" ", "_").to_lowercase()));
    std::fs::write(&file_path, &content)?;

    Ok(json!({
        "success": true,
        "path": file_path.to_string_lossy(),
        "name": name,
        "steps": steps.len()
    }))
}

/// Create .agent/rules/*.md file
#[allow(dead_code)]
pub async fn create_rule(args: Value) -> Result<Value> {
    let repo_path = args
        .get("repo_path")
        .and_then(|v| v.as_str())
        .unwrap_or(".");
    let name = args.get("name").and_then(|v| v.as_str()).unwrap_or("rule");
    let description = args
        .get("description")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let conditions: Vec<String> = args
        .get("conditions")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let actions: Vec<String> = args
        .get("actions")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    let rules_dir = std::path::Path::new(repo_path).join(".agent").join("rules");
    std::fs::create_dir_all(&rules_dir)?;

    let mut content = format!("# {}\n\n", name);
    if !description.is_empty() {
        content.push_str(&format!("{}\n\n", description));
    }

    if !conditions.is_empty() {
        content.push_str("## Conditions\n\n");
        for cond in &conditions {
            content.push_str(&format!("- {}\n", cond));
        }
        content.push('\n');
    }

    if !actions.is_empty() {
        content.push_str("## Actions\n\n");
        for action in &actions {
            content.push_str(&format!("- {}\n", action));
        }
    }

    let file_path = rules_dir.join(format!("{}.md", name.replace(" ", "_").to_lowercase()));
    std::fs::write(&file_path, &content)?;

    Ok(json!({
        "success": true,
        "path": file_path.to_string_lossy(),
        "name": name,
        "conditions": conditions.len(),
        "actions": actions.len()
    }))
}

pub async fn clone(args: Value) -> Result<Value> {
    let url = args.get("url").and_then(|v| v.as_str()).unwrap_or("");
    let destination = args.get("destination").and_then(|v| v.as_str());
    let branch = args.get("branch").and_then(|v| v.as_str());

    if url.is_empty() {
        anyhow::bail!("url is required");
    }

    let mut cmd_args = vec!["clone".to_string()];
    if let Some(b) = branch {
        cmd_args.push("--branch".to_string());
        cmd_args.push(b.to_string());
    }
    cmd_args.push(url.to_string());
    if let Some(dest) = destination {
        cmd_args.push(dest.to_string());
    }

    let output = tokio::process::Command::new("git")
        .args(&cmd_args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .await?;

    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
    .trim()
    .to_string();

    if output.status.success() {
        Ok(json!({"success": true, "url": url, "output": combined}))
    } else {
        Ok(json!({"error": combined}))
    }
}

pub async fn remote(args: Value) -> Result<Value> {
    let repo = args
        .get("repo_path")
        .and_then(|v| v.as_str())
        .unwrap_or(".");
    let action = args
        .get("action")
        .and_then(|v| v.as_str())
        .unwrap_or("list");
    let name = args.get("name").and_then(|v| v.as_str());
    let url = args.get("url").and_then(|v| v.as_str());

    match action {
        "list" => {
            let output = tokio::process::Command::new("git")
                .args(["-C", repo, "remote", "-v"])
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .output()
                .await?;
            let remotes = String::from_utf8_lossy(&output.stdout).to_string();
            Ok(json!({"remotes": remotes.trim()}))
        }
        "add" => {
            let n = name.ok_or_else(|| anyhow::anyhow!("name required for add"))?;
            let u = url.ok_or_else(|| anyhow::anyhow!("url required for add"))?;
            let output = tokio::process::Command::new("git")
                .args(["-C", repo, "remote", "add", n, u])
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .output()
                .await?;
            if output.status.success() {
                Ok(json!({"added": n, "url": u}))
            } else {
                Ok(json!({"error": String::from_utf8_lossy(&output.stderr).to_string()}))
            }
        }
        "remove" => {
            let n = name.ok_or_else(|| anyhow::anyhow!("name required for remove"))?;
            let output = tokio::process::Command::new("git")
                .args(["-C", repo, "remote", "remove", n])
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .output()
                .await?;
            if output.status.success() {
                Ok(json!({"removed": n}))
            } else {
                Ok(json!({"error": String::from_utf8_lossy(&output.stderr).to_string()}))
            }
        }
        _ => Ok(json!({"error": format!("Unknown action: {}. Use list, add, or remove", action)})),
    }
}
