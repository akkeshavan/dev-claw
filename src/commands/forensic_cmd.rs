use anyhow::Result;
use clap::Subcommand;
use std::collections::HashSet;

use crate::{config::Config, creds, llm, memory, usage::UsageTracker};

// ── Subcommand enum ───────────────────────────────────────────────────────────

#[derive(Subcommand)]
pub enum ForensicAction {
    /// Explain WHY code exists using git blame and LLM analysis
    Explain {
        /// File to analyse
        file: String,
        /// Line range, e.g. 10-25
        #[arg(long, value_name = "START-END")]
        lines: Option<String>,
    },
    /// Pretty-print git blame output (no LLM)
    Blame {
        /// File to blame
        file: String,
        /// Line range, e.g. 10-25
        #[arg(long, value_name = "START-END")]
        lines: Option<String>,
    },
    /// Natural language query — dev-claw forensic "<what you want to know>"
    #[command(external_subcommand)]
    Nl(Vec<String>),
}

// ── Blame line value object ───────────────────────────────────────────────────

struct BlameLine {
    hash: String,
    author: String,
    summary: String,
    content: String,
    lineno: u32,
}

// ── Public entry point ────────────────────────────────────────────────────────

pub async fn run(action: ForensicAction) -> Result<()> {
    match action {
        ForensicAction::Explain { file, lines } => explain(&file, lines.as_deref()).await,
        ForensicAction::Blame { file, lines } => blame_cmd(&file, lines.as_deref()),
        ForensicAction::Nl(_) => unreachable!("Nl handled in main"),
    }
}

// ── Explain subcommand ────────────────────────────────────────────────────────

async fn explain(file: &str, lines: Option<&str>) -> Result<()> {
    let cfg = Config::load()?;
    enforce_quota(&cfg)?;
    let range = lines.map(parse_line_range).transpose()?;
    let raw = run_git_blame(file, range)?;
    let blame = parse_blame_output(&raw);
    if blame.is_empty() {
        anyhow::bail!("`{file}` has no blame output — is it tracked by git?");
    }
    let suffix = range
        .map(|(s, e)| format!(" lines {s}–{e}"))
        .unwrap_or_default();
    println!("Analysing {file}{suffix}...\n");
    let prompt = build_llm_prompt(file, &blame, range);
    let analysis = call_llm(&prompt).await?;
    println!("{analysis}");
    Ok(())
}

// ── Blame subcommand ──────────────────────────────────────────────────────────

fn blame_cmd(file: &str, lines: Option<&str>) -> Result<()> {
    let range = lines.map(parse_line_range).transpose()?;
    let raw = run_git_blame(file, range)?;
    let blame = parse_blame_output(&raw);
    if blame.is_empty() {
        anyhow::bail!("`{file}` has no blame output — is it tracked by git?");
    }
    print_blame_table(&blame);
    Ok(())
}

// ── git blame runner ──────────────────────────────────────────────────────────

fn run_git_blame(file: &str, range: Option<(u32, u32)>) -> Result<String> {
    let mut cmd = std::process::Command::new("git");
    cmd.args(["blame", "--line-porcelain"]);
    if let Some((start, end)) = range {
        cmd.args(["-L", &format!("{start},{end}")]);
    }
    cmd.args(["--", file]);
    let out = cmd.output()?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        anyhow::bail!("git blame failed: {err}");
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

// ── Blame output parser ───────────────────────────────────────────────────────

fn parse_blame_output(raw: &str) -> Vec<BlameLine> {
    let mut result = Vec::new();
    let mut block: Vec<&str> = Vec::new();
    for line in raw.lines() {
        block.push(line);
        if line.starts_with('\t') {
            if let Some(b) = parse_blame_block(&block) {
                result.push(b);
            }
            block.clear();
        }
    }
    result
}

fn parse_blame_block(block: &[&str]) -> Option<BlameLine> {
    let first = block.first()?;
    let parts: Vec<&str> = first.split(' ').collect();
    let hash = parts.first()?.chars().take(8).collect();
    let lineno: u32 = parts.get(2)?.parse().ok()?;
    let author = find_field(block, "author ").unwrap_or("unknown");
    let summary = find_field(block, "summary ").unwrap_or("(no message)");
    let content = block.last()?.strip_prefix('\t').unwrap_or("").to_string();
    Some(BlameLine {
        hash,
        author: author.into(),
        summary: summary.into(),
        content,
        lineno,
    })
}

fn find_field<'a>(block: &[&'a str], prefix: &str) -> Option<&'a str> {
    block
        .iter()
        .find(|l| l.starts_with(prefix))
        .map(|l| &l[prefix.len()..])
}

// ── Display helpers ───────────────────────────────────────────────────────────

fn print_blame_table(lines: &[BlameLine]) {
    for l in lines {
        let summary = trunc_display(&l.summary, 38);
        let author = trunc_display(&l.author, 20);
        println!(
            "{} ({author}, {summary}) {:>5}: {}",
            l.hash, l.lineno, l.content
        );
    }
}

fn trunc_display(s: &str, max: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max {
        return s.to_string();
    }
    format!("{}…", chars[..max - 1].iter().collect::<String>())
}

// ── LLM helpers ───────────────────────────────────────────────────────────────

fn build_llm_prompt(file: &str, lines: &[BlameLine], range: Option<(u32, u32)>) -> String {
    let range_str = match range {
        Some((s, e)) => format!("lines {s}–{e}"),
        None => "all lines".to_string(),
    };
    let code = lines
        .iter()
        .map(|l| format!("{:>4} {}", l.lineno, l.content))
        .collect::<Vec<_>>()
        .join("\n");
    let commits = unique_summaries(lines)
        .into_iter()
        .map(|(h, a, s)| format!("{h} ({a}): {s}"))
        .collect::<Vec<_>>()
        .join("\n");
    let code = truncate_llm(&code, 6_000);
    format!("File: `{file}` ({range_str})\n\nCode:\n```\n{code}\n```\n\nCommits that introduced these lines:\n{commits}")
}

fn unique_summaries(lines: &[BlameLine]) -> Vec<(String, String, String)> {
    let mut seen = HashSet::new();
    lines
        .iter()
        .filter(|l| seen.insert(l.hash.clone()))
        .map(|l| (l.hash.clone(), l.author.clone(), l.summary.clone()))
        .collect()
}

fn truncate_llm(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let trimmed: String = s.chars().take(max_chars).collect();
    format!("{trimmed}\n[... truncated]")
}

async fn call_llm(prompt: &str) -> Result<String> {
    let ctx = memory::context_for_prompt("forensic");
    let provider = resolve_provider()?;
    let api_key = creds::load(&provider)?;
    let client = llm::client_for(&provider, &api_key);
    let base_system = "You are a code historian. Given source lines and git blame metadata, \
                    explain WHY this code exists — what problem it solved, what bug it fixed, \
                    or what feature it implements. Be concise and specific.";
    let system = format!("{ctx}{base_system}");
    let result = client.complete(&system, prompt).await?;
    memory::record_interaction("forensic", &result);
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
    let cmd_limit = limits.map(|l| l.forensic_limit()).unwrap_or(50);
    let warn_at = limits.map(|l| l.warn_at_percent()).unwrap_or(80);
    if let Some(w) = tracker.check_and_record("forensic", cmd_limit, monthly, warn_at)? {
        eprintln!("{w}");
    }
    Ok(())
}

// ── Line range parsing ────────────────────────────────────────────────────────

fn parse_line_range(s: &str) -> Result<(u32, u32)> {
    let (a, b) = s
        .split_once('-')
        .ok_or_else(|| anyhow::anyhow!("Expected range like 10-25, got: {s}"))?;
    let start: u32 = a
        .trim()
        .parse()
        .map_err(|_| anyhow::anyhow!("Invalid start line: {a}"))?;
    let end: u32 = b
        .trim()
        .parse()
        .map_err(|_| anyhow::anyhow!("Invalid end line: {b}"))?;
    if start > end {
        anyhow::bail!("Start {start} must be ≤ end {end}");
    }
    Ok((start, end))
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_PORCELAIN: &str = "\
abc12345678901234567890123456789012345678 1 3 2\n\
author Alice\n\
author-mail <alice@example.com>\n\
author-time 1700000000\n\
author-tz +0000\n\
committer Alice\n\
committer-mail <alice@example.com>\n\
committer-time 1700000000\n\
committer-tz +0000\n\
summary Fix the null pointer in payment flow\n\
filename src/payment.rs\n\
\tlet result = process_payment(ctx);\n\
def12345678901234567890123456789012345678 2 4 1\n\
author Bob\n\
author-mail <bob@example.com>\n\
author-time 1700000001\n\
author-tz +0000\n\
committer Bob\n\
committer-mail <bob@example.com>\n\
committer-time 1700000001\n\
committer-tz +0000\n\
summary Add retry logic for transient errors\n\
filename src/payment.rs\n\
\tif result.is_err() { retry(ctx); }\n";

    #[test]
    fn parse_blame_output_two_blocks() {
        let lines = parse_blame_output(SAMPLE_PORCELAIN);
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn parse_blame_block_extracts_hash() {
        let lines = parse_blame_output(SAMPLE_PORCELAIN);
        assert_eq!(lines[0].hash, "abc12345");
    }

    #[test]
    fn parse_blame_block_extracts_author() {
        let lines = parse_blame_output(SAMPLE_PORCELAIN);
        assert_eq!(lines[0].author, "Alice");
        assert_eq!(lines[1].author, "Bob");
    }

    #[test]
    fn parse_blame_block_extracts_summary() {
        let lines = parse_blame_output(SAMPLE_PORCELAIN);
        assert_eq!(lines[0].summary, "Fix the null pointer in payment flow");
    }

    #[test]
    fn parse_blame_block_extracts_content() {
        let lines = parse_blame_output(SAMPLE_PORCELAIN);
        assert_eq!(lines[0].content, "let result = process_payment(ctx);");
    }

    #[test]
    fn parse_blame_block_extracts_lineno() {
        let lines = parse_blame_output(SAMPLE_PORCELAIN);
        assert_eq!(lines[0].lineno, 3);
        assert_eq!(lines[1].lineno, 4);
    }

    #[test]
    fn unique_summaries_deduplicates_by_hash() {
        // Same hash repeated (like two lines from same commit)
        let lines = parse_blame_output(SAMPLE_PORCELAIN);
        // Build a second set with a duplicate of lines[0]
        let with_dup = vec![
            BlameLine {
                hash: "abc12345".into(),
                author: "Alice".into(),
                summary: "Fix the null ptr".into(),
                content: "x".into(),
                lineno: 1,
            },
            BlameLine {
                hash: "abc12345".into(),
                author: "Alice".into(),
                summary: "Fix the null ptr".into(),
                content: "y".into(),
                lineno: 2,
            },
            BlameLine {
                hash: "def12345".into(),
                author: "Bob".into(),
                summary: "Add retry".into(),
                content: "z".into(),
                lineno: 3,
            },
        ];
        let summaries = unique_summaries(&with_dup);
        assert_eq!(summaries.len(), 2);
        let _ = lines; // suppress unused warning
    }

    #[test]
    fn parse_line_range_valid() {
        assert_eq!(parse_line_range("10-25").unwrap(), (10, 25));
        assert_eq!(parse_line_range("1-1").unwrap(), (1, 1));
    }

    #[test]
    fn parse_line_range_invalid_format() {
        assert!(parse_line_range("25").is_err());
        assert!(parse_line_range("abc-def").is_err());
    }

    #[test]
    fn parse_line_range_reversed_is_error() {
        assert!(parse_line_range("25-10").is_err());
    }

    #[test]
    fn truncate_llm_short_unchanged() {
        let s = "hello";
        assert_eq!(truncate_llm(s, 100), "hello");
    }

    #[test]
    fn truncate_llm_long_is_clipped() {
        let s = "a".repeat(200);
        let out = truncate_llm(&s, 50);
        assert!(out.contains("[... truncated]"));
        assert!(out.chars().count() < 200);
    }

    #[test]
    fn trunc_display_short_unchanged() {
        assert_eq!(trunc_display("hello", 10), "hello");
    }

    #[test]
    fn trunc_display_long_adds_ellipsis() {
        let out = trunc_display("abcdefghij", 5);
        assert!(out.ends_with('…'));
        assert!(out.chars().count() <= 5);
    }

    #[test]
    fn build_llm_prompt_includes_file_and_range() {
        let lines = parse_blame_output(SAMPLE_PORCELAIN);
        let prompt = build_llm_prompt("src/payment.rs", &lines, Some((3, 4)));
        assert!(prompt.contains("src/payment.rs"));
        assert!(prompt.contains("lines 3–4"));
        assert!(prompt.contains("Fix the null pointer"));
    }

    #[test]
    fn build_llm_prompt_all_lines_when_no_range() {
        let lines = parse_blame_output(SAMPLE_PORCELAIN);
        let prompt = build_llm_prompt("src/main.rs", &lines, None);
        assert!(prompt.contains("all lines"));
    }

    #[test]
    fn empty_porcelain_gives_empty_vec() {
        assert!(parse_blame_output("").is_empty());
    }
}
