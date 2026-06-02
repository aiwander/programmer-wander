# Security Policy

## Reporting a vulnerability

If you find a security issue in Programmer-Wander, use GitHub's private vulnerability reporting for this repository if available. If that is not available, email **josephwander@gmail.com** directly. **Do not** open a public GitHub issue.

We aim to:
- Acknowledge within 72 hours
- Triage within 7 days
- Ship a fix or mitigation for high-severity issues within 14 days

## In scope

- The `programmer.exe` binary
- The `install` / `uninstall` subcommands and their config-file edits
- The `./.programmer/` state directory (permissions, contents)
- Shipping artifacts on the Releases page (zip / MSI)

## Out of scope

- Third-party MCP host apps (Claude Desktop, LM Studio, Cowork, Claude Code)
- The user's host operating system
- Issues in Rust, Cargo, or third-party crates (report those upstream)

## Disclosure

After a fix lands, we'll credit the reporter (with permission) in release notes.
