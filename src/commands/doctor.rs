use anyhow::{Context, Result};
use std::io::{self, IsTerminal, Read};

use crate::{
    config::{Config, DoctorConfig, UsageLimits},
    creds, llm, memory,
    usage::UsageTracker,
};

const SYSTEM_PROMPT: &str = r#"You are a surgical developer diagnostic tool embedded in a terminal.

The user has piped raw terminal output (stderr, build logs, or a crash dump) to you.

Your job — two steps only:
1. ROOT CAUSE: Identify the actual cause in 1–2 lines. Include file name and line number if visible.
2. FIX: Provide a concrete, copy-pasteable fix — a shell command OR a minimal code diff. Not both.

Output format (use exactly this structure, no preamble, no trailing notes):

Root cause: <concise description, 1–2 lines>
Fix: <shell command OR diff>

Rules:
- Missing dependency → exact install command (npm install X, cargo add X, pip install X).
- Type / syntax error → show only the corrected line(s) in diff format.
- Config error → show the corrected key = value.
- Ambiguous error → pick the most likely cause; note it is an assumption.
- Hard limit: 8 lines of output total.
- No greetings. No "I hope this helps." Just the diagnosis."#;

pub async fn run() -> Result<()> {
    let raw     = read_stdin()?;
    let cfg     = Config::load()?;
    let cleaned = apply_doctor_config(&raw, cfg.doctor.as_ref());
    let warning = enforce_quota(cfg.usage.as_ref())?;
    let answer  = call_llm(&cleaned).await?;

    println!("\n{}\n", answer.trim());
    if let Some(w) = warning {
        eprintln!("⚠  {}", w);
    }
    Ok(())
}

const MAX_STDIN_BYTES: u64 = 1_048_576; // 1 MiB

fn read_stdin() -> Result<String> {
    if io::stdin().is_terminal() {
        anyhow::bail!(
            "Expecting piped input, got a terminal.\n\
             Usage:  npm run dev 2>&1 | dev-claw doctor\n\
                     cargo build 2>&1 | dev-claw doctor"
        );
    }
    let mut buf = String::new();
    io::stdin()
        .take(MAX_STDIN_BYTES)
        .read_to_string(&mut buf)
        .context("Failed to read from stdin")?;
    if buf.trim().is_empty() {
        anyhow::bail!("Received empty input — nothing to diagnose.");
    }
    Ok(buf)
}

fn apply_doctor_config(raw: &str, cfg: Option<&DoctorConfig>) -> String {
    preprocess(
        raw,
        cfg.and_then(|d| d.max_log_lines).unwrap_or(100),
        cfg.and_then(|d| d.auto_ignore_patterns.as_deref())
            .unwrap_or(&[]),
    )
}

fn enforce_quota(cfg: Option<&UsageLimits>) -> Result<Option<String>> {
    let total_limit = cfg.map(|u| u.monthly_limit()).unwrap_or(200);
    let cmd_limit   = cfg.map(|u| u.doctor_limit()).unwrap_or(75);
    let warn_pct    = cfg.map(|u| u.warn_at_percent()).unwrap_or(80);
    UsageTracker::open()?.check_and_record("doctor", cmd_limit, total_limit, warn_pct)
}

async fn call_llm(cleaned: &str) -> Result<String> {
    let ctx      = memory::context_for_prompt("doctor");
    let system   = format!("{ctx}{SYSTEM_PROMPT}");
    let provider = resolve_provider()?;
    let api_key  = creds::load(&provider)?;
    let result   = llm::client_for(&provider, &api_key).complete(&system, cleaned).await?;
    memory::record_interaction("doctor", &result);
    Ok(result)
}

fn resolve_provider() -> Result<String> {
    std::env::var("DEV_CLAW_PROVIDER")
        .ok()
        .or_else(creds::auto_detect_provider)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "No API key configured.\n\
                 Run:  dev-claw config set-key --provider deepseek\n\
                 Or:   DEV_CLAW_PROVIDER=ollama dev-claw doctor"
            )
        })
}

/// Strips ANSI codes, removes noisy lines, keeps the last `max_lines` lines.
/// Errors surface at the tail of build output, so we drop the head.
fn preprocess(raw: &str, max_lines: usize, ignore: &[String]) -> String {
    let stripped = strip_ansi_escapes::strip(raw.as_bytes());
    let text     = String::from_utf8_lossy(&stripped);

    let lines: Vec<&str> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter(|l| !ignore.iter().any(|pat| l.contains(pat.as_str())))
        .collect();

    let start = lines.len().saturating_sub(max_lines);
    lines[start..].join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_ansi_escape_codes() {
        let input = "\x1b[31mError\x1b[0m: something failed";
        let out   = preprocess(input, 100, &[]);
        assert!(!out.contains('\x1b'));
        assert!(out.contains("Error: something failed"));
    }

    #[test]
    fn filters_ignore_patterns() {
        let input    = "node_modules/foo/bar.js: noise\nactual error here";
        let patterns = vec!["node_modules".to_string()];
        let out      = preprocess(input, 100, &patterns);
        assert!(!out.contains("node_modules"));
        assert!(out.contains("actual error here"));
    }

    #[test]
    fn takes_tail_not_head() {
        let input: String = (1..=200).map(|i| format!("line {i}\n")).collect();
        let out   = preprocess(&input, 10, &[]);
        assert!(out.contains("line 200"));
        assert!(!out.contains("line 1\n"));
        assert_eq!(out.lines().count(), 10);
    }

    #[test]
    fn removes_blank_lines() {
        let input = "error A\n\n\nerror B\n";
        let out   = preprocess(input, 100, &[]);
        assert!(!out.contains("\n\n"));
        assert_eq!(out.lines().count(), 2);
    }

    #[test]
    fn multiple_patterns_all_filtered() {
        let input = "node_modules/x.js:1: bad\n.next/chunk.js:2: bad\nreal error";
        let pats  = vec!["node_modules".to_string(), ".next".to_string()];
        let out   = preprocess(input, 100, &pats);
        assert_eq!(out.lines().count(), 1);
        assert!(out.contains("real error"));
    }

    #[test]
    fn empty_input_produces_empty_output() {
        let out = preprocess("", 100, &[]);
        assert!(out.is_empty());
    }

    #[test]
    fn max_lines_zero_gives_empty_output() {
        let out = preprocess("line 1\nline 2", 0, &[]);
        assert!(out.is_empty());
    }
}
