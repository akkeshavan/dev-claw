use anyhow::{bail, Result};
use clap::Subcommand;
use std::process::Command;

use crate::{config::Config, creds, llm, memory, usage::UsageTracker};

// ── Constants ─────────────────────────────────────────────────────────────────

const MAX_DIFF_CHARS: usize = 12_000;
const VALID_FOCUS: &[&str] = &["all", "security", "performance", "style"];

// ── Subcommand enum ───────────────────────────────────────────────────────────

#[derive(Subcommand)]
pub enum ReviewAction {
    /// Review uncommitted or staged changes
    Diff {
        /// Review only staged (cached) changes; default reviews all uncommitted changes
        #[arg(long)]
        staged: bool,
        /// Review focus: all | security | performance | style
        #[arg(long, default_value = "all")]
        focus: String,
    },
    /// Review a GitHub pull request by number
    Pr {
        /// PR number to review
        number: u32,
        /// Review focus: all | security | performance | style
        #[arg(long, default_value = "all")]
        focus: String,
    },
    /// Natural language query — dev-claw review "<what you want to review>"
    #[command(external_subcommand)]
    Nl(Vec<String>),
}

// ── Public entry point ────────────────────────────────────────────────────────

pub async fn run(action: ReviewAction) -> Result<()> {
    match action {
        ReviewAction::Diff { staged, focus } => review_diff(staged, &focus).await,
        ReviewAction::Pr { number, focus } => review_pr(number, &focus).await,
        ReviewAction::Nl(_) => unreachable!("Nl handled in main"),
    }
}

// ── Diff subcommand ───────────────────────────────────────────────────────────

async fn review_diff(staged: bool, focus: &str) -> Result<()> {
    validate_focus(focus)?;
    let cfg = Config::load()?;
    enforce_quota(&cfg)?;
    let diff = diff_content(staged)?;
    let label = if staged { "staged" } else { "uncommitted" };
    println!("Reviewing {label} changes ({} chars)…\n", diff.len());
    let review = call_llm(&build_system_prompt(focus), &build_user_prompt(&diff)).await?;
    println!("{review}");
    Ok(())
}

// ── PR subcommand ─────────────────────────────────────────────────────────────

async fn review_pr(number: u32, focus: &str) -> Result<()> {
    validate_focus(focus)?;
    let cfg = Config::load()?;
    enforce_quota(&cfg)?;
    let diff = pr_diff(number)?;
    println!("Reviewing PR #{number} ({} chars)…\n", diff.len());
    let review = call_llm(&build_system_prompt(focus), &build_user_prompt(&diff)).await?;
    println!("{review}");
    Ok(())
}

// ── Git helpers ───────────────────────────────────────────────────────────────

fn diff_content(staged: bool) -> Result<String> {
    let args: &[&str] = if staged {
        &["diff", "--cached"]
    } else {
        &["diff", "HEAD"]
    };
    let output = Command::new("git")
        .args(args)
        .output()
        .map_err(|e| anyhow::anyhow!("git not found: {e}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if stdout.is_empty() {
        let label = if staged { "staged" } else { "uncommitted" };
        bail!("No {label} changes found to review");
    }
    Ok(truncate_diff(&stdout))
}

fn pr_diff(number: u32) -> Result<String> {
    let out = Command::new("gh")
        .args(["pr", "diff", &number.to_string()])
        .output()
        .map_err(|e| {
            anyhow::anyhow!("gh CLI not found: {e}. Install from https://cli.github.com")
        })?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        bail!("gh pr diff failed: {stderr}");
    }
    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if stdout.is_empty() {
        bail!("PR #{number} has no diff");
    }
    Ok(truncate_diff(&stdout))
}

// ── Prompt builders ───────────────────────────────────────────────────────────

pub fn build_system_prompt(focus: &str) -> String {
    let focus_instruction = focus_hint(focus);
    format!(
        "You are an expert code reviewer with deep knowledge of security, performance, and \
         software design. Review the provided git diff thoroughly.\n\n\
         Structure your review as:\n\
         **Summary** — 1-2 sentences describing the overall change.\n\
         **Findings** — grouped by severity:\n\
           🔴 Critical: bugs, security vulnerabilities, data loss risks\n\
           🟠 Major: logic errors, missing error handling, performance bottlenecks\n\
           🟡 Minor: code smells, unclear naming, missing tests\n\
           ⚪ Nitpick: style and formatting (only if impactful)\n\
         For each finding, cite the file and approximate line number if possible.\n\
         **Positives** — what was done well (be specific, not generic).\n\n\
         Focus: {focus_instruction}"
    )
}

fn focus_hint(focus: &str) -> &'static str {
    match focus {
        "security"    => "Prioritize security: injection flaws, auth bypasses, insecure data \
                          handling, secret exposure, and input validation. Use OWASP Top 10 as a guide.",
        "performance" => "Prioritize performance: algorithmic complexity, unnecessary allocations, \
                          N+1 queries, blocking I/O, and missing caching opportunities.",
        "style"       => "Prioritize readability and maintainability: naming clarity, function \
                          length, abstraction quality, and idiomatic patterns for the language.",
        _             => "Comprehensive review covering security, correctness, performance, \
                          and maintainability in that priority order.",
    }
}

pub fn build_user_prompt(diff: &str) -> String {
    format!("Git diff to review:\n\n```diff\n{diff}\n```")
}

// ── Diff truncation ───────────────────────────────────────────────────────────

pub fn truncate_diff(s: &str) -> String {
    if s.chars().count() <= MAX_DIFF_CHARS {
        return s.to_string();
    }
    let trimmed: String = s.chars().take(MAX_DIFF_CHARS).collect();
    format!("{trimmed}\n-- [diff truncated at {MAX_DIFF_CHARS} chars; review may be partial]")
}

// ── Focus validation ──────────────────────────────────────────────────────────

fn validate_focus(focus: &str) -> Result<()> {
    if VALID_FOCUS.contains(&focus) {
        return Ok(());
    }
    bail!(
        "Unknown focus `{focus}`. Valid options: {}",
        VALID_FOCUS.join(", ")
    )
}

// ── LLM helpers ───────────────────────────────────────────────────────────────

async fn call_llm(system: &str, user: &str) -> Result<String> {
    let ctx = memory::context_for_prompt("review");
    let provider = resolve_provider()?;
    let api_key = creds::load(&provider)?;
    let result = llm::client_for(&provider, &api_key)
        .complete(&format!("{ctx}{system}"), user)
        .await?;
    memory::record_interaction("review", &result);
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
    if let Some(w) = tracker.check_and_record("review", monthly, monthly, warn_at)? {
        eprintln!("{w}");
    }
    Ok(())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // --- focus validation ---

    #[test]
    fn validate_focus_accepts_all_valid() {
        for f in VALID_FOCUS {
            assert!(validate_focus(f).is_ok(), "{f} should be valid");
        }
    }

    #[test]
    fn validate_focus_rejects_unknown() {
        assert!(validate_focus("speed").is_err());
        assert!(validate_focus("bugs").is_err());
        assert!(validate_focus("").is_err());
    }

    // --- system prompt content ---

    #[test]
    fn system_prompt_all_mentions_comprehensive() {
        let p = build_system_prompt("all");
        assert!(p.contains("Comprehensive"));
    }

    #[test]
    fn system_prompt_security_mentions_owasp() {
        let p = build_system_prompt("security");
        assert!(p.contains("OWASP"));
    }

    #[test]
    fn system_prompt_performance_mentions_complexity() {
        let p = build_system_prompt("performance");
        assert!(p.contains("complexity"));
    }

    #[test]
    fn system_prompt_style_mentions_readability() {
        let p = build_system_prompt("style");
        assert!(p.contains("readability"));
    }

    #[test]
    fn system_prompt_always_includes_severity_labels() {
        for focus in VALID_FOCUS {
            let p = build_system_prompt(focus);
            assert!(p.contains("Critical"), "{focus}: missing Critical");
            assert!(p.contains("Major"), "{focus}: missing Major");
            assert!(p.contains("Minor"), "{focus}: missing Minor");
            assert!(p.contains("Positives"), "{focus}: missing Positives");
        }
    }

    #[test]
    fn system_prompt_always_includes_summary_section() {
        for focus in VALID_FOCUS {
            let p = build_system_prompt(focus);
            assert!(p.contains("Summary"), "{focus}: missing Summary section");
        }
    }

    // --- user prompt ---

    #[test]
    fn user_prompt_wraps_diff_in_fence() {
        let diff = "--- a/foo.rs\n+++ b/foo.rs\n@@ -1 +1 @@\n-old\n+new";
        let p = build_user_prompt(diff);
        assert!(p.contains("```diff"));
        assert!(p.contains(diff));
        assert!(p.contains("```"));
    }

    #[test]
    fn user_prompt_labels_diff() {
        let p = build_user_prompt("+ something");
        assert!(p.to_lowercase().contains("diff"));
    }

    // --- truncation ---

    #[test]
    fn truncate_diff_short_is_unchanged() {
        let s = "short diff";
        assert_eq!(truncate_diff(s), s);
    }

    #[test]
    fn truncate_diff_long_adds_marker() {
        let s = "x".repeat(15_000);
        let out = truncate_diff(&s);
        assert!(out.contains("truncated"));
        assert!(out.chars().count() < 15_000);
    }

    #[test]
    fn truncate_diff_exactly_at_limit_is_unchanged() {
        let s = "y".repeat(MAX_DIFF_CHARS);
        assert_eq!(truncate_diff(&s).chars().count(), MAX_DIFF_CHARS);
    }

    #[test]
    fn truncate_diff_one_over_limit_adds_marker() {
        let s = "z".repeat(MAX_DIFF_CHARS + 1);
        let out = truncate_diff(&s);
        assert!(out.contains("truncated"));
    }

    // --- focus hint completeness ---

    #[test]
    fn focus_hint_returns_non_empty_for_all_valid() {
        for f in VALID_FOCUS {
            let h = focus_hint(f);
            assert!(!h.is_empty(), "{f}: hint should not be empty");
        }
    }

    #[test]
    fn focus_hint_unknown_falls_back_to_comprehensive() {
        assert!(focus_hint("unknown").contains("Comprehensive"));
    }
}
