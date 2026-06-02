//! Programmer-Wander — MCP server giving any AI a complete Rust + Windows dev shell.
//!
//! Subcommands:
//!   programmer            -> run stdio MCP server (default, same as `serve`)
//!   programmer serve      -> run stdio MCP server explicitly
//!   programmer install --target <host>     -> register with host config
//!   programmer uninstall --target <host>   -> unregister from host config
//!   programmer --version | -V              -> print version
//!
//! Supported install targets: claude-desktop, claude-code, lm-studio, all
//!
//! See https://github.com/AIWander/Programmer-Wander for documentation.

mod mcp;
mod tools;

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use tracing::Level;

const SERVER_KEY: &str = "programmer-wander";

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let sub = args.get(1).map(|s| s.as_str());

    match sub {
        Some("--version") | Some("-V") => {
            println!("programmer {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some("--help") | Some("-h") => {
            print_help();
            Ok(())
        }
        Some("install") => install_or_uninstall(&args[2..], false),
        Some("uninstall") => install_or_uninstall(&args[2..], true),
        Some("serve") | None => run_serve().await,
        Some(other) => {
            eprintln!("Unknown subcommand: {}", other);
            print_help();
            std::process::exit(2);
        }
    }
}

fn print_help() {
    println!("Programmer-Wander v{}", env!("CARGO_PKG_VERSION"));
    println!();
    println!("USAGE:");
    println!("  programmer                              Run stdio MCP server (default)");
    println!("  programmer serve                        Same as above");
    println!("  programmer install --target <host>      Register with host config");
    println!("  programmer uninstall --target <host>    Unregister from host config");
    println!("  programmer --version                    Print version");
    println!("  programmer --help                       Print this help");
    println!();
    println!("INSTALL TARGETS:");
    println!("  claude-desktop    %APPDATA%\\Claude\\claude_desktop_config.json");
    println!("  claude-code       %USERPROFILE%\\.claude\\settings.json");
    println!("  lm-studio         %USERPROFILE%\\.lmstudio\\mcp.json");
    println!("  all               All detected hosts");
    println!();
    println!("Repository: https://github.com/AIWander/Programmer-Wander");
}

async fn run_serve() -> Result<()> {
    tracing_subscriber::fmt()
        .with_max_level(Level::INFO)
        .with_writer(std::io::stderr)
        .init();

    if let Ok(workspace) = std::env::var("WORKSPACE_PATH") {
        if Path::new(&workspace).is_dir() {
            std::env::set_current_dir(&workspace).ok();
            tracing::info!("Workspace set to: {}", workspace);
        } else {
            tracing::warn!(
                "WORKSPACE_PATH '{}' is not a valid directory, ignoring",
                workspace
            );
        }
    }

    tracing::info!(
        "Programmer-Wander v{} starting (stdio MCP)...",
        env!("CARGO_PKG_VERSION")
    );
    mcp::run_stdio_server().await
}

// ---------------------------------------------------------------------------
// install / uninstall subcommand
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Target {
    ClaudeDesktop,
    ClaudeCode,
    LmStudio,
}

impl Target {
    fn all() -> &'static [Target] {
        &[Target::ClaudeDesktop, Target::ClaudeCode, Target::LmStudio]
    }

    fn parse(s: &str) -> Option<Vec<Target>> {
        match s {
            "claude-desktop" => Some(vec![Target::ClaudeDesktop]),
            "claude-code" => Some(vec![Target::ClaudeCode]),
            "lm-studio" => Some(vec![Target::LmStudio]),
            "all" => Some(Target::all().to_vec()),
            _ => None,
        }
    }

    fn name(&self) -> &'static str {
        match self {
            Target::ClaudeDesktop => "claude-desktop",
            Target::ClaudeCode => "claude-code",
            Target::LmStudio => "lm-studio",
        }
    }

    fn config_path(&self) -> Option<PathBuf> {
        match self {
            Target::ClaudeDesktop => {
                // Windows: %APPDATA%\Claude\claude_desktop_config.json
                dirs::config_dir().map(|p| p.join("Claude").join("claude_desktop_config.json"))
            }
            Target::ClaudeCode => {
                // %USERPROFILE%\.claude\settings.json
                dirs::home_dir().map(|p| p.join(".claude").join("settings.json"))
            }
            Target::LmStudio => {
                // %USERPROFILE%\.lmstudio\mcp.json
                dirs::home_dir().map(|p| p.join(".lmstudio").join("mcp.json"))
            }
        }
    }
}

fn install_or_uninstall(args: &[String], remove: bool) -> Result<()> {
    let target_str = parse_target_arg(args).context(
        "missing required --target <host> (one of: claude-desktop, claude-code, lm-studio, all)",
    )?;

    let targets = Target::parse(&target_str).with_context(|| {
        format!(
            "unknown target: '{}'. Valid: claude-desktop, claude-code, lm-studio, all",
            target_str
        )
    })?;

    let exe_path = std::env::current_exe().context("could not resolve current executable path")?;
    let exe_str = exe_path.to_string_lossy().to_string();

    let action = if remove { "uninstall" } else { "install" };
    println!(
        "{} target(s): {}",
        action,
        targets
            .iter()
            .map(|t| t.name())
            .collect::<Vec<_>>()
            .join(", ")
    );
    println!("exe path: {}", exe_str);
    println!();

    let mut any_success = false;
    let mut any_error = false;

    for target in &targets {
        match apply_target(*target, &exe_str, remove) {
            Ok(msg) => {
                println!("[OK]   {}: {}", target.name(), msg);
                any_success = true;
            }
            Err(e) => {
                println!("[SKIP] {}: {}", target.name(), e);
                if !is_skip_reason(&e) {
                    any_error = true;
                }
            }
        }
    }

    println!();
    if remove {
        println!(
            "Restart any host apps that had Programmer-Wander loaded for changes to take effect."
        );
    } else if any_success {
        println!("Restart any registered host apps for the new tools to appear.");
    }

    if any_error && !any_success {
        std::process::exit(1);
    }
    Ok(())
}

fn parse_target_arg(args: &[String]) -> Option<String> {
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--target" | "-t" => {
                return args.get(i + 1).cloned();
            }
            s if s.starts_with("--target=") => {
                return Some(s[9..].to_string());
            }
            _ => i += 1,
        }
    }
    None
}

fn is_skip_reason(e: &anyhow::Error) -> bool {
    let msg = format!("{}", e);
    msg.contains("not detected") || msg.contains("already absent")
}

fn apply_target(target: Target, exe_path: &str, remove: bool) -> Result<String> {
    let config_path = target
        .config_path()
        .with_context(|| format!("could not resolve config path for {}", target.name()))?;

    // Detect: parent directory must exist (means the host app is installed)
    let parent = config_path
        .parent()
        .with_context(|| format!("config path has no parent: {}", config_path.display()))?;

    if !parent.exists() {
        bail!("host not detected (no {} directory)", parent.display());
    }

    let mut config = read_or_init_config(&config_path)?;

    let servers_map = ensure_mcp_servers_map(&mut config);

    if remove {
        if servers_map.remove(SERVER_KEY).is_none() {
            bail!("entry already absent");
        }
    } else {
        servers_map.insert(
            SERVER_KEY.to_string(),
            json!({
                "command": exe_path,
                "args": []
            }),
        );
    }

    backup_if_exists(&config_path)?;
    write_config_pretty(&config_path, &config)?;

    Ok(format!("{}", config_path.display()))
}

fn read_or_init_config(path: &Path) -> Result<Value> {
    if !path.exists() {
        return Ok(json!({}));
    }
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("read failed: {}", path.display()))?;
    if text.trim().is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_str(&text).with_context(|| format!("invalid JSON in {}", path.display()))
}

fn ensure_mcp_servers_map(config: &mut Value) -> &mut serde_json::Map<String, Value> {
    let obj = config
        .as_object_mut()
        .expect("top-level config must be a JSON object");
    if !obj.contains_key("mcpServers") {
        obj.insert("mcpServers".to_string(), json!({}));
    }
    obj.get_mut("mcpServers")
        .and_then(|v| v.as_object_mut())
        .expect("mcpServers should be an object after insertion")
}

fn backup_if_exists(path: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let ts = chrono::Utc::now().format("%Y%m%d_%H%M%S");
    let backup = path.with_extension(format!("pre_{}.bak", ts));
    std::fs::copy(path, &backup)
        .with_context(|| format!("backup failed: {} -> {}", path.display(), backup.display()))?;
    Ok(())
}

fn write_config_pretty(path: &Path, config: &Value) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("could not create parent dir: {}", parent.display()))?;
    }
    let text =
        serde_json::to_string_pretty(config).context("failed to serialize config to JSON")?;
    std::fs::write(path, text).with_context(|| format!("write failed: {}", path.display()))?;
    Ok(())
}
