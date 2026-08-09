use anyhow::{Context, Result};
use clap::Subcommand;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::{config::Config, creds, llm, memory, usage::UsageTracker};

// ── Subcommand enum ───────────────────────────────────────────────────────────

#[derive(Subcommand)]
pub enum GitAction {
    /// Scan staged diff for blocked keywords — safe to use as a pre-commit hook
    Check,
    /// Generate a conventional commit message from staged changes
    Commit {
        /// Run `git commit -m` with the generated message automatically
        #[arg(long)]
        apply: bool,
    },
    /// Draft a structured PR description from branch changes
    Pr {
        /// Base branch to diff against
        #[arg(long, default_value = "main")]
        base: String,
    },
    /// Install dev-claw as a Git pre-commit hook
    Hook,
}

// ── LLM prompts ───────────────────────────────────────────────────────────────

const COMMIT_PROMPT: &str = r#"You are a Git commit message generator following Conventional Commits.

Analyze the provided staged diff and generate exactly one commit message.

Format:  <type>(<optional-scope>): <short description>

         [optional body — only if motivation is non-obvious]

Types: feat | fix | docs | style | refactor | test | chore | perf | ci | build

Rules:
- Subject line: max 72 chars, imperative mood ("add" not "added"), no trailing period
- Scope = the module or component most affected (omit if too broad)
- Body only if the why is not obvious from the diff
- Output ONLY the commit message — no explanation, no markdown fences"#;

const PR_PROMPT: &str = r#"You are a PR description writer for a software engineering team.

Generate a structured PR description in Markdown from the provided commits and diff.

## Summary
- <bullet>

## Changes
### Features
- <new capability>

### Bug Fixes
- <fix>

### Breaking Changes
- <change, or omit section if none>

## Testing
- <what was tested>

Rules:
- Factual and concise; no filler phrases
- Omit empty sections entirely
- Output only the Markdown — no surrounding code fences"#;

// ── Domain type ───────────────────────────────────────────────────────────────

struct Violation {
    file: String,
    line: u32,
    keyword: String,
    context: String,
}

// ── Public entry point ────────────────────────────────────────────────────────

pub async fn run(action: GitAction) -> Result<()> {
    let cfg = Config::load()?;
    match action {
        GitAction::Check => run_check(&cfg),
        GitAction::Commit { apply } => run_commit(&cfg, apply).await,
        GitAction::Pr { base } => run_pr(&cfg, &base).await,
        GitAction::Hook => run_hook(),
    }
}

// ── Action handlers ───────────────────────────────────────────────────────────

fn run_check(cfg: &Config) -> Result<()> {
    let keywords = effective_keywords(cfg);
    let diff = get_staged_diff()?;

    if diff.trim().is_empty() {
        println!("✓  Nothing staged to check.");
        return Ok(());
    }

    let violations = find_violations(&diff, &keywords);
    if violations.is_empty() {
        println!("✓  No blocked keywords in staged changes.");
        return Ok(());
    }

    print_violations(&violations);
    anyhow::bail!(
        "{} violation(s) found — fix before committing.",
        violations.len()
    )
}

async fn run_commit(cfg: &Config, apply: bool) -> Result<()> {
    let diff = get_staged_diff()?;
    if diff.trim().is_empty() {
        anyhow::bail!("Nothing staged. Run `git add <files>` first.");
    }

    let warning = enforce_quota(cfg)?;
    let stat = get_staged_stat()?;
    let user = format!(
        "## Staged stat\n{stat}\n\n## Diff\n{}",
        truncate(&diff, 8_000)
    );
    let message = call_llm(COMMIT_PROMPT, &user).await?;
    let message = message.trim().to_string();

    println!("\n{message}\n");

    if apply {
        git_commit(&message)?;
        println!("✓  Committed.");
    } else {
        println!("  To apply:  dev-claw git commit --apply");
    }

    print_warning(warning);
    Ok(())
}

async fn run_pr(cfg: &Config, base: &str) -> Result<()> {
    let commits = get_branch_commits(base)?;
    if commits.trim().is_empty() {
        anyhow::bail!("No commits ahead of '{base}'. Nothing to draft.");
    }

    let warning = enforce_quota(cfg)?;
    let diff = get_branch_diff(base)?;
    let user = format!(
        "## Commits\n{commits}\n\n## Diff\n{}",
        truncate(&diff, 8_000)
    );
    let body = call_llm(PR_PROMPT, &user).await?;

    println!("\n{}\n", body.trim());
    print_warning(warning);
    Ok(())
}

fn run_hook() -> Result<()> {
    let hook_path = find_hooks_dir()?.join("pre-commit");
    install_hook(&hook_path)
}

// ── Git operations ────────────────────────────────────────────────────────────

fn get_staged_diff() -> Result<String> {
    git_output(&["diff", "--staged"])
}

fn get_staged_stat() -> Result<String> {
    git_output(&["diff", "--staged", "--stat"])
}

fn get_branch_commits(base: &str) -> Result<String> {
    git_output(&["log", "--oneline", &format!("{base}..HEAD")])
}

fn get_branch_diff(base: &str) -> Result<String> {
    git_output(&["diff", &format!("{base}...HEAD")])
}

fn git_commit(message: &str) -> Result<()> {
    let status = Command::new("git")
        .args(["commit", "-m", message])
        .status()
        .context("Failed to run git commit")?;
    if !status.success() {
        anyhow::bail!("git commit failed — see output above.");
    }
    Ok(())
}

fn git_output(args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .args(args)
        .output()
        .context("git not found in PATH")?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        anyhow::bail!("git {}: {}", args.join(" "), err.trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn find_hooks_dir() -> Result<PathBuf> {
    let raw = git_output(&["rev-parse", "--git-dir"])?;
    Ok(PathBuf::from(raw.trim()).join("hooks"))
}

fn install_hook(hook_path: &Path) -> Result<()> {
    if hook_path.exists() {
        anyhow::bail!(
            "Pre-commit hook already exists: {}\nRemove it manually to replace.",
            hook_path.display()
        );
    }
    let script = "#!/bin/sh\ndev-claw git check\n";
    std::fs::write(hook_path, script)
        .with_context(|| format!("Cannot write {}", hook_path.display()))?;
    set_executable(hook_path)?;
    println!("✓  Installed: {}", hook_path.display());
    println!("   Runs `dev-claw git check` before every commit.");
    Ok(())
}

#[cfg(unix)]
fn set_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path)?.permissions();
    perms.set_mode(perms.mode() | 0o111);
    std::fs::set_permissions(path, perms).context("Cannot set executable bit on hook")
}

#[cfg(not(unix))]
fn set_executable(_path: &Path) -> Result<()> {
    Ok(()) // Windows does not use executable bits
}

// ── Diff scanning ─────────────────────────────────────────────────────────────

fn find_violations(diff: &str, keywords: &[String]) -> Vec<Violation> {
    let mut violations = Vec::new();
    let mut file = String::new();
    let mut line_num = 0u32;

    for raw in diff.lines() {
        if let Some(f) = parse_diff_filename(raw) {
            file = f;
            line_num = 0;
            continue;
        }
        if let Some(n) = parse_hunk_start(raw) {
            line_num = n;
            continue;
        }
        if is_diff_metadata(raw) || raw.starts_with('-') {
            continue;
        }
        if is_added_line(raw) {
            violations.extend(line_violations(&raw[1..], &file, line_num, keywords));
        }
        line_num += 1;
    }
    violations
}

fn line_violations(content: &str, file: &str, line: u32, keywords: &[String]) -> Vec<Violation> {
    keywords
        .iter()
        .filter(|kw| content.contains(kw.as_str()))
        .map(|kw| Violation {
            file: file.to_string(),
            line,
            keyword: kw.clone(),
            context: content.trim().to_string(),
        })
        .collect()
}

fn parse_diff_filename(line: &str) -> Option<String> {
    if !line.starts_with("diff --git ") {
        return None;
    }
    line.split(' ')
        .next_back()
        .map(|s| s.trim_start_matches("b/").to_string())
}

fn parse_hunk_start(line: &str) -> Option<u32> {
    if !line.starts_with("@@ ") {
        return None;
    }
    // "@@ -old +new,count @@ ..." — we want `new`
    let after_plus = line.split('+').nth(1)?;
    let num_str = after_plus.split([',', ' ']).next()?;
    num_str.parse().ok()
}

fn is_added_line(line: &str) -> bool {
    line.starts_with('+') && !line.starts_with("+++")
}

fn is_diff_metadata(line: &str) -> bool {
    line.starts_with("index ") || line.starts_with("+++ ") || line.starts_with('\\')
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn effective_keywords(cfg: &Config) -> Vec<String> {
    cfg.git
        .as_ref()
        .and_then(|g| g.block_on_keywords.clone())
        .unwrap_or_else(|| {
            vec![
                "console.log".into(),
                "debugger".into(),
                "print!".into(),
                "TODO".into(),
                "FIXME".into(),
            ]
        })
}

fn enforce_quota(cfg: &Config) -> Result<Option<String>> {
    let use_cfg = cfg.usage.as_ref();
    let total = use_cfg.map(|u| u.monthly_limit()).unwrap_or(200);
    let cmd = use_cfg.map(|u| u.git_limit()).unwrap_or(60);
    let warn = use_cfg.map(|u| u.warn_at_percent()).unwrap_or(80);
    UsageTracker::open()?.check_and_record("git", cmd, total, warn)
}

async fn call_llm(system: &str, user: &str) -> Result<String> {
    let ctx = memory::context_for_prompt("git");
    let provider = resolve_provider()?;
    let api_key = creds::load(&provider)?;
    let result = llm::client_for(&provider, &api_key)
        .complete(&format!("{ctx}{system}"), user)
        .await?;
    memory::record_interaction("git", &result);
    Ok(result)
}

fn resolve_provider() -> Result<String> {
    std::env::var("DEV_CLAW_PROVIDER")
        .ok()
        .or_else(creds::auto_detect_provider)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "No API key configured.\nRun: dev-claw config set-key --provider deepseek"
            )
        })
}

fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let trimmed: String = s.chars().take(max_chars).collect();
    format!("{trimmed}\n\n[... diff truncated at {max_chars} chars ...]")
}

fn print_violations(violations: &[Violation]) {
    eprintln!();
    for v in violations {
        eprintln!("  ⚠  {}:{}  [{}]", v.file, v.line, v.keyword);
        eprintln!("     {}", v.context);
    }
    eprintln!();
}

fn print_warning(w: Option<String>) {
    if let Some(msg) = w {
        eprintln!("⚠  {msg}");
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn tmp() -> TempDir {
        tempfile::tempdir().unwrap()
    }

    // --- parse_diff_filename ---

    #[test]
    fn filename_extracted_from_diff_header() {
        let line = "diff --git a/src/main.rs b/src/main.rs";
        assert_eq!(parse_diff_filename(line).unwrap(), "src/main.rs");
    }

    #[test]
    fn filename_with_nested_path() {
        let line = "diff --git a/a/b/c.ts b/a/b/c.ts";
        assert_eq!(parse_diff_filename(line).unwrap(), "a/b/c.ts");
    }

    #[test]
    fn non_diff_lines_return_none() {
        assert!(parse_diff_filename("+added line").is_none());
        assert!(parse_diff_filename("@@ -1,3 +1,4 @@").is_none());
        assert!(parse_diff_filename("index abc..def").is_none());
    }

    // --- parse_hunk_start ---

    #[test]
    fn hunk_start_extracts_new_file_line() {
        assert_eq!(
            parse_hunk_start("@@ -1,5 +10,8 @@ fn main() {").unwrap(),
            10
        );
    }

    #[test]
    fn hunk_start_single_line_hunk() {
        assert_eq!(parse_hunk_start("@@ -0,0 +1 @@").unwrap(), 1);
    }

    #[test]
    fn hunk_start_first_line() {
        assert_eq!(parse_hunk_start("@@ -1,3 +1,4 @@").unwrap(), 1);
    }

    #[test]
    fn non_hunk_lines_return_none() {
        assert!(parse_hunk_start("diff --git a/f b/f").is_none());
        assert!(parse_hunk_start("+added").is_none());
    }

    // --- is_added_line / is_diff_metadata ---

    #[test]
    fn added_line_detection() {
        assert!(is_added_line("+new line"));
        assert!(!is_added_line("+++ b/file")); // file header excluded
        assert!(!is_added_line("-removed"));
        assert!(!is_added_line(" context"));
    }

    #[test]
    fn diff_metadata_detection() {
        assert!(is_diff_metadata("index abc..def 100644"));
        assert!(is_diff_metadata("+++ b/file.rs"));
        assert!(is_diff_metadata("\\ No newline at end of file"));
        assert!(!is_diff_metadata("+added line"));
        assert!(!is_diff_metadata(" context line"));
    }

    // --- find_violations ---

    const SAMPLE_DIFF: &str = "\
diff --git a/src/lib.rs b/src/lib.rs
index abc..def 100644
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,4 +1,6 @@
 fn greet() {
+    console.log(\"debug\");
+    let x = 42;
     println!(\"hello\");
 }
";

    #[test]
    fn detects_keyword_in_added_line() {
        let kws = vec!["console.log".to_string()];
        let vs = find_violations(SAMPLE_DIFF, &kws);
        assert_eq!(vs.len(), 1);
        assert_eq!(vs[0].file, "src/lib.rs");
        assert_eq!(vs[0].keyword, "console.log");
        assert!(vs[0].context.contains("console.log"));
    }

    #[test]
    fn ignores_keyword_on_removed_lines() {
        let diff = "\
diff --git a/f b/f
index a..b 100644
--- a/f
+++ b/f
@@ -1 +1 @@
-console.log(\"old\")
+clean_code()
";
        let kws = vec!["console.log".to_string()];
        assert!(find_violations(diff, &kws).is_empty());
    }

    #[test]
    fn no_false_positives_on_clean_diff() {
        let diff = "\
diff --git a/f b/f
index a..b 100644
--- a/f
+++ b/f
@@ -1 +1 @@
+clean code here
";
        let kws = vec!["console.log".to_string(), "debugger".to_string()];
        assert!(find_violations(diff, &kws).is_empty());
    }

    #[test]
    fn detects_multiple_keywords_same_line() {
        let diff = "\
diff --git a/f b/f
index a..b 100644
--- a/f
+++ b/f
@@ -1 +1 @@
+console.log(debugger)
";
        let kws = vec!["console.log".to_string(), "debugger".to_string()];
        assert_eq!(find_violations(diff, &kws).len(), 2);
    }

    #[test]
    fn detects_violations_across_multiple_files() {
        let diff = "\
diff --git a/a.js b/a.js
index a..b 100644
--- a/a.js
+++ b/a.js
@@ -1 +1 @@
+console.log(1)
diff --git a/b.js b/b.js
index c..d 100644
--- a/b.js
+++ b/b.js
@@ -1 +1 @@
+console.log(2)
";
        let kws = vec!["console.log".to_string()];
        let vs = find_violations(diff, &kws);
        assert_eq!(vs.len(), 2);
        assert_eq!(vs[0].file, "a.js");
        assert_eq!(vs[1].file, "b.js");
    }

    // --- truncate ---

    #[test]
    fn short_string_is_unchanged() {
        assert_eq!(truncate("hello", 100), "hello");
    }

    #[test]
    fn long_string_is_truncated_with_note() {
        let s = "a".repeat(200);
        let out = truncate(&s, 100);
        assert!(out.len() > 100);
        assert!(out.contains("truncated"));
    }

    // --- effective_keywords ---

    #[test]
    fn defaults_include_common_debug_patterns() {
        let cfg = Config::default();
        let kws = effective_keywords(&cfg);
        assert!(kws.iter().any(|k| k == "console.log"));
        assert!(kws.iter().any(|k| k == "debugger"));
        assert!(kws.iter().any(|k| k == "TODO"));
    }

    #[test]
    fn config_keywords_override_defaults() {
        use crate::config::GitConfig;
        let cfg = Config {
            git: Some(GitConfig {
                commit_style: None,
                block_on_keywords: Some(vec!["MY_DEBUG".to_string()]),
            }),
            ..Default::default()
        };
        let kws = effective_keywords(&cfg);
        assert_eq!(kws, vec!["MY_DEBUG".to_string()]);
    }

    // --- install_hook ---

    #[test]
    fn hook_file_is_written_correctly() {
        let d = tmp();
        let path = d.path().join("pre-commit");
        install_hook(&path).unwrap();
        let content = fs::read_to_string(&path).unwrap();
        assert!(content.starts_with("#!/bin/sh"));
        assert!(content.contains("dev-claw git check"));
    }

    #[test]
    fn refuses_to_overwrite_existing_hook() {
        let d = tmp();
        let path = d.path().join("pre-commit");
        fs::write(&path, "existing hook").unwrap();
        assert!(install_hook(&path).is_err());
    }
}
