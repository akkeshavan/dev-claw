use anyhow::{bail, Result};
use clap::Subcommand;
use std::process::Command;

use crate::{config::Config, creds, llm, memory, usage::UsageTracker, utils};

// ── Subcommand enum ───────────────────────────────────────────────────────────

#[derive(Subcommand)]
pub enum ReleaseAction {
    /// Draft polished release notes from commits since the last tag
    Notes {
        /// Compare from this git ref instead of the last tag
        #[arg(long)]
        since: Option<String>,
    },
    /// Suggest a semver bump, draft notes, and optionally create a git tag
    Cut {
        /// Version to release, e.g. v1.2.3 (auto-suggested if omitted)
        #[arg(value_name = "VERSION")]
        version: Option<String>,
        /// Print what would happen without writing any files or tags
        #[arg(long)]
        dry_run: bool,
    },
}

// ── Public entry point ────────────────────────────────────────────────────────

pub async fn run(action: ReleaseAction) -> Result<()> {
    match action {
        ReleaseAction::Notes { since } => notes(since.as_deref()).await,
        ReleaseAction::Cut { version, dry_run } => cut(version.as_deref(), dry_run).await,
    }
}

// ── Notes subcommand ──────────────────────────────────────────────────────────

async fn notes(since: Option<&str>) -> Result<()> {
    let cfg = Config::load()?;
    enforce_quota(&cfg)?;
    let base = since.map(str::to_string).unwrap_or_else(last_tag);
    let commits = commits_since(&base)?;
    if commits.is_empty() {
        bail!("No commits found since {base}");
    }
    println!(
        "Drafting release notes from {} commit(s) since {base}…\n",
        commits.len()
    );
    let notes = draft_notes(&commits, &base).await?;
    println!("{notes}");
    Ok(())
}

// ── Cut subcommand ────────────────────────────────────────────────────────────

async fn cut(version_override: Option<&str>, dry_run: bool) -> Result<()> {
    let cfg = Config::load()?;
    enforce_quota(&cfg)?;
    let base = last_tag();
    let commits = commits_since(&base)?;
    if commits.is_empty() {
        bail!("No commits found since {base} — nothing to release");
    }
    let version = match version_override {
        Some(v) => v.trim_start_matches('v').to_string(),
        None => suggest_version(&base, &commits),
    };
    println!(
        "Drafting release notes for v{version} ({} commit(s) since {base})…\n",
        commits.len()
    );
    let notes = draft_notes(&commits, &base).await?;
    println!("{notes}\n");
    if dry_run {
        println!("-- dry-run: would update CHANGELOG.md and create tag v{version}");
        return Ok(());
    }
    if !utils::confirm(&format!(
        "Write to CHANGELOG.md and create tag v{version}? [y/N] "
    ))? {
        println!("Aborted.");
        return Ok(());
    }
    prepend_changelog(&version, &notes)?;
    println!("Updated CHANGELOG.md");
    create_tag(&version, &notes)?;
    println!("Created tag v{version}");
    Ok(())
}

// ── Git helpers ───────────────────────────────────────────────────────────────

pub fn last_tag() -> String {
    let out = Command::new("git")
        .args(["describe", "--tags", "--abbrev=0"])
        .output();
    match out {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
        _ => first_commit(),
    }
}

fn first_commit() -> String {
    let out = Command::new("git")
        .args(["rev-list", "--max-parents=0", "HEAD"])
        .output();
    match out {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
        _ => String::from("HEAD~10"),
    }
}

pub fn commits_since(base: &str) -> Result<Vec<String>> {
    let range = format!("{base}..HEAD");
    let output = Command::new("git")
        .args(["log", &range, "--pretty=format:%s", "--no-merges"])
        .output()
        .map_err(|e| anyhow::anyhow!("git log failed: {e}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<String> = stdout
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect();
    Ok(lines)
}

pub fn suggest_version(last_tag: &str, commits: &[String]) -> String {
    let (major, minor, patch) = parse_semver(last_tag);
    let has_breaking = commits
        .iter()
        .any(|c| c.contains("BREAKING CHANGE") || c.starts_with("!") || c.contains("!:"));
    let has_feat = commits.iter().any(|c| c.starts_with("feat"));
    if has_breaking {
        return format!("{}.0.0", major + 1);
    }
    if has_feat {
        return format!("{}.{}.0", major, minor + 1);
    }
    format!("{major}.{minor}.{}", patch + 1)
}

fn parse_semver(tag: &str) -> (u32, u32, u32) {
    let v = tag.trim_start_matches('v');
    let parts: Vec<&str> = v.split('.').collect();
    let n = |i: usize| parts.get(i).and_then(|s| s.parse().ok()).unwrap_or(0);
    (n(0), n(1), n(2))
}

// ── Changelog ─────────────────────────────────────────────────────────────────

pub fn prepend_changelog(version: &str, notes: &str) -> Result<()> {
    prepend_changelog_at(std::path::Path::new("CHANGELOG.md"), version, notes)
}

pub fn prepend_changelog_at(path: &std::path::Path, version: &str, notes: &str) -> Result<()> {
    let date = chrono::Local::now().format("%Y-%m-%d");
    let header = format!("## v{version} ({date})\n\n{notes}\n\n");
    let existing = std::fs::read_to_string(path).unwrap_or_default();
    std::fs::write(path, format!("{header}{existing}"))
        .map_err(|e| anyhow::anyhow!("Cannot write {}: {e}", path.display()))
}

fn create_tag(version: &str, notes: &str) -> Result<()> {
    let first_line = notes.lines().next().unwrap_or("Release").trim().to_string();
    let tag = format!("v{version}");
    let status = Command::new("git")
        .args(["tag", "-a", &tag, "-m", &first_line])
        .status()
        .map_err(|e| anyhow::anyhow!("git tag failed: {e}"))?;
    if !status.success() {
        bail!("git tag returned non-zero exit code");
    }
    Ok(())
}

// ── LLM draft ─────────────────────────────────────────────────────────────────

async fn draft_notes(commits: &[String], base: &str) -> Result<String> {
    let ctx = memory::context_for_prompt("release");
    let provider = resolve_provider()?;
    let api_key = creds::load(&provider)?;
    let client = llm::client_for(&provider, &api_key);
    let base_system = "You are a technical writer drafting release notes for a software project. \
        Given a list of commit messages (Conventional Commits format), produce polished, \
        user-facing release notes grouped by category (Features, Bug Fixes, Improvements, \
        Breaking Changes). Write for end-users, not developers. Be concise and clear. \
        Omit chore/ci/docs commits unless significant.";
    let system = format!("{ctx}{base_system}");
    let prompt = format!(
        "Commits since {base}:\n{}\n\nWrite release notes.",
        commits.join("\n")
    );
    let result = client.complete(&system, &prompt).await?;
    memory::record_interaction("release", &result);
    Ok(result)
}

fn resolve_provider() -> Result<String> {
    if let Ok(p) = std::env::var("DEV_CLAW_PROVIDER") {
        return Ok(p);
    }
    creds::auto_detect_provider().ok_or_else(|| {
        anyhow::anyhow!(
            "No API provider configured. Run: dev-claw config set-key --provider deepseek"
        )
    })
}

// ── Quota enforcement ─────────────────────────────────────────────────────────

fn enforce_quota(cfg: &Config) -> Result<()> {
    let tracker = UsageTracker::open()?;
    let limits = cfg.usage.as_ref();
    let monthly = limits.map(|l| l.monthly_limit()).unwrap_or(200);
    let warn_at = limits.map(|l| l.warn_at_percent()).unwrap_or(80);
    // release shares the monthly pool; no per-command limit
    if let Some(w) = tracker.check_and_record("release", monthly, monthly, warn_at)? {
        eprintln!("{w}");
    }
    Ok(())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // --- semver parsing ---

    #[test]
    fn parse_semver_v_prefix() {
        assert_eq!(parse_semver("v1.2.3"), (1, 2, 3));
    }

    #[test]
    fn parse_semver_no_prefix() {
        assert_eq!(parse_semver("2.10.0"), (2, 10, 0));
    }

    #[test]
    fn parse_semver_partial_tag() {
        assert_eq!(parse_semver("v1.0"), (1, 0, 0));
    }

    #[test]
    fn parse_semver_non_semver_tag() {
        assert_eq!(parse_semver("release-abc"), (0, 0, 0));
    }

    // --- version suggestion ---

    #[test]
    fn suggest_patch_for_fixes_only() {
        let commits = vec![
            "fix: null pointer in auth".to_string(),
            "fix: typo".to_string(),
        ];
        let v = suggest_version("v1.2.3", &commits);
        assert_eq!(v, "1.2.4");
    }

    #[test]
    fn suggest_minor_for_feat() {
        let commits = vec!["feat: add dark mode".to_string(), "fix: typo".to_string()];
        let v = suggest_version("v1.2.3", &commits);
        assert_eq!(v, "1.3.0");
    }

    #[test]
    fn suggest_major_for_breaking_change_annotation() {
        let commits = vec!["feat!: remove legacy API".to_string()];
        let v = suggest_version("v1.2.3", &commits);
        assert_eq!(v, "2.0.0");
    }

    #[test]
    fn suggest_major_for_breaking_change_footer() {
        let commits = vec!["feat: new auth\n\nBREAKING CHANGE: token format changed".to_string()];
        let v = suggest_version("v1.2.3", &commits);
        assert_eq!(v, "2.0.0");
    }

    #[test]
    fn suggest_patch_from_v0_tag() {
        let commits = vec!["chore: bump deps".to_string()];
        let v = suggest_version("v0.1.0", &commits);
        assert_eq!(v, "0.1.1");
    }

    #[test]
    fn suggest_version_handles_non_semver_base() {
        let commits = vec!["fix: something".to_string()];
        let v = suggest_version("not-a-version", &commits);
        // 0.0.0 → patch → 0.0.1
        assert_eq!(v, "0.0.1");
    }

    // --- changelog prepend ---

    #[test]
    fn prepend_changelog_creates_file() {
        let d = tempfile::TempDir::new().unwrap();
        let path = d.path().join("CHANGELOG.md");
        prepend_changelog_at(&path, "1.0.0", "Initial release.").unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("v1.0.0"));
        assert!(content.contains("Initial release."));
    }

    #[test]
    fn prepend_changelog_prepends_to_existing() {
        let d = tempfile::TempDir::new().unwrap();
        let path = d.path().join("CHANGELOG.md");
        std::fs::write(&path, "## v0.9.0\n\nOld notes.\n").unwrap();
        prepend_changelog_at(&path, "1.0.0", "New stuff.").unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        let idx_new = content.find("v1.0.0").unwrap();
        let idx_old = content.find("v0.9.0").unwrap();
        assert!(
            idx_new < idx_old,
            "new version should appear before old version"
        );
    }

    // --- commits_since (git required) ---

    #[test]
    fn commits_since_empty_range_returns_empty() {
        // HEAD..HEAD is always empty
        let result = commits_since("HEAD");
        match result {
            Ok(v) => assert!(v.is_empty()),
            Err(_) => {} // no git repo in test env is fine
        }
    }
}
