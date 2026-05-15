//! Programmer-Wander — MCP server giving any AI a complete Rust + Windows dev shell.
//!
//! v0.1.0-alpha: scaffold only. Real tool surface lands in subsequent commits.
//! See <https://github.com/AIWander/Programmer-Wander> for status and install instructions.

fn main() {
    let version = env!("CARGO_PKG_VERSION");
    let args: Vec<String> = std::env::args().collect();

    match args.get(1).map(String::as_str) {
        Some("--version") | Some("-V") => {
            println!("programmer {version}");
        }
        Some("install") => {
            eprintln!("install subcommand not yet implemented (scaffold v{version}).");
            eprintln!("See README: https://github.com/AIWander/Programmer-Wander#install");
            std::process::exit(2);
        }
        Some("uninstall") => {
            eprintln!("uninstall subcommand not yet implemented (scaffold v{version}).");
            std::process::exit(2);
        }
        _ => {
            eprintln!("programmer v{version} — scaffold, not yet functional.");
            eprintln!("See https://github.com/AIWander/Programmer-Wander for status.");
        }
    }
}
