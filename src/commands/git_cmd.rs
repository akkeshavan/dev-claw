use anyhow::{Context, Result};
use clap::Subcommand;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::{config::Config, creds, llm, memory, usage::UsageTracker, utils::confirm};

// ── Subcommand enum ───────────────────────────────────────────────────────────

#[derive(Subcommand)]
pub enum GitAction {
    /// Scan staged diff for blocked keywords — safe to use as a pre-commit hook
    #[command(after_help = "\
EXAMPLES:
  dev-claw git check
  dev-claw git hook   # install as pre-commit hook so this runs automatically")]
    Check,

    /// Generate a conventional commit message from staged changes
    #[command(after_help = "\
EXAMPLES:
  git add -p && dev-claw git commit
  dev-claw git commit --apply   # also runs git commit -m <message>")]
    Commit {
        #[arg(long)]
        apply: bool,
    },

    /// Draft a structured PR description; optionally create it via gh CLI
    #[command(after_help = "\
EXAMPLES:
  dev-claw git pr                        # draft description only
  dev-claw git pr --base develop         # diff against a different base
  dev-claw git pr --create               # draft + push branch + gh pr create
  dev-claw git pr --create --base develop")]
    Pr {
        #[arg(long, default_value = "main")]
        base: String,
        /// Push the branch and create the PR via gh CLI
        #[arg(long)]
        create: bool,
    },

    /// Install dev-claw as a Git pre-commit hook
    #[command(after_help = "\
EXAMPLES:
  dev-claw git hook   # installs .git/hooks/pre-commit running `dev-claw git check`")]
    Hook,

    /// Generate a branch name from a plain-English description
    #[command(after_help = "\
EXAMPLES:
  dev-claw git branch \"fix login timeout for oauth users\"
  dev-claw git branch \"add dark mode toggle\" --apply   # also runs git checkout -b")]
    Branch {
        /// Plain-English description of the branch's purpose
        description: String,
        /// Run git checkout -b <name> automatically
        #[arg(long)]
        apply: bool,
    },

    /// Squash the last N commits into one with an AI-generated message
    #[command(after_help = "\
EXAMPLES:
  dev-claw git squash 3          # preview squashed message for last 3 commits
  dev-claw git squash 3 --apply  # actually squash (git reset --soft HEAD~3 + commit)
  dev-claw git squash            # defaults to last 2 commits")]
    Squash {
        /// Number of commits to squash (default: 2)
        #[arg(default_value = "2")]
        n: u32,
        /// Run git reset --soft and commit automatically (prompts for confirmation)
        #[arg(long)]
        apply: bool,
    },

    /// Smart push — sets upstream if missing, guards secrets, optional force
    #[command(after_help = "\
EXAMPLES:
  dev-claw git push                   # push current branch, set upstream if needed
  dev-claw git push --force           # push with --force-with-lease (safer force)")]
    Push {
        /// Use --force-with-lease (prompts for confirmation)
        #[arg(long)]
        force: bool,
    },

    /// AI-assisted merge conflict resolution — resolves each conflict, then stages
    #[command(after_help = "\
EXAMPLES:
  git merge feature/x     # produces conflicts
  dev-claw git resolve    # resolves each conflict with AI suggestion + confirmation
  git merge --continue    # or git rebase --continue after resolve")]
    Resolve,

    /// Fetch origin and rebase the current branch onto origin/<base>
    #[command(after_help = "\
EXAMPLES:
  dev-claw git sync                    # fetch + rebase onto origin/main
  dev-claw git sync --base develop     # rebase onto origin/develop")]
    Sync {
        #[arg(long, default_value = "main")]
        base: String,
    },

    /// AI narrative summary of recent commits
    #[command(after_help = "\
EXAMPLES:
  dev-claw git log                          # summarise commits from the last week
  dev-claw git log --since \"2 days ago\"
  dev-claw git log --since v1.2.0           # since a tag
  dev-claw git log --format slack           # emoji bullets for Slack")]
    Log {
        /// How far back to look (e.g. \"yesterday\", \"2 days ago\", tag name like \"v1.2.0\")
        #[arg(long, default_value = "1 week ago")]
        since: String,
        /// Output style: plain | slack | markdown
        #[arg(long, default_value = "plain")]
        format: String,
    },

    /// Generate an AI interactive-rebase plan for the last N commits
    #[command(after_help = "\
EXAMPLES:
  dev-claw git rebase 5    # plan for HEAD~5..HEAD; launches git rebase -i automatically")]
    Rebase {
        /// Number of commits to include in the rebase plan
        #[arg(value_name = "N")]
        n: u32,
    },

    /// Stash current changes with an AI-generated description
    #[command(after_help = "\
EXAMPLES:
  dev-claw git stash                          # AI-generated stash message
  dev-claw git stash --message \"WIP: auth\"   # provide your own message")]
    Stash {
        /// Provide a custom stash message instead of generating one
        #[arg(long)]
        message: Option<String>,
    },

    /// Cherry-pick a commit; resolves conflicts with AI if they arise
    #[command(after_help = "\
EXAMPLES:
  dev-claw git cherry-pick abc1234
  dev-claw git cherry-pick abc1234   # conflicts? AI resolves them automatically")]
    CherryPick {
        /// SHA of the commit to cherry-pick
        #[arg(value_name = "SHA")]
        sha: String,
    },
    /// Natural language query — dev-claw git "<what you want to do>"
    #[command(external_subcommand)]
    Nl(Vec<String>),
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
The FIRST line must be the PR title (imperative, max 70 chars, no markdown).
Then a blank line.
Then the body:

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
- First line is the title — no # heading, no bold, plain text only
- Factual and concise; no filler phrases
- Omit empty sections entirely
- Output only title + blank line + Markdown body — no surrounding fences"#;

const BRANCH_PROMPT: &str = r#"You are a git branch naming assistant.

Generate exactly one branch name from the given description.

Rules:
- Format: <type>/<short-kebab-description>
- Types: feat, fix, chore, docs, refactor, test, perf, ci, release
- Max 50 chars total, all lowercase, hyphens only (no underscores)
- Imperative and concise ("add-oauth" not "adding-oauth-support")
- Output ONLY the branch name — no explanation, no markdown"#;

const SQUASH_PROMPT: &str = r#"You are a Git commit message generator following Conventional Commits.

The developer is squashing multiple commits into one. Analyze the combined diff and commit list to produce a single representative commit message.

Format:  <type>(<optional-scope>): <short description>

         [optional body — only if motivation is non-obvious]

Types: feat | fix | docs | style | refactor | test | chore | perf | ci | build

Rules:
- Subject line: max 72 chars, imperative mood, no trailing period
- If the squashed commits span multiple concerns, pick the dominant type
- Output ONLY the commit message — no explanation, no markdown fences"#;

const RESOLVE_PROMPT: &str = r#"You are a merge conflict resolver.

You will be shown a merge conflict between two versions of code.
Produce the correctly merged version by analyzing both sides.

Rules:
- Output ONLY the resolved code — no conflict markers (<<<<<<<, =======, >>>>>>>)
- No explanation, no markdown fences
- If one side is clearly the correct change, use it
- If both sides add valid changes, merge them intelligently
- Preserve indentation and style of the surrounding code"#;

const LOG_PROMPT_PLAIN: &str = r#"You are a developer summarizing recent git activity.

Write a concise narrative summary of the commits provided. Group related changes.

Rules:
- 3-8 bullet points, plain text dashes
- Present tense: "add X", "fix Y", "refactor Z"
- Skip trivial changes (fmt, typos, version bumps)
- Output plain text bullets only — no headings, no markdown"#;

const LOG_PROMPT_SLACK: &str = r#"You are a developer writing a Slack standup update from git commits.

Rules:
- 3-8 emoji bullets (✅ for done, 🔧 for fix, 🚀 for feat, 📝 for docs)
- *Bold* the key nouns using Slack formatting
- Present tense, concise
- Output only the bullets — no headings"#;

const LOG_PROMPT_MARKDOWN: &str = r#"You are a developer summarizing recent git activity in Markdown.

Rules:
- ## heading with the date range
- Grouped bullet points under ### Features, ### Fixes, ### Chores (omit empty sections)
- Present tense, concise
- Output only the Markdown — no fences"#;

const REBASE_PROMPT: &str = r#"You are a git rebase planner.

Given a list of recent commits (oldest first, as output by `git log --oneline HEAD~N..HEAD` reversed), generate a git rebase-interactive todo list.

Format each line exactly as: <action> <sha> <description>

Actions:
- pick   — keep commit as-is
- squash — squash into the previous commit (combines messages)
- fixup  — like squash but discards this commit's message
- reword — keep commit but you'll edit the message

Rules:
- The FIRST commit (chronologically oldest) must be "pick"
- Group small fix/style commits into the preceding feature commit using fixup
- Squash closely related commits with squash
- Output ONLY the todo lines — no explanation, no blank lines before content"#;

const STASH_PROMPT: &str = r#"You are a git stash message writer.

Generate a concise stash description for the given diff.

Rules:
- One line, max 60 chars
- Start with "WIP: "
- Describe what was being worked on, not what files changed
- Output ONLY the stash message — no explanation"#;

// ── Conflict parsing ──────────────────────────────────────────────────────────

struct Conflict {
    start_line: usize,
    end_line: usize,
    ours: String,
    theirs: String,
}

fn parse_conflicts(content: &str) -> Vec<Conflict> {
    let lines: Vec<&str> = content.lines().collect();
    let mut conflicts = Vec::new();
    let mut i = 0;

    while i < lines.len() {
        if !lines[i].starts_with("<<<<<<<") {
            i += 1;
            continue;
        }
        let start = i;
        let mut sep = None;
        let mut end = None;

        for (j, line) in lines.iter().enumerate().skip(i + 1) {
            if *line == "=======" && sep.is_none() {
                sep = Some(j);
            } else if line.starts_with(">>>>>>>") {
                end = Some(j);
                break;
            }
        }

        if let (Some(sep_line), Some(end_line)) = (sep, end) {
            conflicts.push(Conflict {
                start_line: start,
                end_line,
                ours: lines[start + 1..sep_line].join("\n"),
                theirs: lines[sep_line + 1..end_line].join("\n"),
            });
            i = end_line + 1;
        } else {
            i += 1;
        }
    }
    conflicts
}

fn apply_resolutions(original: &str, conflicts: &[Conflict], resolutions: &[String]) -> String {
    let lines: Vec<&str> = original.lines().collect();
    let trailing_newline = original.ends_with('\n');
    let mut parts: Vec<String> = Vec::new();
    let mut cursor = 0;

    for (conflict, resolution) in conflicts.iter().zip(resolutions.iter()) {
        if cursor < conflict.start_line {
            parts.push(lines[cursor..conflict.start_line].join("\n"));
        }
        parts.push(resolution.clone());
        cursor = conflict.end_line + 1;
    }
    if cursor < lines.len() {
        parts.push(lines[cursor..].join("\n"));
    }

    let mut out = parts.join("\n");
    if trailing_newline && !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

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
        GitAction::Pr { base, create } => run_pr(&cfg, &base, create).await,
        GitAction::Hook => run_hook(),
        GitAction::Branch { description, apply } => run_branch(&cfg, &description, apply).await,
        GitAction::Squash { n, apply } => run_squash(&cfg, n, apply).await,
        GitAction::Push { force } => run_push(force).await,
        GitAction::Resolve => run_resolve(&cfg).await,
        GitAction::Sync { base } => run_sync(&cfg, &base).await,
        GitAction::Log { since, format } => run_log(&cfg, &since, &format).await,
        GitAction::Rebase { n } => run_rebase(&cfg, n).await,
        GitAction::Stash { message } => run_stash(&cfg, message.as_deref()).await,
        GitAction::CherryPick { sha } => run_cherry_pick(&cfg, &sha).await,
        GitAction::Nl(_) => unreachable!("Nl handled in main"),
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

async fn run_pr(cfg: &Config, base: &str, create: bool) -> Result<()> {
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
    let body = body.trim().to_string();

    println!("\n{body}\n");

    if create {
        create_github_pr(&body, base)?;
    }

    print_warning(warning);
    Ok(())
}

fn create_github_pr(body: &str, base: &str) -> Result<()> {
    // First line is the title; rest is the body
    let mut lines = body.lines();
    let title = lines.next().unwrap_or("").trim();
    let pr_body: String = lines.collect::<Vec<_>>().join("\n").trim().to_string();

    if title.is_empty() {
        anyhow::bail!("Could not extract PR title from generated description.");
    }

    // Push branch, setting upstream if needed
    let branch = current_branch()?;
    if !git_has_upstream() {
        println!("Pushing branch '{branch}'...");
        git_exec_args(&["push", "--set-upstream", "origin", &branch])?;
    }

    println!("Creating PR...");
    let status = Command::new("gh")
        .args([
            "pr", "create", "--title", title, "--body", &pr_body, "--base", base,
        ])
        .status()
        .context("gh CLI not found — install from https://cli.github.com")?;

    if !status.success() {
        anyhow::bail!(
            "gh pr create failed — check that you are authenticated with `gh auth login`"
        );
    }
    Ok(())
}

fn run_hook() -> Result<()> {
    let hook_path = find_hooks_dir()?.join("pre-commit");
    install_hook(&hook_path)
}

async fn run_branch(cfg: &Config, description: &str, apply: bool) -> Result<()> {
    let warning = enforce_quota(cfg)?;
    let name = call_llm(BRANCH_PROMPT, description).await?;
    let name = name.trim().to_string();

    println!("\n{name}\n");

    if apply {
        git_exec_args(&["checkout", "-b", &name])?;
        println!("✓  Created and switched to '{name}'.");
    } else {
        println!("  To create:  git checkout -b {name}");
        println!("  Or:         dev-claw git branch --apply \"{description}\"");
    }

    print_warning(warning);
    Ok(())
}

async fn run_squash(cfg: &Config, n: u32, apply: bool) -> Result<()> {
    let commits = git_output(&["log", "--oneline", &format!("HEAD~{n}..HEAD")])?;
    if commits.trim().is_empty() {
        anyhow::bail!("Fewer than {n} commits on this branch.");
    }

    let warning = enforce_quota(cfg)?;
    let diff = git_output(&["diff", &format!("HEAD~{n}..HEAD")])?;
    let user = format!(
        "## Commits being squashed\n{commits}\n\n## Combined diff\n{}",
        truncate(&diff, 8_000)
    );
    let message = call_llm(SQUASH_PROMPT, &user).await?;
    let message = message.trim().to_string();

    println!("\n{message}\n");

    if apply {
        if !confirm(&format!("Squash {n} commits with this message? (y/N) "))? {
            println!("Aborted.");
            return Ok(());
        }
        git_exec_args(&["reset", "--soft", &format!("HEAD~{n}")])?;
        git_commit(&message)?;
        println!("✓  Squashed {n} commits.");
    } else {
        println!("  To apply:  dev-claw git squash {n} --apply");
    }

    print_warning(warning);
    Ok(())
}

async fn run_push(force: bool) -> Result<()> {
    let branch = current_branch()?;

    if force
        && !confirm(&format!(
            "Force-push branch '{branch}' with --force-with-lease? (y/N) "
        ))?
    {
        println!("Aborted.");
        return Ok(());
    }

    let mut cmd = Command::new("git");
    cmd.arg("push");
    if !git_has_upstream() {
        cmd.args(["--set-upstream", "origin", &branch]);
    }
    if force {
        cmd.arg("--force-with-lease");
    }

    let status = cmd.status().context("git push failed to start")?;
    if !status.success() {
        anyhow::bail!("git push failed — see output above");
    }
    println!("✓  Pushed branch '{branch}'.");
    Ok(())
}

pub async fn run_resolve(cfg: &Config) -> Result<()> {
    let conflicted = get_conflicted_files()?;
    if conflicted.is_empty() {
        println!("✓  No merge conflicts found.");
        return Ok(());
    }

    println!("Found {} conflicted file(s):", conflicted.len());
    for f in &conflicted {
        println!("  {f}");
    }
    println!();

    let warning = enforce_quota(cfg)?;
    let mut resolved_files: Vec<String> = Vec::new();

    for file in &conflicted {
        let content =
            std::fs::read_to_string(file).with_context(|| format!("Cannot read {file}"))?;
        let conflicts = parse_conflicts(&content);
        if conflicts.is_empty() {
            continue;
        }

        println!("Resolving {file} ({} conflict(s))...", conflicts.len());
        let mut resolutions: Vec<String> = Vec::new();
        let mut skipped = false;

        for (idx, conflict) in conflicts.iter().enumerate() {
            let user = format!(
                "## File: {file}\n## Conflict {} of {}\n\n### Ours (HEAD)\n{}\n\n### Theirs\n{}",
                idx + 1,
                conflicts.len(),
                conflict.ours,
                conflict.theirs
            );
            let suggestion = call_llm(RESOLVE_PROMPT, &user).await?;
            let suggestion = suggestion.trim().to_string();

            println!("\n--- Conflict {} ---", idx + 1);
            println!("<<< OURS\n{}", conflict.ours);
            println!("=== THEIRS\n{}", conflict.theirs);
            println!(">>> SUGGESTED RESOLUTION\n{suggestion}\n");

            if confirm("Apply this resolution? (y/N) ")? {
                resolutions.push(suggestion);
            } else {
                println!("  Skipping — leaving conflict markers in place.");
                skipped = true;
                break;
            }
        }

        if !skipped && resolutions.len() == conflicts.len() {
            let new_content = apply_resolutions(&content, &conflicts, &resolutions);
            std::fs::write(file, new_content).with_context(|| format!("Cannot write {file}"))?;
            resolved_files.push(file.clone());
            println!("✓  {file} resolved.");
        }
    }

    if !resolved_files.is_empty() {
        let mut args = vec!["add".to_string()];
        args.extend(resolved_files.clone());
        let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        git_exec_args(&arg_refs)?;
        println!("\n✓  Staged {} resolved file(s).", resolved_files.len());
    }

    print_warning(warning);
    Ok(())
}

async fn run_sync(cfg: &Config, base: &str) -> Result<()> {
    println!("Fetching origin...");
    git_exec_args(&["fetch", "origin"])?;

    println!("Rebasing onto origin/{base}...");
    let ok = Command::new("git")
        .args(["rebase", &format!("origin/{base}")])
        .status()
        .context("git rebase failed to start")?
        .success();

    if !ok {
        println!("\nConflicts detected. Attempting AI resolution...\n");
        run_resolve(cfg).await?;
        println!("\nContinuing rebase...");
        Command::new("git")
            .args(["rebase", "--continue"])
            .env("GIT_EDITOR", "true")
            .status()
            .context("git rebase --continue failed")?;
    }

    println!("✓  Synced with origin/{base}.");
    Ok(())
}

async fn run_log(cfg: &Config, since: &str, format: &str) -> Result<()> {
    let log = git_output(&["log", "--oneline", &format!("--since={since}")])?;
    if log.trim().is_empty() {
        // Try treating `since` as a tag/ref
        let tag_log = git_output(&["log", "--oneline", &format!("{since}..HEAD")]);
        match tag_log {
            Ok(l) if !l.trim().is_empty() => return run_log_summary(cfg, since, format, &l).await,
            _ => {
                println!("No commits found since '{since}'.");
                return Ok(());
            }
        }
    }
    run_log_summary(cfg, since, format, &log).await
}

async fn run_log_summary(cfg: &Config, since: &str, format: &str, log: &str) -> Result<()> {
    let warning = enforce_quota(cfg)?;
    let prompt = match format {
        "slack" => LOG_PROMPT_SLACK,
        "markdown" => LOG_PROMPT_MARKDOWN,
        _ => LOG_PROMPT_PLAIN,
    };
    let user = format!("## Git log since {since}\n\n{log}");
    let summary = call_llm(prompt, &user).await?;
    println!("\n{}\n", summary.trim());
    print_warning(warning);
    Ok(())
}

async fn run_rebase(cfg: &Config, n: u32) -> Result<()> {
    // git log lists newest-first; reverse to oldest-first for the todo
    let commits_raw = git_output(&["log", "--oneline", &format!("HEAD~{n}..HEAD")])?;
    if commits_raw.trim().is_empty() {
        anyhow::bail!("Fewer than {n} commits on this branch.");
    }
    let commits: Vec<&str> = commits_raw.lines().collect();
    let commits_oldest_first: Vec<&str> = commits.iter().copied().rev().collect();
    let commits_for_llm = commits_oldest_first.join("\n");

    let warning = enforce_quota(cfg)?;
    let plan = call_llm(REBASE_PROMPT, &commits_for_llm).await?;
    let plan = plan.trim().to_string();

    println!("\nSuggested rebase plan for HEAD~{n}:\n\n{plan}\n");

    if !confirm("Apply this plan with git rebase -i? (y/N) ")? {
        println!("Aborted.");
        return Ok(());
    }

    launch_rebase_with_plan(&plan, n)?;
    print_warning(warning);
    Ok(())
}

#[cfg(unix)]
fn launch_rebase_with_plan(plan: &str, n: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let tmp = std::env::temp_dir();
    let todo = tmp.join("dev-claw-rebase-todo.txt");
    let editor = tmp.join("dev-claw-rebase-editor.sh");

    std::fs::write(&todo, plan)?;
    std::fs::write(
        &editor,
        format!("#!/bin/sh\ncp {} \"$1\"\n", todo.display()),
    )?;
    std::fs::set_permissions(&editor, std::fs::Permissions::from_mode(0o755))?;

    let status = Command::new("git")
        .args(["rebase", "-i", &format!("HEAD~{n}")])
        .env("GIT_SEQUENCE_EDITOR", &editor)
        .status()
        .context("git rebase -i failed to start")?;

    if !status.success() {
        anyhow::bail!(
            "git rebase -i failed — resolve any conflicts then run `git rebase --continue`"
        );
    }
    println!("✓  Rebase complete.");
    Ok(())
}

#[cfg(not(unix))]
fn launch_rebase_with_plan(plan: &str, n: u32) -> Result<()> {
    let todo = std::env::temp_dir().join("dev-claw-rebase-todo.txt");
    std::fs::write(&todo, plan)?;
    println!("Todo saved to: {}", todo.display());
    println!("Run:  git rebase -i HEAD~{n}");
    println!("Then paste the plan into the editor that opens.");
    Ok(())
}

async fn run_stash(cfg: &Config, message: Option<&str>) -> Result<()> {
    let msg = if let Some(m) = message {
        m.to_string()
    } else {
        let diff = get_all_changes()?;
        if diff.trim().is_empty() {
            anyhow::bail!("Nothing to stash — no modified or staged files.");
        }
        let warning = enforce_quota(cfg)?;
        let generated = call_llm(STASH_PROMPT, &truncate(&diff, 4_000)).await?;
        let msg = generated.trim().to_string();
        println!("\nStash message: {msg}\n");
        if !confirm("Stash with this message? (y/N) ")? {
            println!("Aborted.");
            return Ok(());
        }
        print_warning(warning);
        msg
    };

    git_exec_args(&["stash", "push", "-m", &msg])?;
    println!("✓  Stashed: {msg}");
    Ok(())
}

async fn run_cherry_pick(cfg: &Config, sha: &str) -> Result<()> {
    let info = git_output(&["show", "--no-patch", "--format=%h %s", sha])
        .unwrap_or_else(|_| sha.to_string());
    println!("Cherry-picking: {}", info.trim());

    let output = Command::new("git")
        .args(["cherry-pick", sha])
        .output()
        .context("git cherry-pick failed to start")?;

    if output.status.success() {
        println!("✓  Cherry-picked {sha}.");
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    if stderr.contains("conflict") || !get_conflicted_files()?.is_empty() {
        println!("\nConflicts detected. Attempting AI resolution...\n");
        run_resolve(cfg).await?;
        println!("\nContinuing cherry-pick...");
        Command::new("git")
            .args(["cherry-pick", "--continue", "--no-edit"])
            .status()
            .context("git cherry-pick --continue failed")?;
        println!("✓  Cherry-picked {sha} (conflicts resolved).");
    } else {
        anyhow::bail!("git cherry-pick failed:\n{stderr}");
    }
    Ok(())
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

fn get_all_changes() -> Result<String> {
    let staged = git_output(&["diff", "--staged"])?;
    let unstaged = git_output(&["diff"])?;
    Ok(format!("{staged}{unstaged}"))
}

fn get_conflicted_files() -> Result<Vec<String>> {
    let out = git_output(&["diff", "--name-only", "--diff-filter=U"])?;
    Ok(out
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(String::from)
        .collect())
}

fn current_branch() -> Result<String> {
    let out = git_output(&["rev-parse", "--abbrev-ref", "HEAD"])?;
    let branch = out.trim().to_string();
    if branch == "HEAD" {
        anyhow::bail!("You are in detached HEAD state — checkout a branch first.");
    }
    Ok(branch)
}

fn git_has_upstream() -> bool {
    Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
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

fn git_exec_args(args: &[&str]) -> Result<()> {
    let status = Command::new("git")
        .args(args)
        .status()
        .with_context(|| format!("git {} failed to start", args.join(" ")))?;
    if !status.success() {
        anyhow::bail!("git {} failed", args.join(" "));
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
    Ok(())
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
        assert!(!is_added_line("+++ b/file"));
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

    // --- parse_conflicts ---

    #[test]
    fn parses_single_conflict() {
        let content = "\
before
<<<<<<< HEAD
our version
=======
their version
>>>>>>> feature/x
after
";
        let conflicts = parse_conflicts(content);
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].ours, "our version");
        assert_eq!(conflicts[0].theirs, "their version");
    }

    #[test]
    fn parses_multiple_conflicts() {
        let content = "\
<<<<<<< HEAD
a
=======
b
>>>>>>> branch
middle
<<<<<<< HEAD
c
=======
d
>>>>>>> branch
";
        let conflicts = parse_conflicts(content);
        assert_eq!(conflicts.len(), 2);
        assert_eq!(conflicts[0].ours, "a");
        assert_eq!(conflicts[1].ours, "c");
    }

    #[test]
    fn parses_multiline_conflict_sides() {
        let content = "\
<<<<<<< HEAD
line one
line two
=======
other one
other two
other three
>>>>>>> branch
";
        let conflicts = parse_conflicts(content);
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].ours, "line one\nline two");
        assert_eq!(conflicts[0].theirs, "other one\nother two\nother three");
    }

    #[test]
    fn returns_empty_vec_when_no_conflicts() {
        let content = "normal file\nno conflicts here\n";
        assert!(parse_conflicts(content).is_empty());
    }

    // --- apply_resolutions ---

    #[test]
    fn applies_single_resolution() {
        let original = "\
before
<<<<<<< HEAD
ours
=======
theirs
>>>>>>> branch
after
";
        let conflicts = parse_conflicts(original);
        let resolutions = vec!["resolved".to_string()];
        let result = apply_resolutions(original, &conflicts, &resolutions);
        assert!(!result.contains("<<<<<<<"));
        assert!(result.contains("resolved"));
        assert!(result.contains("before"));
        assert!(result.contains("after"));
    }

    #[test]
    fn preserves_trailing_newline() {
        let original = "<<<<<<< HEAD\na\n=======\nb\n>>>>>>> x\n";
        let conflicts = parse_conflicts(original);
        let result = apply_resolutions(original, &conflicts, &["a".to_string()]);
        assert!(result.ends_with('\n'));
    }

    #[test]
    fn applies_multiple_resolutions() {
        let original = "\
<<<<<<< HEAD
a
=======
b
>>>>>>> x
middle
<<<<<<< HEAD
c
=======
d
>>>>>>> x
";
        let conflicts = parse_conflicts(original);
        assert_eq!(conflicts.len(), 2);
        let resolutions = vec!["resolved_a".to_string(), "resolved_c".to_string()];
        let result = apply_resolutions(original, &conflicts, &resolutions);
        assert!(result.contains("resolved_a"));
        assert!(result.contains("resolved_c"));
        assert!(result.contains("middle"));
        assert!(!result.contains("<<<<<<<"));
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
