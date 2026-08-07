use anyhow::{bail, Result};
use clap::Subcommand;
use std::collections::BTreeMap;
use std::path::Path;

// ── Subcommand enum ───────────────────────────────────────────────────────────

#[derive(Subcommand)]
pub enum EnvAction {
    /// Compare .env against .env.example and report drift
    Check,
    /// Scan staged diff for secret leaks before committing
    Guard,
    /// Install pre-commit and post-checkout git hooks
    Hook,
}

// ── Constants ─────────────────────────────────────────────────────────────────

const SECRET_KEYWORDS: &[&str] = &[
    "SECRET", "PASSWORD", "API_KEY", "TOKEN", "PRIVATE_KEY",
    "ACCESS_KEY", "AUTH_KEY", "CREDENTIALS", "PASSPHRASE", "PASSWD",
];

const PLACEHOLDER_VALUES: &[&str] = &[
    "changeme", "placeholder", "your_key_here", "xxx", "todo",
    "replace_me", "example", "test", "dummy", "fake",
];

// ── Public entry point ────────────────────────────────────────────────────────

pub async fn run(action: EnvAction) -> Result<()> {
    match action {
        EnvAction::Check => check_env(),
        EnvAction::Guard => guard_env(),
        EnvAction::Hook  => install_hooks(),
    }
}

// ── Check subcommand ──────────────────────────────────────────────────────────

struct CheckResult {
    missing: Vec<String>,
    empty:   Vec<String>,
    extra:   Vec<String>,
}

fn check_env() -> Result<()> {
    let dir = std::env::current_dir()?;
    let example_path = find_example_file(&dir)
        .ok_or_else(|| anyhow::anyhow!("No .env.example / .env.sample / .env.template found"))?;
    let env_path = dir.join(".env");
    if !env_path.exists() {
        bail!(".env not found — copy {} and fill in values", example_path.display());
    }
    let example = parse_env_file(&std::fs::read_to_string(&example_path)?);
    let env     = parse_env_file(&std::fs::read_to_string(&env_path)?);
    let result  = compare_envs(&example, &env);
    print_check_result(&result, &example_path.display().to_string());
    if !result.missing.is_empty() {
        std::process::exit(1);
    }
    Ok(())
}

fn find_example_file(dir: &Path) -> Option<std::path::PathBuf> {
    for name in [".env.example", ".env.sample", ".env.template"] {
        let p = dir.join(name);
        if p.exists() { return Some(p); }
    }
    None
}

fn compare_envs(
    example: &BTreeMap<String, Option<String>>,
    env:     &BTreeMap<String, Option<String>>,
) -> CheckResult {
    let mut missing = Vec::new();
    let mut empty   = Vec::new();
    for key in example.keys() {
        match env.get(key) {
            None                          => missing.push(key.clone()),
            Some(None)                    => empty.push(key.clone()),
            Some(Some(v)) if v.is_empty() => empty.push(key.clone()),
            Some(Some(_))                 => {}
        }
    }
    let extra = env.keys()
        .filter(|k| !example.contains_key(*k))
        .cloned()
        .collect();
    CheckResult { missing, empty, extra }
}

fn print_check_result(r: &CheckResult, example_name: &str) {
    if r.missing.is_empty() && r.empty.is_empty() && r.extra.is_empty() {
        println!("✓  .env is in sync with {example_name}");
        return;
    }
    for key in &r.missing { println!("✗  MISSING : {key}  (required by {example_name})"); }
    for key in &r.empty   { println!("⚠  EMPTY   : {key}  (set in {example_name} but blank in .env)"); }
    for key in &r.extra   { println!("·  EXTRA   : {key}  (in .env but not in {example_name})"); }
    if !r.missing.is_empty() {
        println!();
        println!("  {} variable(s) missing — add them to .env", r.missing.len());
    }
}

// ── Guard subcommand ──────────────────────────────────────────────────────────

struct SecretLeak {
    file:    String,
    line:    u32,
    pattern: String,
    context: String,
}

fn guard_env() -> Result<()> {
    let diff = staged_diff()?;
    if diff.is_empty() {
        println!("✓  No staged changes.");
        return Ok(());
    }
    let leaks = find_leaks(&diff);
    if leaks.is_empty() {
        println!("✓  No secret leaks detected in staged changes.");
        return Ok(());
    }
    for leak in &leaks {
        println!("✗  {}:{} — {} leaked", leak.file, leak.line, leak.pattern);
        println!("   {}", leak.context);
    }
    println!();
    println!("  {} potential secret leak(s) found.", leaks.len());
    println!("  To bypass: git commit --no-verify   (only if you are sure this is safe)");
    std::process::exit(1);
}

fn staged_diff() -> Result<String> {
    let out = std::process::Command::new("git")
        .args(["diff", "--cached"])
        .output()?;
    if !out.status.success() {
        anyhow::bail!("git diff failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn find_leaks(diff: &str) -> Vec<SecretLeak> {
    let mut leaks   = Vec::new();
    let mut file    = String::new();
    let mut line_no = 0u32;
    for raw in diff.lines() {
        if let Some(f) = parse_diff_filename(raw) { file = f; line_no = 0; continue; }
        if let Some(n) = parse_hunk_start(raw)    { line_no = n; continue; }
        if is_diff_meta(raw) || raw.starts_with('-') { continue; }
        if is_added_line(raw) {
            if let Some(leak) = check_line_for_leak(&raw[1..], &file, line_no) {
                leaks.push(leak);
            }
        }
        line_no += 1;
    }
    leaks
}

fn check_line_for_leak(content: &str, file: &str, line: u32) -> Option<SecretLeak> {
    let name = Path::new(file).file_name().and_then(|n| n.to_str()).unwrap_or("");
    if name.ends_with(".example") || name.ends_with(".sample") || name.ends_with(".template") {
        return None; // example files are intentionally public
    }
    if is_secret_env_file(file) {
        return check_secret_file_line(content, file, line);
    }
    check_kv_leak(content, file, line)
}

fn check_secret_file_line(content: &str, file: &str, line: u32) -> Option<SecretLeak> {
    let trimmed = content.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') { return None; }
    let (_, val) = parse_env_line(content)?;
    if !val.as_deref().is_some_and(|v| !v.is_empty() && !is_placeholder_value(v)) {
        return None;
    }
    Some(SecretLeak {
        file: file.to_string(),
        line,
        pattern: "secret env file committed".into(),
        context: redact_value(content),
    })
}

fn check_kv_leak(content: &str, file: &str, line: u32) -> Option<SecretLeak> {
    let (key, val) = parse_env_line(content)?;
    let key_upper  = key.to_uppercase();
    let keyword    = SECRET_KEYWORDS.iter().find(|&&kw| key_upper.contains(kw))?;
    if !val.as_deref().is_some_and(|v| !v.is_empty() && !is_placeholder_value(v)) {
        return None;
    }
    Some(SecretLeak {
        file: file.to_string(),
        line,
        pattern: (*keyword).to_string(),
        context: redact_value(content),
    })
}

fn redact_value(content: &str) -> String {
    if let Some(eq) = content.find('=') {
        return format!("{}=[REDACTED]", content[..eq].trim());
    }
    "[REDACTED]".to_string()
}

// ── Hook subcommand ───────────────────────────────────────────────────────────

fn install_hooks() -> Result<()> {
    let hooks_dir = hooks_path()?;
    std::fs::create_dir_all(&hooks_dir)?;
    install_hook(&hooks_dir.join("pre-commit"),    "dev-claw env guard\n")?;
    install_hook(&hooks_dir.join("post-checkout"), "dev-claw env check || true\n")?;
    println!("✓  Hooks installed in {}", hooks_dir.display());
    println!("   pre-commit   : runs `dev-claw env guard`");
    println!("   post-checkout: runs `dev-claw env check` on branch switch");
    Ok(())
}

fn hooks_path() -> Result<std::path::PathBuf> {
    let out = std::process::Command::new("git")
        .args(["rev-parse", "--git-dir"])
        .output()?;
    if !out.status.success() {
        bail!("Not inside a git repository");
    }
    let git_dir = String::from_utf8_lossy(&out.stdout).trim().to_string();
    Ok(std::path::PathBuf::from(git_dir).join("hooks"))
}

fn install_hook(path: &Path, snippet: &str) -> Result<()> {
    if path.exists() {
        let existing = std::fs::read_to_string(path)?;
        if existing.contains(snippet.trim()) {
            println!("·  {} — already installed, skipped", path.display());
            return Ok(());
        }
        println!("·  {} — already exists; append manually:", path.display());
        println!("   {}", snippet.trim());
        return Ok(());
    }
    std::fs::write(path, format!("#!/bin/sh\n{snippet}"))?;
    set_executable(path)?;
    println!("✓  Created {}", path.display());
    Ok(())
}

#[cfg(unix)]
fn set_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path)?.permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms)?;
    Ok(())
}

#[cfg(not(unix))]
fn set_executable(_path: &Path) -> Result<()> {
    Ok(())
}

// ── .env file parsing ─────────────────────────────────────────────────────────

pub fn parse_env_file(content: &str) -> BTreeMap<String, Option<String>> {
    content.lines().filter_map(parse_env_line).collect()
}

fn parse_env_line(line: &str) -> Option<(String, Option<String>)> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') { return None; }
    let stripped = trimmed.strip_prefix("export ").unwrap_or(trimmed);
    let eq  = stripped.find('=')?;
    let key = stripped[..eq].trim().to_string();
    if key.is_empty() { return None; }
    let raw_val = stripped[eq + 1..].trim();
    let val = if raw_val.is_empty() { None } else { Some(parse_value(raw_val)) };
    Some((key, val))
}

fn parse_value(raw: &str) -> String {
    if raw.starts_with('"') && raw.ends_with('"') && raw.len() >= 2 {
        return raw[1..raw.len() - 1].replace("\\\"", "\"").replace("\\n", "\n");
    }
    if raw.starts_with('\'') && raw.ends_with('\'') && raw.len() >= 2 {
        return raw[1..raw.len() - 1].to_string();
    }
    if let Some(idx) = raw.find(" #") {
        return raw[..idx].trim().to_string();
    }
    raw.to_string()
}

fn is_placeholder_value(val: &str) -> bool {
    let lower = val.to_lowercase();
    if PLACEHOLDER_VALUES.iter().any(|&p| lower == p) { return true; }
    val.starts_with("${") || val.starts_with('<') || val.contains("REPLACE")
}

fn is_secret_env_file(file: &str) -> bool {
    let name = Path::new(file).file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");
    if name.ends_with(".example") || name.ends_with(".sample") || name.ends_with(".template") {
        return false;
    }
    name == ".env" || name.starts_with(".env.")
}

// ── Diff helpers (local copies — keep modules independent) ────────────────────

fn parse_diff_filename(line: &str) -> Option<String> {
    let rest   = line.strip_prefix("diff --git ")?;
    let b_part = rest.split(' ').next_back()?;
    Some(b_part.trim_start_matches("b/").to_string())
}

fn parse_hunk_start(line: &str) -> Option<u32> {
    let rest  = line.strip_prefix("@@ -")?;
    let after = rest.split('+').nth(1)?;
    after.split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()
}

fn is_diff_meta(line: &str) -> bool {
    line.starts_with("index ")
        || line.starts_with("+++ ")
        || line.starts_with("--- ")
        || line.starts_with("\\ No newline")
}

fn is_added_line(line: &str) -> bool {
    line.starts_with('+') && !line.starts_with("+++")
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // --- .env file parsing ---

    #[test]
    fn parses_basic_key_value() {
        let env = parse_env_file("KEY=value\nOTHER=123\n");
        assert_eq!(env["KEY"].as_deref(), Some("value"));
        assert_eq!(env["OTHER"].as_deref(), Some("123"));
    }

    #[test]
    fn ignores_comment_lines() {
        let env = parse_env_file("# This is a comment\nKEY=value\n");
        assert!(!env.contains_key("# This is a comment"));
        assert_eq!(env["KEY"].as_deref(), Some("value"));
    }

    #[test]
    fn ignores_blank_lines() {
        let env = parse_env_file("\n\nKEY=value\n\n");
        assert_eq!(env.len(), 1);
    }

    #[test]
    fn parses_double_quoted_value() {
        let env = parse_env_file(r#"KEY="hello world""#);
        assert_eq!(env["KEY"].as_deref(), Some("hello world"));
    }

    #[test]
    fn parses_single_quoted_value() {
        let env = parse_env_file("KEY='hello world'");
        assert_eq!(env["KEY"].as_deref(), Some("hello world"));
    }

    #[test]
    fn strips_inline_comment() {
        let env = parse_env_file("KEY=value # inline comment");
        assert_eq!(env["KEY"].as_deref(), Some("value"));
    }

    #[test]
    fn handles_export_prefix() {
        let env = parse_env_file("export KEY=value");
        assert_eq!(env["KEY"].as_deref(), Some("value"));
    }

    #[test]
    fn empty_value_gives_none() {
        let env = parse_env_file("KEY=");
        assert_eq!(env.get("KEY"), Some(&None));
    }

    #[test]
    fn double_quoted_escape_sequences() {
        let env = parse_env_file(r#"KEY="line1\nline2""#);
        assert!(env["KEY"].as_deref().unwrap().contains('\n'));
    }

    // --- parse_value ---

    #[test]
    fn parse_value_strips_outer_double_quotes() {
        assert_eq!(parse_value(r#""hello""#), "hello");
    }

    #[test]
    fn parse_value_strips_outer_single_quotes() {
        assert_eq!(parse_value("'hello'"), "hello");
    }

    #[test]
    fn parse_value_strips_trailing_hash_comment() {
        assert_eq!(parse_value("value # comment"), "value");
    }

    #[test]
    fn parse_value_hash_without_space_is_literal() {
        assert_eq!(parse_value("val#ue"), "val#ue");
    }

    // --- is_placeholder_value ---

    #[test]
    fn changeme_is_placeholder() {
        assert!(is_placeholder_value("changeme"));
    }

    #[test]
    fn template_syntax_is_placeholder() {
        assert!(is_placeholder_value("${MY_VAR}"));
        assert!(is_placeholder_value("<your-key-here>"));
    }

    #[test]
    fn real_key_is_not_placeholder() {
        assert!(!is_placeholder_value("sk-realkey12345"));
    }

    // --- is_secret_env_file ---

    #[test]
    fn dot_env_is_secret() {
        assert!(is_secret_env_file("path/to/.env"));
        assert!(is_secret_env_file(".env.local"));
        assert!(is_secret_env_file(".env.production"));
    }

    #[test]
    fn example_files_are_not_secret() {
        assert!(!is_secret_env_file(".env.example"));
        assert!(!is_secret_env_file(".env.sample"));
        assert!(!is_secret_env_file(".env.template"));
    }

    // --- compare_envs ---

    #[test]
    fn compare_reports_missing_keys() {
        let example = parse_env_file("API_KEY=\nDB_URL=\n");
        let env     = parse_env_file("DB_URL=postgres://localhost\n");
        let r = compare_envs(&example, &env);
        assert_eq!(r.missing, vec!["API_KEY"]);
    }

    #[test]
    fn compare_reports_empty_keys() {
        let example = parse_env_file("API_KEY=\n");
        let env     = parse_env_file("API_KEY=\n");
        let r = compare_envs(&example, &env);
        assert!(r.missing.is_empty());
        assert_eq!(r.empty, vec!["API_KEY"]);
    }

    #[test]
    fn compare_reports_extra_keys() {
        let example = parse_env_file("KEY=\n");
        let env     = parse_env_file("KEY=value\nEXTRA=val\n");
        let r = compare_envs(&example, &env);
        assert_eq!(r.extra, vec!["EXTRA"]);
    }

    #[test]
    fn compare_in_sync_reports_nothing() {
        let example = parse_env_file("KEY=\n");
        let env     = parse_env_file("KEY=value\n");
        let r = compare_envs(&example, &env);
        assert!(r.missing.is_empty() && r.empty.is_empty() && r.extra.is_empty());
    }

    // --- find_leaks ---

    #[test]
    fn find_leaks_detects_secret_kv_in_any_file() {
        let diff = "diff --git a/config.js b/config.js\n\
                    @@ -1,0 +1,1 @@\n\
                    +API_KEY=sk-real123secret\n";
        let leaks = find_leaks(diff);
        assert_eq!(leaks.len(), 1);
        assert!(leaks[0].pattern.contains("API_KEY"));
    }

    #[test]
    fn find_leaks_ignores_placeholder_values() {
        let diff = "diff --git a/config.js b/config.js\n\
                    @@ -1,0 +1,1 @@\n\
                    +API_KEY=changeme\n";
        let leaks = find_leaks(diff);
        assert!(leaks.is_empty());
    }

    #[test]
    fn find_leaks_ignores_example_env_files() {
        let diff = "diff --git a/.env.example b/.env.example\n\
                    @@ -1,0 +1,1 @@\n\
                    +SECRET_KEY=sk-reallooksreal\n";
        let leaks = find_leaks(diff);
        assert!(leaks.is_empty());
    }

    #[test]
    fn find_leaks_detects_committed_secret_env_file() {
        let diff = "diff --git a/.env b/.env\n\
                    @@ -1,0 +1,1 @@\n\
                    +DB_PASSWORD=supersecretpassword\n";
        let leaks = find_leaks(diff);
        assert_eq!(leaks.len(), 1);
        assert!(leaks[0].context.contains("[REDACTED]"));
    }

    // --- diff helpers ---

    #[test]
    fn parse_diff_filename_extracts_path() {
        let line = "diff --git a/src/main.rs b/src/main.rs";
        assert_eq!(parse_diff_filename(line).as_deref(), Some("src/main.rs"));
    }

    #[test]
    fn parse_hunk_start_extracts_new_line() {
        let line = "@@ -10,4 +20,6 @@ fn main() {";
        assert_eq!(parse_hunk_start(line), Some(20));
    }
}
