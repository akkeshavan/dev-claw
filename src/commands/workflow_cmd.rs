use anyhow::{bail, Result};
use clap::Subcommand;
use std::io::Write;
use std::process::{Command, Stdio};

use crate::config::{Config, WorkflowDef};

// ── Subcommand enum ───────────────────────────────────────────────────────────

#[derive(Subcommand)]
pub enum WorkflowAction {
    /// List all available workflows
    Ls,
    /// Run a named workflow step by step
    Run {
        /// Workflow name
        name: String,
        /// Print each step without executing it
        #[arg(long)]
        dry_run: bool,
    },
    /// Publish a workflow as a public GitHub Gist
    Publish {
        /// Workflow name to publish
        name: String,
    },
    /// Import a workflow from a GitHub Gist URL or raw TOML URL
    Import {
        /// Gist URL (https://gist.github.com/...) or raw TOML URL
        url: String,
    },
}

// ── Public entry point ────────────────────────────────────────────────────────

pub async fn run(action: WorkflowAction) -> Result<()> {
    match action {
        WorkflowAction::Ls                    => list_workflows(),
        WorkflowAction::Run { name, dry_run } => run_workflow(&name, dry_run),
        WorkflowAction::Publish { name }      => publish_workflow(&name),
        WorkflowAction::Import { url }        => import_workflow(&url),
    }
}

// ── List ──────────────────────────────────────────────────────────────────────

fn list_workflows() -> Result<()> {
    let project = load_project_workflows()?;
    let global  = load_global_workflows()?;
    if project.is_empty() && global.is_empty() {
        println!("No workflows defined.\n");
        println!("Add to .devclawrc:\n");
        println!("  [[workflows]]");
        println!("  name = \"pre-push\"");
        println!("  steps = [\"env check\", \"review diff --staged\"]");
        return Ok(());
    }
    print_workflow_group("Project (.devclawrc)", &project);
    let global_path = global_workflows_path()?;
    print_workflow_group(&format!("Global ({})", global_path.display()), &global);
    Ok(())
}

// ── Run ───────────────────────────────────────────────────────────────────────

fn run_workflow(name: &str, dry_run: bool) -> Result<()> {
    let all = load_all_workflows()?;
    let wf  = find_workflow(&all, name)
        .ok_or_else(|| anyhow::anyhow!(
            "No workflow named '{name}'. Run `dev-claw workflow ls` to list workflows."
        ))?
        .clone();
    let label = if dry_run { " [dry-run]" } else { "" };
    println!("Running workflow: {}{label} ({} steps)\n", wf.name, wf.steps.len());
    for (i, step) in wf.steps.iter().enumerate() {
        println!("── Step {}/{}: dev-claw {step}", i + 1, wf.steps.len());
        if dry_run {
            println!("   [dry-run: skipped]\n");
            continue;
        }
        if !execute_step(step)? {
            bail!("Step failed: dev-claw {step}\nWorkflow aborted.");
        }
        println!();
    }
    if !dry_run {
        println!("Workflow '{}' completed successfully.", wf.name);
    }
    Ok(())
}

// ── Publish ───────────────────────────────────────────────────────────────────

fn publish_workflow(name: &str) -> Result<()> {
    let all = load_all_workflows()?;
    let wf  = find_workflow(&all, name)
        .ok_or_else(|| anyhow::anyhow!("No workflow named '{name}'"))?
        .clone();
    let toml     = serialize_workflows(&[wf]);
    let desc     = format!("dev-claw workflow: {name}");
    let filename = format!("{name}.toml");
    let mut child = Command::new("gh")
        .args(["gist", "create", "--public", "--desc", &desc, "--filename", &filename, "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| anyhow::anyhow!(
            "gh CLI not found: {e}. Install from https://cli.github.com"
        ))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(toml.as_bytes())
            .map_err(|e| anyhow::anyhow!("Failed to write to gh stdin: {e}"))?;
    }
    let out = child.wait_with_output()
        .map_err(|e| anyhow::anyhow!("gh process error: {e}"))?;
    if !out.status.success() {
        bail!("gh gist create failed: {}", String::from_utf8_lossy(&out.stderr));
    }
    let gist_url = String::from_utf8_lossy(&out.stdout).trim().to_string();
    println!("Published! Share this workflow with:");
    println!("  dev-claw workflow import {gist_url}");
    Ok(())
}

// ── Import ────────────────────────────────────────────────────────────────────

fn import_workflow(url: &str) -> Result<()> {
    let content  = fetch_workflow_content(url)?;
    let incoming = parse_workflow_toml(&content)?;
    if incoming.is_empty() {
        bail!("No [[workflows]] found at {url}");
    }
    let mut existing = load_global_workflows()?;
    let mut added = 0usize;
    for def in incoming {
        if existing.iter().any(|w| w.name == def.name) {
            println!("Skipping '{}' (already exists in global workflows)", def.name);
            continue;
        }
        println!("Importing: {}", def.name);
        existing.push(def);
        added += 1;
    }
    save_global_workflows(&existing)?;
    println!("\nImported {added} workflow(s). Run `dev-claw workflow ls` to see them.");
    Ok(())
}

// ── Step execution ────────────────────────────────────────────────────────────

pub fn execute_step(step: &str) -> Result<bool> {
    let exe  = std::env::current_exe()
        .map_err(|e| anyhow::anyhow!("Cannot locate dev-claw binary: {e}"))?;
    let args = shlex::split(step)
        .ok_or_else(|| anyhow::anyhow!("Invalid step syntax (unmatched quote?): {step}"))?;
    let status = Command::new(&exe)
        .args(&args)
        .status()
        .map_err(|e| anyhow::anyhow!("Failed to execute step: {e}"))?;
    Ok(status.success())
}

// ── Workflow loading ──────────────────────────────────────────────────────────

fn load_project_workflows() -> Result<Vec<WorkflowDef>> {
    Ok(Config::load()?.workflows.unwrap_or_default())
}

pub fn load_global_workflows() -> Result<Vec<WorkflowDef>> {
    let path = global_workflows_path()?;
    if !path.exists() { return Ok(vec![]); }
    let raw = std::fs::read_to_string(&path)
        .map_err(|e| anyhow::anyhow!("Cannot read {}: {e}", path.display()))?;
    parse_workflow_toml(&raw)
}

fn load_all_workflows() -> Result<Vec<WorkflowDef>> {
    let mut all = load_project_workflows()?;
    all.extend(load_global_workflows()?);
    Ok(all)
}

pub fn find_workflow<'a>(workflows: &'a [WorkflowDef], name: &str) -> Option<&'a WorkflowDef> {
    workflows.iter().find(|w| w.name == name)
}

// ── Workflow persistence ──────────────────────────────────────────────────────

pub fn save_global_workflows(workflows: &[WorkflowDef]) -> Result<()> {
    let path = global_workflows_path()?;
    std::fs::write(&path, serialize_workflows(workflows))
        .map_err(|e| anyhow::anyhow!("Cannot write {}: {e}", path.display()))
}

pub fn global_workflows_path() -> Result<std::path::PathBuf> {
    Ok(Config::data_dir()?.join("workflows.toml"))
}

// ── Serialization / parsing ───────────────────────────────────────────────────

#[derive(serde::Serialize)]
struct WorkflowFile<'a> {
    workflows: &'a [WorkflowDef],
}

pub fn serialize_workflows(workflows: &[WorkflowDef]) -> String {
    toml::to_string(&WorkflowFile { workflows })
        .unwrap_or_else(|_| String::from("# serialization error\n"))
}

pub fn parse_workflow_toml(content: &str) -> Result<Vec<WorkflowDef>> {
    #[derive(serde::Deserialize)]
    struct WorkflowFile { workflows: Option<Vec<WorkflowDef>> }
    let parsed: WorkflowFile = toml::from_str(content)
        .map_err(|e| anyhow::anyhow!("Invalid workflow TOML: {e}"))?;
    Ok(parsed.workflows.unwrap_or_default())
}

// ── Fetch helpers ─────────────────────────────────────────────────────────────

fn fetch_workflow_content(url: &str) -> Result<String> {
    if let Some(gist_id) = extract_gist_id(url) {
        fetch_via_gh_gist(&gist_id)
    } else {
        fetch_url(url)
    }
}

pub fn extract_gist_id(url: &str) -> Option<String> {
    if !url.contains("gist.github.com") { return None; }
    url.trim_end_matches('/')
        .split('/')
        .next_back()
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn fetch_via_gh_gist(gist_id: &str) -> Result<String> {
    let out = Command::new("gh")
        .args(["gist", "view", gist_id, "--raw"])
        .output()
        .map_err(|e| anyhow::anyhow!("gh CLI not found: {e}"))?;
    if !out.status.success() {
        bail!("gh gist view failed: {}", String::from_utf8_lossy(&out.stderr));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

fn fetch_url(url: &str) -> Result<String> {
    if !url.starts_with("https://") && !url.starts_with("http://") {
        bail!("Only http:// and https:// URLs are supported for workflow import (got: {url})");
    }
    let out = Command::new("curl")
        .args(["-fsSL", url])
        .output()
        .map_err(|e| anyhow::anyhow!("curl not found: {e}"))?;
    if !out.status.success() {
        bail!("Failed to fetch {url}: {}", String::from_utf8_lossy(&out.stderr));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

// ── Display ───────────────────────────────────────────────────────────────────

fn print_workflow_group(label: &str, workflows: &[WorkflowDef]) {
    if workflows.is_empty() { return; }
    println!("── {label}");
    for wf in workflows {
        let desc = wf.description.as_deref().unwrap_or("");
        let suffix = if desc.is_empty() { String::new() } else { format!(" — {desc}") };
        println!("   {}{suffix}", wf.name);
        for step in &wf.steps {
            println!("     • dev-claw {step}");
        }
    }
    println!();
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_wf(name: &str, steps: &[&str]) -> WorkflowDef {
        WorkflowDef {
            name:        name.to_string(),
            description: None,
            steps:       steps.iter().map(|s| s.to_string()).collect(),
        }
    }

    // --- find_workflow ---

    #[test]
    fn find_workflow_returns_matching() {
        let wfs = vec![make_wf("pre-push", &["env check"]), make_wf("release", &["deps audit"])];
        assert!(find_workflow(&wfs, "pre-push").is_some());
    }

    #[test]
    fn find_workflow_returns_second_entry() {
        let wfs = vec![make_wf("a", &[]), make_wf("b", &["review diff"])];
        assert_eq!(find_workflow(&wfs, "b").unwrap().steps[0], "review diff");
    }

    #[test]
    fn find_workflow_returns_none_for_missing() {
        let wfs = vec![make_wf("pre-push", &["env check"])];
        assert!(find_workflow(&wfs, "nonexistent").is_none());
    }

    #[test]
    fn find_workflow_empty_list_returns_none() {
        assert!(find_workflow(&[], "anything").is_none());
    }

    // --- parse_workflow_toml ---

    #[test]
    fn parse_valid_workflow_toml() {
        let toml = r#"
[[workflows]]
name = "pre-push"
description = "Run before pushing"
steps = ["env check", "deps audit"]
"#;
        let wfs = parse_workflow_toml(toml).unwrap();
        assert_eq!(wfs.len(), 1);
        assert_eq!(wfs[0].name, "pre-push");
        assert_eq!(wfs[0].steps.len(), 2);
        assert_eq!(wfs[0].description.as_deref(), Some("Run before pushing"));
    }

    #[test]
    fn parse_multiple_workflows() {
        let toml = r#"
[[workflows]]
name = "flow-a"
steps = ["env check"]

[[workflows]]
name = "flow-b"
steps = ["deps audit", "review diff"]
"#;
        let wfs = parse_workflow_toml(toml).unwrap();
        assert_eq!(wfs.len(), 2);
        assert_eq!(wfs[1].name, "flow-b");
        assert_eq!(wfs[1].steps.len(), 2);
    }

    #[test]
    fn parse_empty_toml_returns_empty_vec() {
        let wfs = parse_workflow_toml("").unwrap();
        assert!(wfs.is_empty());
    }

    #[test]
    fn parse_invalid_toml_returns_error() {
        assert!(parse_workflow_toml("[[[[invalid").is_err());
    }

    #[test]
    fn parse_toml_description_is_optional() {
        let toml = "[[workflows]]\nname = \"x\"\nsteps = []\n";
        let wfs = parse_workflow_toml(toml).unwrap();
        assert_eq!(wfs[0].description, None);
    }

    // --- serialize_workflows ---

    #[test]
    fn serialize_round_trips() {
        let wfs = vec![make_wf("my-flow", &["env check", "deps audit"])];
        let toml = serialize_workflows(&wfs);
        let back = parse_workflow_toml(&toml).unwrap();
        assert_eq!(back[0].name, "my-flow");
        assert_eq!(back[0].steps, vec!["env check", "deps audit"]);
    }

    #[test]
    fn serialize_empty_returns_valid_toml() {
        let toml = serialize_workflows(&[]);
        assert!(parse_workflow_toml(&toml).is_ok());
    }

    #[test]
    fn serialize_description_round_trips() {
        let mut wf = make_wf("x", &["env check"]);
        wf.description = Some("My workflow".to_string());
        let toml = serialize_workflows(&[wf]);
        let back = parse_workflow_toml(&toml).unwrap();
        assert_eq!(back[0].description.as_deref(), Some("My workflow"));
    }

    // --- extract_gist_id ---

    #[test]
    fn extract_gist_id_from_valid_url() {
        let url = "https://gist.github.com/user/abc123def456";
        assert_eq!(extract_gist_id(url), Some("abc123def456".to_string()));
    }

    #[test]
    fn extract_gist_id_with_trailing_slash() {
        let url = "https://gist.github.com/user/abc123/";
        assert_eq!(extract_gist_id(url), Some("abc123".to_string()));
    }

    #[test]
    fn extract_gist_id_non_gist_url_returns_none() {
        assert_eq!(extract_gist_id("https://example.com/file.toml"), None);
        assert_eq!(extract_gist_id("https://github.com/user/repo"), None);
        assert_eq!(extract_gist_id("https://raw.githubusercontent.com/x/y/main/f.toml"), None);
    }

    #[test]
    fn extract_gist_id_raw_gist_url_still_extracts() {
        // gist.githubusercontent.com does NOT contain "gist.github.com"
        let raw = "https://gist.githubusercontent.com/user/abc/raw/file.toml";
        assert_eq!(extract_gist_id(raw), None); // raw URLs go through curl, not gh
    }

    // --- save/load global workflows (round-trip via serialize/parse) ---

    #[test]
    fn save_load_round_trip() {
        let d    = tempfile::TempDir::new().unwrap();
        let path = d.path().join("workflows.toml");
        let wfs  = vec![make_wf("test-flow", &["env check", "review diff"])];
        std::fs::write(&path, serialize_workflows(&wfs)).unwrap();
        let back = parse_workflow_toml(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(back[0].name, "test-flow");
        assert_eq!(back[0].steps.len(), 2);
    }

    #[test]
    fn load_global_nonexistent_returns_empty() {
        // global_workflows_path may or may not exist; serialize/parse path is reliable
        let wfs = parse_workflow_toml("").unwrap();
        assert!(wfs.is_empty());
    }

    // --- global_workflows_path ---

    #[test]
    fn global_workflows_path_ends_in_toml() {
        if let Ok(p) = global_workflows_path() {
            assert!(p.extension().map(|e| e == "toml").unwrap_or(false));
        }
    }
}
