use anyhow::{bail, Result};
use clap::Subcommand;
use std::process::Command;

use crate::{creds, llm, memory};

// ── Subcommand enum ───────────────────────────────────────────────────────────

#[derive(Subcommand)]
pub enum DepsAction {
    /// Run security audits across detected package managers
    Audit {
        /// Send audit output to LLM for risk triage
        #[arg(long)]
        triage: bool,
    },
    /// Check for outdated dependencies across detected package managers
    Outdated,
    /// Natural language query — dclaw deps "<what you want to do>"
    #[command(external_subcommand)]
    Nl(Vec<String>),
}

// ── Package manager detection ─────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum PackageManager {
    Cargo,
    Npm,
    Go,
    Pip,
}

impl PackageManager {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Cargo => "cargo",
            Self::Npm => "npm",
            Self::Go => "go",
            Self::Pip => "pip",
        }
    }

    pub fn audit_cmd(&self) -> (&'static str, &'static [&'static str]) {
        match self {
            Self::Cargo => ("cargo", &["audit"]),
            Self::Npm => ("npm", &["audit"]),
            Self::Go => ("govulncheck", &["./..."]),
            Self::Pip => ("pip-audit", &[]),
        }
    }

    pub fn outdated_cmd(&self) -> (&'static str, &'static [&'static str]) {
        match self {
            Self::Cargo => ("cargo", &["outdated"]),
            Self::Npm => ("npm", &["outdated"]),
            Self::Go => ("go", &["list", "-u", "-m", "all"]),
            Self::Pip => ("pip", &["list", "--outdated"]),
        }
    }
}

pub fn detect_package_managers() -> Vec<PackageManager> {
    let dir = std::env::current_dir().unwrap_or_default();
    detect_package_managers_in(&dir)
}

pub fn detect_package_managers_in(dir: &std::path::Path) -> Vec<PackageManager> {
    let mut found = Vec::new();
    if dir.join("Cargo.toml").exists() {
        found.push(PackageManager::Cargo);
    }
    if dir.join("package.json").exists() {
        found.push(PackageManager::Npm);
    }
    if dir.join("go.mod").exists() {
        found.push(PackageManager::Go);
    }
    if dir.join("pyproject.toml").exists() || dir.join("requirements.txt").exists() {
        found.push(PackageManager::Pip);
    }
    found
}

// ── Public entry point ────────────────────────────────────────────────────────

pub async fn run(action: DepsAction) -> Result<()> {
    match action {
        DepsAction::Audit { triage } => audit(triage).await,
        DepsAction::Outdated => outdated().await,
        DepsAction::Nl(_) => unreachable!("Nl handled in main"),
    }
}

// ── Audit subcommand ──────────────────────────────────────────────────────────

async fn audit(triage: bool) -> Result<()> {
    let managers = detect_package_managers();
    if managers.is_empty() {
        bail!("No supported package manager detected (Cargo.toml / package.json / go.mod / pyproject.toml / requirements.txt)");
    }
    let mut all_output = String::new();
    for pm in &managers {
        let (cmd, args) = pm.audit_cmd();
        println!("── {} audit ──────────────────────────────", pm.name());
        let out = run_tool(cmd, args);
        println!("{out}");
        all_output.push_str(&format!("=== {} audit ===\n{out}\n", pm.name()));
    }
    if triage {
        println!("── LLM triage ────────────────────────────");
        let analysis = llm_triage(&all_output).await?;
        println!("{analysis}");
    }
    Ok(())
}

// ── Outdated subcommand ───────────────────────────────────────────────────────

async fn outdated() -> Result<()> {
    let managers = detect_package_managers();
    if managers.is_empty() {
        bail!("No supported package manager detected (Cargo.toml / package.json / go.mod / pyproject.toml / requirements.txt)");
    }
    for pm in &managers {
        let (cmd, args) = pm.outdated_cmd();
        println!("── {} outdated ────────────────────────────", pm.name());
        let out = run_tool(cmd, args);
        println!("{out}");
    }
    Ok(())
}

// ── Tool runner ───────────────────────────────────────────────────────────────

pub fn run_tool(cmd: &str, args: &[&str]) -> String {
    match Command::new(cmd).args(args).output() {
        Err(e) => format!("[{cmd} not found: {e}]"),
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            if stdout.trim().is_empty() {
                stderr.trim().to_string()
            } else {
                stdout.trim().to_string()
            }
        }
    }
}

// ── LLM triage ────────────────────────────────────────────────────────────────

const MAX_AUDIT_CHARS: usize = 10_000;

fn truncate_audit(s: &str) -> String {
    if s.chars().count() <= MAX_AUDIT_CHARS {
        return s.to_string();
    }
    let trimmed: String = s.chars().take(MAX_AUDIT_CHARS).collect();
    format!("{trimmed}\n[... truncated at {MAX_AUDIT_CHARS} chars]")
}

async fn llm_triage(audit_output: &str) -> Result<String> {
    let ctx = memory::context_for_prompt("deps");
    let provider = resolve_provider()?;
    let api_key = creds::load(&provider)?;
    let client = llm::client_for(&provider, &api_key);
    let base_system = "You are a security engineer triaging dependency vulnerabilities. \
        For each vulnerability found, assess: severity (Critical/High/Medium/Low), \
        whether it is exploitable in a typical web/CLI context, and whether an upgrade is available. \
        Conclude with a ranked action list: what to fix now vs. later vs. ignore. \
        Be concise and specific.";
    let system = format!("{ctx}{base_system}");
    let prompt = format!(
        "Audit output from package managers:\n\n{}\n\nTriage these findings.",
        truncate_audit(audit_output)
    );
    let result = client.complete(&system, &prompt).await?;
    memory::record_interaction("deps", &result);
    Ok(result)
}

fn resolve_provider() -> Result<String> {
    if let Ok(p) = std::env::var("DEV_CLAW_PROVIDER") {
        return Ok(p);
    }
    creds::auto_detect_provider().ok_or_else(|| {
        anyhow::anyhow!("No API provider configured. Run: dclaw config set-key --provider deepseek")
    })
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn detect_none_in_empty_dir() {
        let d = TempDir::new().unwrap();
        let found = detect_package_managers_in(d.path());
        assert!(
            found.is_empty(),
            "expected no package managers, got {found:?}"
        );
    }

    #[test]
    fn detect_cargo() {
        let d = TempDir::new().unwrap();
        std::fs::write(d.path().join("Cargo.toml"), "[package]").unwrap();
        let found = detect_package_managers_in(d.path());
        assert!(found.contains(&PackageManager::Cargo));
    }

    #[test]
    fn detect_npm() {
        let d = TempDir::new().unwrap();
        std::fs::write(d.path().join("package.json"), "{}").unwrap();
        let found = detect_package_managers_in(d.path());
        assert!(found.contains(&PackageManager::Npm));
    }

    #[test]
    fn detect_go() {
        let d = TempDir::new().unwrap();
        std::fs::write(d.path().join("go.mod"), "module example.com/foo\n\ngo 1.21").unwrap();
        let found = detect_package_managers_in(d.path());
        assert!(found.contains(&PackageManager::Go));
    }

    #[test]
    fn detect_pip_via_requirements() {
        let d = TempDir::new().unwrap();
        std::fs::write(d.path().join("requirements.txt"), "requests==2.28.0").unwrap();
        let found = detect_package_managers_in(d.path());
        assert!(found.contains(&PackageManager::Pip));
    }

    #[test]
    fn detect_pip_via_pyproject() {
        let d = TempDir::new().unwrap();
        std::fs::write(d.path().join("pyproject.toml"), "[project]").unwrap();
        let found = detect_package_managers_in(d.path());
        assert!(found.contains(&PackageManager::Pip));
    }

    #[test]
    fn detect_multiple_managers() {
        let d = TempDir::new().unwrap();
        std::fs::write(d.path().join("Cargo.toml"), "[package]").unwrap();
        std::fs::write(d.path().join("package.json"), "{}").unwrap();
        let found = detect_package_managers_in(d.path());
        assert!(found.contains(&PackageManager::Cargo));
        assert!(found.contains(&PackageManager::Npm));
    }

    #[test]
    fn package_manager_names() {
        assert_eq!(PackageManager::Cargo.name(), "cargo");
        assert_eq!(PackageManager::Npm.name(), "npm");
        assert_eq!(PackageManager::Go.name(), "go");
        assert_eq!(PackageManager::Pip.name(), "pip");
    }

    #[test]
    fn run_tool_returns_output_for_echo() {
        let out = run_tool("echo", &["hello"]);
        assert_eq!(out.trim(), "hello");
    }

    #[test]
    fn run_tool_missing_binary_returns_error_msg() {
        let out = run_tool("__no_such_binary__", &[]);
        assert!(out.starts_with('['));
    }

    #[test]
    fn audit_cmd_cargo_uses_cargo_audit() {
        let (cmd, args) = PackageManager::Cargo.audit_cmd();
        assert_eq!(cmd, "cargo");
        assert!(args.contains(&"audit"));
    }

    #[test]
    fn outdated_cmd_npm_uses_npm_outdated() {
        let (cmd, args) = PackageManager::Npm.outdated_cmd();
        assert_eq!(cmd, "npm");
        assert!(args.contains(&"outdated"));
    }

    #[test]
    fn audit_cmd_go_uses_govulncheck() {
        let (cmd, _) = PackageManager::Go.audit_cmd();
        assert_eq!(cmd, "govulncheck");
    }

    #[test]
    fn audit_cmd_pip_uses_pip_audit() {
        let (cmd, _) = PackageManager::Pip.audit_cmd();
        assert_eq!(cmd, "pip-audit");
    }
}
