use anyhow::{bail, Result};
use std::process::Command;

use crate::{config::Config, creds, llm, usage::UsageTracker};

// ── Public entry point ────────────────────────────────────────────────────────

pub async fn run(since: &str, format: &str) -> Result<()> {
    validate_format(format)?;
    let cfg    = Config::load()?;
    enforce_quota(&cfg)?;
    let author  = git_author()?;
    let commits = recent_commits(since, &author)?;
    let branches = active_branches();
    if commits.is_empty() && branches.is_empty() {
        bail!("No commits or branches found since {since} for author {author}");
    }
    println!("Generating standup for {author} (since {since})…\n");
    let draft = draft_standup(&commits, &branches, format).await?;
    println!("{draft}");
    Ok(())
}

// ── Git helpers ───────────────────────────────────────────────────────────────

pub fn git_author() -> Result<String> {
    let out = Command::new("git")
        .args(["config", "user.email"])
        .output()
        .map_err(|e| anyhow::anyhow!("git not found: {e}"))?;
    if !out.status.success() || out.stdout.is_empty() {
        bail!("git config user.email is not set — configure git before running standup");
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

pub fn recent_commits(since: &str, author: &str) -> Result<Vec<String>> {
    let output = Command::new("git")
        .args([
            "log",
            &format!("--since={since}"),
            &format!("--author={author}"),
            "--pretty=format:%s",
            "--no-merges",
            "--all",
        ])
        .output()
        .map_err(|e| anyhow::anyhow!("git log failed: {e}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(stdout
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect())
}

pub fn active_branches() -> Vec<String> {
    let out = Command::new("git")
        .args(["branch", "--sort=-committerdate", "--format=%(refname:short)"])
        .output();
    let Ok(out) = out else { return vec![]; };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && *l != "main" && *l != "master")
        .take(3)
        .map(str::to_string)
        .collect()
}

// ── LLM draft ─────────────────────────────────────────────────────────────────

async fn draft_standup(commits: &[String], branches: &[String], format: &str) -> Result<String> {
    let provider = resolve_provider()?;
    let api_key  = creds::load(&provider)?;
    let client   = llm::client_for(&provider, &api_key);
    let system   = build_system_prompt(format);
    let prompt   = build_prompt(commits, branches);
    client.complete(&system, &prompt).await
}

pub fn build_system_prompt(format: &str) -> String {
    let fmt_hint = match format {
        "slack"    => "Format the standup with emoji bullets (✅ Done, 🔧 In Progress, 🚧 Blockers). \
                       Keep each point to one line. Use *bold* for section headers.",
        "markdown" => "Format with ## Done, ## In Progress, ## Blockers sections. \
                       Use bullet lists under each section.",
        _          => "Format as plain text with Done / In Progress / Blockers sections. \
                       No markdown or special formatting.",
    };
    format!(
        "You are a developer writing a daily standup update. \
         Given a list of recent git commits and active branches, synthesize a concise, \
         professional standup message with three sections: Done, In Progress, Blockers. \
         Infer 'In Progress' from branch names. List 'No blockers' if none are apparent. \
         Avoid jargon and keep it under 150 words.\n{fmt_hint}"
    )
}

pub fn build_prompt(commits: &[String], branches: &[String]) -> String {
    let commit_list  = if commits.is_empty() {
        "(no commits in range)".to_string()
    } else {
        commits.join("\n")
    };
    let branch_list = if branches.is_empty() {
        "(none)".to_string()
    } else {
        branches.join(", ")
    };
    format!(
        "Recent commits:\n{commit_list}\n\nActive branches: {branch_list}\n\nWrite my standup."
    )
}

// ── Format validation ─────────────────────────────────────────────────────────

const VALID_FORMATS: &[&str] = &["plain", "slack", "markdown"];

fn validate_format(fmt: &str) -> Result<()> {
    if VALID_FORMATS.contains(&fmt) { return Ok(()); }
    bail!("Unknown format `{fmt}`. Valid options: {}", VALID_FORMATS.join(", "))
}

// ── Quota enforcement ─────────────────────────────────────────────────────────

fn enforce_quota(cfg: &Config) -> Result<()> {
    let tracker = UsageTracker::open()?;
    let limits  = cfg.usage.as_ref();
    let monthly = limits.map(|l| l.monthly_limit()).unwrap_or(200);
    let warn_at = limits.map(|l| l.warn_at_percent()).unwrap_or(80);
    if let Some(w) = tracker.check_and_record("standup", monthly, monthly, warn_at)? {
        eprintln!("{w}");
    }
    Ok(())
}

fn resolve_provider() -> Result<String> {
    if let Ok(p) = std::env::var("DEV_CLAW_PROVIDER") { return Ok(p); }
    creds::auto_detect_provider().ok_or_else(|| anyhow::anyhow!(
        "No API provider configured. Run: dev-claw config set-key --provider deepseek"
    ))
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // --- format validation ---

    #[test]
    fn validate_format_accepts_all_valid() {
        for fmt in VALID_FORMATS {
            assert!(validate_format(fmt).is_ok(), "{fmt} should be valid");
        }
    }

    #[test]
    fn validate_format_rejects_unknown() {
        assert!(validate_format("json").is_err());
        assert!(validate_format("html").is_err());
        assert!(validate_format("").is_err());
    }

    // --- system prompt ---

    #[test]
    fn system_prompt_plain_mentions_plain_text() {
        let p = build_system_prompt("plain");
        assert!(p.contains("plain text"));
    }

    #[test]
    fn system_prompt_slack_mentions_emoji() {
        let p = build_system_prompt("slack");
        assert!(p.contains("emoji"));
    }

    #[test]
    fn system_prompt_markdown_mentions_sections() {
        let p = build_system_prompt("markdown");
        assert!(p.contains("##"));
    }

    #[test]
    fn system_prompt_includes_standup_structure() {
        for fmt in VALID_FORMATS {
            let p = build_system_prompt(fmt);
            assert!(p.contains("Done"), "{fmt}: missing Done");
            assert!(p.contains("In Progress"), "{fmt}: missing In Progress");
            assert!(p.contains("Blockers"), "{fmt}: missing Blockers");
        }
    }

    // --- prompt building ---

    #[test]
    fn build_prompt_includes_commits() {
        let commits  = vec!["feat: new login".to_string(), "fix: crash on start".to_string()];
        let branches = vec!["feature/auth".to_string()];
        let p = build_prompt(&commits, &branches);
        assert!(p.contains("feat: new login"));
        assert!(p.contains("fix: crash on start"));
        assert!(p.contains("feature/auth"));
    }

    #[test]
    fn build_prompt_empty_commits_shows_placeholder() {
        let p = build_prompt(&[], &[]);
        assert!(p.contains("no commits in range"));
    }

    #[test]
    fn build_prompt_empty_branches_shows_none() {
        let p = build_prompt(&["fix: something".to_string()], &[]);
        assert!(p.contains("(none)"));
    }

    // --- git_author (live git call) ---

    #[test]
    fn git_author_returns_string_or_error() {
        // Just verify it doesn't panic; in CI without git config it may return Err
        let _ = git_author();
    }

    // --- recent_commits ---

    #[test]
    fn recent_commits_empty_on_future_since() {
        // asking for commits in the future should return empty
        let result = recent_commits("2099-01-01", "nobody@example.com");
        match result {
            Ok(v)  => assert!(v.is_empty()),
            Err(_) => {} // no git repo is also acceptable
        }
    }

    // --- active_branches ---

    #[test]
    fn active_branches_returns_at_most_three() {
        let branches = active_branches();
        assert!(branches.len() <= 3);
    }

    #[test]
    fn active_branches_excludes_main_master() {
        let branches = active_branches();
        assert!(!branches.contains(&"main".to_string()));
        assert!(!branches.contains(&"master".to_string()));
    }
}
