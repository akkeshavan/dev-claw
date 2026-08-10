/// Natural language command planner.
///
/// Scoped mode (`dev-claw git "<query>"`) sends only that namespace's tool schemas
/// (~8-13 tools, ~150-250 tokens). Use this for single-domain requests.
///
/// Global mode (`dev-claw "<query>"`) sends all schemas (~40 tools, ~600 tokens)
/// and should be reserved for cross-domain workflows such as
/// "review staged changes and if clean commit and push".
use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::Value;

use crate::{
    commands::{
        cloud_cmd::{self, CloudAction},
        deps_cmd::{self, DepsAction},
        env_cmd::{self, EnvAction},
        forensic_cmd::{self, ForensicAction},
        git_cmd::{self, GitAction},
        init, memory_cmd,
        memory_cmd::MemoryAction,
        mock_cmd::{self, MockAction},
        release_cmd::{self, ReleaseAction},
        review_cmd::{self, ReviewAction},
        standup_cmd, usage_cmd,
        workflow_cmd::{self, WorkflowAction},
    },
    creds, llm,
    utils::confirm,
};

// ── Tool schemas (compact text format — cheaper than JSON Schema) ─────────────

const GIT_TOOLS: &str = "\
TOOLS (git):
  git_commit(apply=true:bool) — generate Conventional Commits message from staged diff; apply executes git commit
  git_squash(n=2:int, apply=true:bool) — squash last N commits with AI message; apply runs git reset --soft + commit (prompts)
  git_pr(base=\"main\":str, create=false:bool) — draft PR description; create=true pushes branch and runs gh pr create
  git_branch(description:str, apply=true:bool) — AI branch name from plain-English description; apply runs git checkout -b
  git_push(force=false:bool) — smart push, sets upstream if missing; force uses --force-with-lease
  git_resolve() — AI merge conflict resolution, shows each conflict and confirms before applying
  git_sync(base=\"main\":str) — git fetch origin then rebase onto origin/base, auto-resolves conflicts
  git_log(since=\"1 week ago\":str, format=\"plain\":str) — AI narrative summary; format: plain|slack|markdown
  git_rebase(n:int) — AI interactive-rebase plan for last N commits, launches git rebase -i
  git_stash(message=null:str?) — stash changes with AI-generated description or a custom message
  git_cherry_pick(sha:str) — cherry-pick a commit, AI-resolves conflicts if needed
  git_check() — scan staged diff for blocked keywords (no LLM)";

const REVIEW_TOOLS: &str = "\
TOOLS (review):
  review_diff(staged=false:bool, focus=\"all\":str) — AI code review of uncommitted changes; focus: all|security|performance|style
  review_pr(number:int, focus=\"all\":str) — review a GitHub PR by number via gh CLI";

const DEPS_TOOLS: &str = "\
TOOLS (deps):
  deps_audit(triage=false:bool) — run cargo audit / npm audit / govulncheck / pip-audit; triage sends results to LLM for risk ranking
  deps_outdated() — check for newer dependency versions across all detected package managers";

const RELEASE_TOOLS: &str = "\
TOOLS (release):
  release_notes(since=null:str?) — draft release notes from commits since the last tag (or a specific ref)
  release_cut(version=null:str?, dry_run=false:bool) — suggest semver bump, draft notes, create git tag";

const ENV_TOOLS: &str = "\
TOOLS (env):
  env_check() — diff .env against .env.example; report missing, empty, or extra keys
  env_guard() — scan staged changes for secrets about to be committed
  env_hook() — install pre-commit (env guard) and post-checkout (env check) git hooks";

const WORKFLOW_TOOLS: &str = "\
TOOLS (workflow):
  workflow_ls() — list all workflows defined in .devclawrc
  workflow_run(name:str, dry_run=false:bool) — execute a named workflow step by step
  workflow_publish(name:str) — publish a workflow as a public GitHub Gist
  workflow_import(url:str) — import a workflow from a Gist URL or raw TOML URL";

const MEMORY_TOOLS: &str = "\
TOOLS (memory):
  memory_ls() — show stored notes and feedback for the current project
  memory_note(text:str, command=null:str?) — add a persistent learning note (injected into future LLM calls)
  memory_feedback(text:str, command=null:str?) — record feedback about LLM output quality
  memory_clear(global=false:bool, all=false:bool) — clear memory; global=true clears cross-project memory";

const MOCK_TOOLS: &str = "\
TOOLS (mock):
  mock_gen(schema:str, count=5:int, format=\"json\":str, out=null:str?) — generate fixture data from a schema file
  mock_factory(types_file:str, out=null:str?) — generate TypeScript factory functions from a type definitions file";

const FORENSIC_TOOLS: &str = "\
TOOLS (forensic):
  forensic_explain(file:str, lines=null:str?) — explain WHY code exists using git blame and LLM; lines e.g. \"40-80\"
  forensic_blame(file:str, lines=null:str?) — pretty-print annotated git blame output (no LLM)";

const CLOUD_TOOLS: &str = "\
TOOLS (cloud):
  cloud_up(provider=\"do\":str, size=null:str?, region=null:str?, image=null:str?, name=null:str?) — provision an ephemeral VM
  cloud_ls() — list VMs created by dev-claw
  cloud_down(vm:str) — destroy a VM by name or ID (prompts for confirmation)
  cloud_ssh(vm:str) — print the SSH command for a VM";

const TOP_TOOLS: &str = "\
TOOLS (top-level):
  standup(since=\"yesterday\":str, format=\"plain\":str) — AI standup update from git history; format: plain|slack|markdown
  init() — auto-detect stack and write a starter .devclawrc
  usage() — show LLM call quota usage for this month
  doctor() — diagnose a build error (NOTE: requires piped stdin — cannot be used in NL mode alone)";

fn all_tools() -> String {
    [
        GIT_TOOLS,
        REVIEW_TOOLS,
        DEPS_TOOLS,
        RELEASE_TOOLS,
        ENV_TOOLS,
        WORKFLOW_TOOLS,
        MEMORY_TOOLS,
        MOCK_TOOLS,
        FORENSIC_TOOLS,
        CLOUD_TOOLS,
        TOP_TOOLS,
    ]
    .join("\n\n")
}

fn tools_for(namespace: &str) -> &'static str {
    match namespace {
        "git" => GIT_TOOLS,
        "review" => REVIEW_TOOLS,
        "deps" => DEPS_TOOLS,
        "release" => RELEASE_TOOLS,
        "env" => ENV_TOOLS,
        "workflow" => WORKFLOW_TOOLS,
        "memory" => MEMORY_TOOLS,
        "mock" => MOCK_TOOLS,
        "forensic" => FORENSIC_TOOLS,
        "cloud" => CLOUD_TOOLS,
        _ => TOP_TOOLS,
    }
}

// ── Planner prompt ────────────────────────────────────────────────────────────

const PLANNER_SYSTEM: &str = r#"You are a dev-claw command planner. Map the user's natural language request to the correct sequence of dev-claw tool calls.

Return a JSON array — no markdown fences, no explanation. Each element:
  {"tool":"<exact name>","args":{<param>:<value>},"desc":"<one-line human description>"}

Rules:
- Use ONLY the tools listed. Never invent tool names.
- Omit args that equal their stated defaults (keep payload small).
- In NL context, execution flags like "apply" default to true unless the user says "preview" or "dry-run".
- Order steps logically — prerequisites first.
- If no tool fits the request, return [].
- Return ONLY the JSON array."#;

// ── Plan step ─────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct Step {
    tool: String,
    #[serde(default)]
    args: Value,
    desc: String,
}

// ── Public entry points ───────────────────────────────────────────────────────

pub async fn run_scoped(namespace: &str, query: &str) -> Result<()> {
    let tools = tools_for(namespace);
    let context = git_context();
    run_plan(tools, &context, query).await
}

/// Global NL mode — sends all ~40 tool schemas (~600 extra tokens vs. scoped).
/// Use only for cross-domain workflows. For single-domain requests prefer:
///   dev-claw git "<query>" | dev-claw review "<query>" | dev-claw deps "<query>" etc.
pub async fn run_global(query: &str) -> Result<()> {
    eprintln!(
        "⚠  Global NL mode sends all tool schemas (~600 extra tokens).\n   \
         For cheaper planning use a scoped command: dev-claw git \"...\", dev-claw review \"...\"\n"
    );
    let tools = all_tools();
    let context = git_context();
    run_plan(&tools, &context, query).await
}

// ── Core pipeline ─────────────────────────────────────────────────────────────

async fn run_plan(tools: &str, context: &str, query: &str) -> Result<()> {
    println!("Planning...");
    let steps = call_planner(tools, context, query).await?;

    if steps.is_empty() {
        anyhow::bail!(
            "Could not map your request to any available tool.\n\
             Try a more specific query or use the structured CLI (dev-claw --help)."
        );
    }

    let n = steps.len();
    println!("\nPlan ({n} step{}):", if n == 1 { "" } else { "s" });
    for (i, step) in steps.iter().enumerate() {
        println!("  {}. {}", i + 1, step.desc);
    }
    println!();

    if !confirm("Proceed? (y/N) ")? {
        println!("Aborted.");
        return Ok(());
    }
    println!();

    for (i, step) in steps.into_iter().enumerate() {
        if n > 1 {
            println!(
                "── Step {} of {n} ──────────────────────────────────────",
                i + 1
            );
        }
        dispatch(&step.tool, &step.args).await?;
        if n > 1 {
            println!();
        }
    }

    Ok(())
}

async fn call_planner(tools: &str, context: &str, query: &str) -> Result<Vec<Step>> {
    let provider = std::env::var("DEV_CLAW_PROVIDER")
        .ok()
        .or_else(creds::auto_detect_provider)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "No API key configured.\nRun: dev-claw config set-key --provider deepseek"
            )
        })?;
    let api_key = creds::load(&provider)?;

    let system = format!("{PLANNER_SYSTEM}\n\n{tools}\n\nCURRENT CONTEXT:\n{context}");
    let raw = llm::client_for(&provider, &api_key)
        .complete(&system, query)
        .await?;

    let json = raw
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();

    serde_json::from_str::<Vec<Step>>(json)
        .with_context(|| format!("Planner returned invalid JSON:\n{raw}"))
}

// ── Dispatcher ────────────────────────────────────────────────────────────────

async fn dispatch(tool: &str, args: &Value) -> Result<()> {
    match tool {
        // ── git ───────────────────────────────────────────────────────────────
        "git_commit" => {
            git_cmd::run(GitAction::Commit {
                apply: bool_arg(args, "apply", true),
            })
            .await
        }
        "git_squash" => {
            git_cmd::run(GitAction::Squash {
                n: u32_arg(args, "n", 2),
                apply: bool_arg(args, "apply", true),
            })
            .await
        }
        "git_pr" => {
            git_cmd::run(GitAction::Pr {
                base: str_arg(args, "base", "main"),
                create: bool_arg(args, "create", false),
            })
            .await
        }
        "git_branch" => {
            git_cmd::run(GitAction::Branch {
                description: str_arg(args, "description", ""),
                apply: bool_arg(args, "apply", true),
            })
            .await
        }
        "git_push" => {
            git_cmd::run(GitAction::Push {
                force: bool_arg(args, "force", false),
            })
            .await
        }
        "git_resolve" => git_cmd::run(GitAction::Resolve).await,
        "git_sync" => {
            git_cmd::run(GitAction::Sync {
                base: str_arg(args, "base", "main"),
            })
            .await
        }
        "git_log" => {
            git_cmd::run(GitAction::Log {
                since: str_arg(args, "since", "1 week ago"),
                format: str_arg(args, "format", "plain"),
            })
            .await
        }
        "git_rebase" => {
            git_cmd::run(GitAction::Rebase {
                n: u32_arg(args, "n", 3),
            })
            .await
        }
        "git_stash" => {
            git_cmd::run(GitAction::Stash {
                message: opt_str(args, "message"),
            })
            .await
        }
        "git_cherry_pick" => {
            git_cmd::run(GitAction::CherryPick {
                sha: str_arg(args, "sha", ""),
            })
            .await
        }
        "git_check" => git_cmd::run(GitAction::Check).await,

        // ── review ────────────────────────────────────────────────────────────
        "review_diff" => {
            review_cmd::run(ReviewAction::Diff {
                staged: bool_arg(args, "staged", false),
                focus: str_arg(args, "focus", "all"),
            })
            .await
        }
        "review_pr" => {
            review_cmd::run(ReviewAction::Pr {
                number: u32_arg(args, "number", 0),
                focus: str_arg(args, "focus", "all"),
            })
            .await
        }

        // ── deps ──────────────────────────────────────────────────────────────
        "deps_audit" => {
            deps_cmd::run(DepsAction::Audit {
                triage: bool_arg(args, "triage", false),
            })
            .await
        }
        "deps_outdated" => deps_cmd::run(DepsAction::Outdated).await,

        // ── release ───────────────────────────────────────────────────────────
        "release_notes" => {
            release_cmd::run(ReleaseAction::Notes {
                since: opt_str(args, "since"),
            })
            .await
        }
        "release_cut" => {
            release_cmd::run(ReleaseAction::Cut {
                version: opt_str(args, "version"),
                dry_run: bool_arg(args, "dry_run", false),
            })
            .await
        }

        // ── env ───────────────────────────────────────────────────────────────
        "env_check" => env_cmd::run(EnvAction::Check).await,
        "env_guard" => env_cmd::run(EnvAction::Guard).await,
        "env_hook" => env_cmd::run(EnvAction::Hook).await,

        // ── workflow ──────────────────────────────────────────────────────────
        "workflow_ls" => workflow_cmd::run(WorkflowAction::Ls).await,
        "workflow_run" => {
            workflow_cmd::run(WorkflowAction::Run {
                name: str_arg(args, "name", ""),
                dry_run: bool_arg(args, "dry_run", false),
            })
            .await
        }
        "workflow_publish" => {
            workflow_cmd::run(WorkflowAction::Publish {
                name: str_arg(args, "name", ""),
            })
            .await
        }
        "workflow_import" => {
            workflow_cmd::run(WorkflowAction::Import {
                url: str_arg(args, "url", ""),
            })
            .await
        }

        // ── memory ────────────────────────────────────────────────────────────
        "memory_ls" => memory_cmd::run(MemoryAction::Ls).await,
        "memory_note" => {
            memory_cmd::run(MemoryAction::Note {
                text: str_arg(args, "text", ""),
                command: opt_str(args, "command"),
            })
            .await
        }
        "memory_feedback" => {
            memory_cmd::run(MemoryAction::Feedback {
                text: str_arg(args, "text", ""),
                command: opt_str(args, "command"),
            })
            .await
        }
        "memory_clear" => {
            memory_cmd::run(MemoryAction::Clear {
                global: bool_arg(args, "global", false),
                all: bool_arg(args, "all", false),
            })
            .await
        }

        // ── mock ──────────────────────────────────────────────────────────────
        "mock_gen" => {
            mock_cmd::run(MockAction::Gen {
                schema: str_arg(args, "schema", ""),
                count: u32_arg(args, "count", 5),
                format: str_arg(args, "format", "json"),
                out: opt_str(args, "out"),
            })
            .await
        }
        "mock_factory" => {
            mock_cmd::run(MockAction::Factory {
                types_file: str_arg(args, "types_file", ""),
                out: opt_str(args, "out"),
            })
            .await
        }

        // ── forensic ──────────────────────────────────────────────────────────
        "forensic_explain" => {
            forensic_cmd::run(ForensicAction::Explain {
                file: str_arg(args, "file", ""),
                lines: opt_str(args, "lines"),
            })
            .await
        }
        "forensic_blame" => {
            forensic_cmd::run(ForensicAction::Blame {
                file: str_arg(args, "file", ""),
                lines: opt_str(args, "lines"),
            })
            .await
        }

        // ── cloud ─────────────────────────────────────────────────────────────
        "cloud_up" => {
            cloud_cmd::run(CloudAction::Up {
                provider: str_arg(args, "provider", "do"),
                size: opt_str(args, "size"),
                region: opt_str(args, "region"),
                image: opt_str(args, "image"),
                name: opt_str(args, "name"),
            })
            .await
        }
        "cloud_ls" => cloud_cmd::run(CloudAction::Ls).await,
        "cloud_down" => {
            cloud_cmd::run(CloudAction::Down {
                vm: str_arg(args, "vm", ""),
            })
            .await
        }
        "cloud_ssh" => {
            cloud_cmd::run(CloudAction::Ssh {
                vm: str_arg(args, "vm", ""),
            })
            .await
        }

        // ── top-level ─────────────────────────────────────────────────────────
        "standup" => {
            standup_cmd::run(
                &str_arg(args, "since", "yesterday"),
                &str_arg(args, "format", "plain"),
            )
            .await
        }
        "init" => init::run().await,
        "usage" => usage_cmd::run().await,

        other => anyhow::bail!(
            "Unknown tool '{other}' — planner error. Report at https://github.com/akkeshavan/dev-claw/issues"
        ),
    }
}

// ── Context ───────────────────────────────────────────────────────────────────

fn git_context() -> String {
    let branch = git_out(&["rev-parse", "--abbrev-ref", "HEAD"]);
    let log = git_out(&["log", "--oneline", "-8"]);
    let status = git_out(&["status", "--short"]);
    let conflicts = git_out(&["diff", "--name-only", "--diff-filter=U"]);

    let mut ctx = format!("Branch: {branch}\nRecent commits:\n{log}\nWorking tree:\n{status}");
    if !conflicts.is_empty() {
        ctx.push_str(&format!("\nConflicted files:\n{conflicts}"));
    }
    ctx
}

fn git_out(args: &[&str]) -> String {
    std::process::Command::new("git")
        .args(args)
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

// ── Arg helpers ───────────────────────────────────────────────────────────────

fn bool_arg(args: &Value, key: &str, default: bool) -> bool {
    args.get(key).and_then(Value::as_bool).unwrap_or(default)
}

fn u32_arg(args: &Value, key: &str, default: u32) -> u32 {
    args.get(key)
        .and_then(Value::as_u64)
        .map(|n| n as u32)
        .unwrap_or(default)
}

fn str_arg(args: &Value, key: &str, default: &str) -> String {
    args.get(key)
        .and_then(Value::as_str)
        .unwrap_or(default)
        .to_string()
}

fn opt_str(args: &Value, key: &str) -> Option<String> {
    args.get(key).and_then(Value::as_str).map(String::from)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn bool_arg_returns_value_when_present() {
        let args = json!({"apply": false});
        assert!(!bool_arg(&args, "apply", true));
    }

    #[test]
    fn bool_arg_returns_default_when_missing() {
        let args = json!({});
        assert!(bool_arg(&args, "apply", true));
    }

    #[test]
    fn u32_arg_returns_value() {
        let args = json!({"n": 5});
        assert_eq!(u32_arg(&args, "n", 2), 5);
    }

    #[test]
    fn u32_arg_returns_default_when_missing() {
        let args = json!({});
        assert_eq!(u32_arg(&args, "n", 2), 2);
    }

    #[test]
    fn str_arg_returns_value() {
        let args = json!({"base": "develop"});
        assert_eq!(str_arg(&args, "base", "main"), "develop");
    }

    #[test]
    fn str_arg_returns_default_when_missing() {
        let args = json!({});
        assert_eq!(str_arg(&args, "base", "main"), "main");
    }

    #[test]
    fn opt_str_returns_some_when_present() {
        let args = json!({"message": "WIP: auth"});
        assert_eq!(opt_str(&args, "message"), Some("WIP: auth".to_string()));
    }

    #[test]
    fn opt_str_returns_none_when_missing() {
        let args = json!({});
        assert_eq!(opt_str(&args, "message"), None);
    }

    #[test]
    fn step_deserializes_from_json() {
        let raw = r#"[{"tool":"git_commit","args":{"apply":true},"desc":"Commit staged changes"}]"#;
        let steps: Vec<Step> = serde_json::from_str(raw).unwrap();
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].tool, "git_commit");
        assert_eq!(steps[0].desc, "Commit staged changes");
    }

    #[test]
    fn step_deserializes_with_empty_args() {
        let raw = r#"[{"tool":"git_resolve","args":{},"desc":"Resolve conflicts"}]"#;
        let steps: Vec<Step> = serde_json::from_str(raw).unwrap();
        assert_eq!(steps[0].tool, "git_resolve");
    }

    #[test]
    fn tools_for_returns_correct_namespace() {
        assert!(tools_for("git").contains("git_commit"));
        assert!(tools_for("review").contains("review_diff"));
        assert!(tools_for("deps").contains("deps_audit"));
        assert!(tools_for("release").contains("release_cut"));
    }

    #[test]
    fn all_tools_contains_all_namespaces() {
        let all = all_tools();
        assert!(all.contains("git_commit"));
        assert!(all.contains("review_diff"));
        assert!(all.contains("deps_audit"));
        assert!(all.contains("cloud_up"));
        assert!(all.contains("standup"));
    }

    #[test]
    fn tools_for_unknown_namespace_falls_back_to_top() {
        let t = tools_for("nonexistent");
        assert!(
            t.contains("standup"),
            "unknown namespace should fall back to TOP_TOOLS"
        );
    }

    #[test]
    fn tools_for_env_namespace() {
        assert!(tools_for("env").contains("env_check"));
        assert!(tools_for("env").contains("env_guard"));
    }

    #[test]
    fn tools_for_workflow_namespace() {
        assert!(tools_for("workflow").contains("workflow_run"));
        assert!(tools_for("workflow").contains("workflow_ls"));
    }

    #[test]
    fn tools_for_memory_namespace() {
        assert!(tools_for("memory").contains("memory_note"));
        assert!(tools_for("memory").contains("memory_ls"));
    }

    #[test]
    fn tools_for_mock_namespace() {
        assert!(tools_for("mock").contains("mock_gen"));
        assert!(tools_for("mock").contains("mock_factory"));
    }

    #[test]
    fn tools_for_forensic_namespace() {
        assert!(tools_for("forensic").contains("forensic_explain"));
        assert!(tools_for("forensic").contains("forensic_blame"));
    }

    #[test]
    fn tools_for_cloud_namespace() {
        assert!(tools_for("cloud").contains("cloud_up"));
        assert!(tools_for("cloud").contains("cloud_down"));
    }

    #[test]
    fn all_tools_contains_every_namespace() {
        let all = all_tools();
        for ns in &[
            "env_check",
            "workflow_run",
            "memory_note",
            "mock_gen",
            "forensic_explain",
            "cloud_up",
            "standup",
        ] {
            assert!(all.contains(ns), "all_tools missing: {ns}");
        }
    }

    #[test]
    fn bool_arg_with_non_boolean_json_returns_default() {
        let args = json!({"apply": "yes"});
        assert!(bool_arg(&args, "apply", true));
    }

    #[test]
    fn u32_arg_with_string_value_returns_default() {
        let args = json!({"n": "five"});
        assert_eq!(u32_arg(&args, "n", 3), 3);
    }

    #[test]
    fn u32_arg_with_zero() {
        let args = json!({"n": 0});
        assert_eq!(u32_arg(&args, "n", 2), 0);
    }

    #[test]
    fn str_arg_with_number_falls_back_to_default() {
        let args = json!({"base": 42});
        assert_eq!(str_arg(&args, "base", "main"), "main");
    }

    #[test]
    fn opt_str_with_explicit_null_returns_none() {
        let args = json!({"message": null});
        assert_eq!(opt_str(&args, "message"), None);
    }

    #[test]
    fn step_without_args_field_defaults_to_null() {
        let raw = r#"[{"tool":"git_resolve","desc":"Resolve conflicts"}]"#;
        let steps: Vec<Step> = serde_json::from_str(raw).unwrap();
        assert_eq!(steps[0].tool, "git_resolve");
        assert!(steps[0].args.is_null());
    }

    #[test]
    fn planner_system_prompt_specifies_json_array_format() {
        assert!(PLANNER_SYSTEM.contains("JSON array"));
        assert!(PLANNER_SYSTEM.contains("tool"));
        assert!(PLANNER_SYSTEM.contains("desc"));
    }
}
