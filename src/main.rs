use anyhow::Result;
use clap::{Parser, Subcommand};

mod commands;
mod config;
mod creds;
mod llm;
mod memory;
pub mod nl;
mod usage;
mod utils;

#[derive(Parser)]
#[command(
    name = "dclaw",
    version,
    about = "AI-powered developer daemon — surgical SDLC automation",
    after_help = "\
QUICK START:
  dclaw config set-key --provider deepseek   # store your API key (once)
  cargo build 2>&1 | dclaw doctor            # diagnose a build error
  dclaw git commit                           # AI commit message
  dclaw standup                              # today's standup from git

NATURAL LANGUAGE INTERFACE:
  # Scoped — cheaper (~150 tokens), single domain (recommended):
  dclaw git \"squash last 3 commits and open a PR\"
  dclaw review \"check staged changes for security issues\"
  dclaw deps \"audit and triage anything critical\"
  dclaw release \"draft notes and cut v2.0.0\"

  # Global — more expensive (~600 tokens), for cross-domain workflows:
  dclaw \"review staged changes and if clean commit and push\"
  dclaw \"sync with main, resolve conflicts, then push\"

COMMON WORKFLOWS (structured):
  # Before every push
  dclaw env check && dclaw review diff --staged && dclaw deps audit

  # Release day
  dclaw deps audit --triage
  dclaw release notes
  dclaw release cut v1.2.0

  # Understand old code
  dclaw forensic explain src/legacy.rs
  dclaw forensic blame src/legacy.rs:87

API KEY SETUP (pick any provider):
  export OPENAI_API_KEY=sk-...        # or set via config set-key
  export ANTHROPIC_API_KEY=sk-ant-...
  export DEEPSEEK_API_KEY=sk-...
  dclaw config set-key --provider deepseek   # encrypted local store

Run 'dclaw <COMMAND> --help' for examples specific to each command."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Diagnose terminal errors — pipe your build stderr here
    #[command(after_help = "\
EXAMPLES:
  cargo build 2>&1 | dclaw doctor
  npm run build 2>&1 | dclaw doctor
  cat ci-failure.log | dclaw doctor
  pytest 2>&1 | dclaw doctor")]
    Doctor,

    /// Auto-detect your stack and write a starter .devclawrc
    #[command(after_help = "\
EXAMPLES:
  dclaw init                 # detects Cargo.toml / package.json / go.mod
  cat .devclawrc                # review the generated config")]
    Init,

    /// AI-powered git workflow — commits, PRs, branches, rebases, conflict resolution
    #[command(after_help = "\
SUBCOMMANDS:
  commit       Generate a conventional commit message from staged changes
  pr           Draft a PR description; --create pushes branch + opens PR via gh
  branch       AI branch name from a plain-English description
  push         Smart push — sets upstream, guards secrets, supports --force
  squash       Squash last N commits with an AI-generated message
  resolve      AI-assisted merge conflict resolution
  sync         Fetch origin and rebase onto origin/<base>
  log          AI narrative summary of recent commits
  rebase       AI interactive-rebase plan for the last N commits
  stash        Stash changes with an AI-generated description
  cherry-pick  Cherry-pick a commit; resolves conflicts with AI if needed
  check        Scan staged diff for blocked keywords
  hook         Install dclaw as a git pre-commit hook

EXAMPLES:
  git add -p && dclaw git commit
  dclaw git branch \"add dark mode toggle\" --apply
  dclaw git squash 3 --apply
  dclaw git pr --create
  dclaw git push
  dclaw git resolve
  dclaw git sync
  dclaw git log --since \"2 days ago\"
  dclaw git rebase 5
  dclaw git stash
  dclaw git cherry-pick abc1234")]
    Git {
        #[command(subcommand)]
        action: commands::git_cmd::GitAction,
    },

    /// Explain why code exists using git blame and LLM context analysis
    #[command(after_help = "\
SUBCOMMANDS:
  explain  Explain what a file does using its full git history
  blame    Trace why a specific line exists

EXAMPLES:
  dclaw forensic explain src/auth/token.rs
  dclaw forensic explain src/auth/token.rs --lines 40-80
  dclaw forensic blame src/auth/token.rs")]
    Forensic {
        #[command(subcommand)]
        action: commands::forensic_cmd::ForensicAction,
    },

    /// Generate fixture data and factory functions from a schema
    #[command(after_help = "\
SUBCOMMANDS:
  gen      Generate test records from a schema file
  factory  Generate factory functions for a type

EXAMPLES:
  dclaw mock gen schema.sql --count 20 --format sql
  dclaw mock gen types.ts --format json --out fixtures.json
  dclaw mock factory src/types/user.ts --out src/__tests__/factories.ts")]
    Mock {
        #[command(subcommand)]
        action: commands::mock_cmd::MockAction,
    },

    /// Provision and manage ephemeral cloud VMs
    #[command(after_help = "\
SUBCOMMANDS:
  up    Spin up a new VM
  ls    List all VMs created by dclaw
  ssh   SSH into a VM
  down  Destroy a VM (prompts for confirmation)

EXAMPLES:
  dclaw cloud up --provider aws --size small
  dclaw cloud up --provider gcp --region us-central1-a
  dclaw cloud ls
  dclaw cloud ssh my-vm
  dclaw cloud down my-vm

SUPPORTED PROVIDERS: aws, gcp, azure, fly")]
    Cloud {
        #[command(subcommand)]
        action: commands::cloud_cmd::CloudAction,
    },

    /// Audit dependencies for vulnerabilities and outdated versions
    #[command(after_help = "\
SUBCOMMANDS:
  audit     Run security audits across all detected package managers
  outdated  Check for newer versions of all dependencies

EXAMPLES:
  dclaw deps audit                # cargo audit + npm audit + govulncheck + pip-audit
  dclaw deps audit --triage       # + AI risk ranking: fix now / later / ignore
  dclaw deps outdated

AUTO-DETECTED: Cargo.toml, package.json, go.mod, pyproject.toml, requirements.txt")]
    Deps {
        #[command(subcommand)]
        action: commands::deps_cmd::DepsAction,
    },

    /// AI code review of diffs or GitHub pull requests
    #[command(after_help = "\
SUBCOMMANDS:
  diff  Review uncommitted or staged changes
  pr    Review a GitHub pull request by number (requires gh CLI)

EXAMPLES:
  dclaw review diff                       # all uncommitted changes
  dclaw review diff --staged              # only staged changes
  dclaw review diff --focus security      # OWASP Top 10 lens
  dclaw review diff --focus performance
  dclaw review pr 142
  dclaw review pr 142 --focus style")]
    Review {
        #[command(subcommand)]
        action: commands::review_cmd::ReviewAction,
    },

    /// Draft release notes, write CHANGELOG, and create git tags
    #[command(after_help = "\
SUBCOMMANDS:
  notes  Draft release notes from commits since the last tag
  cut    Write CHANGELOG.md and create an annotated git tag (prompts first)

EXAMPLES:
  dclaw release notes
  dclaw release notes --since v1.1.0
  dclaw release cut v2.0.0")]
    Release {
        #[command(subcommand)]
        action: commands::release_cmd::ReleaseAction,
    },

    /// Generate a daily standup update from your git activity
    #[command(after_help = "\
EXAMPLES:
  dclaw standup
  dclaw standup --since yesterday
  dclaw standup --since \"2 days ago\"
  dclaw standup --since \"last friday\"
  dclaw standup --format slack      # emoji bullets, *bold* headers
  dclaw standup --format markdown")]
    Standup {
        /// How far back to look (e.g. "yesterday", "2 days ago", "last friday")
        #[arg(long, default_value = "yesterday")]
        since: String,
        /// Output format: plain | slack | markdown
        #[arg(long, default_value = "plain")]
        format: String,
    },

    /// Scan .env files and guard against secret leaks in commits
    #[command(after_help = "\
SUBCOMMANDS:
  check  Diff .env against .env.example — report missing or extra keys
  guard  Scan staged changes for secrets about to be committed
  hook   Install pre-commit (guard) and post-checkout (check) git hooks

EXAMPLES:
  dclaw env check
  dclaw env guard
  dclaw env hook")]
    Env {
        #[command(subcommand)]
        action: commands::env_cmd::EnvAction,
    },

    /// Define, run, publish, and import multi-step automation workflows
    #[command(after_help = "\
SUBCOMMANDS:
  ls       List all workflows (project + global)
  run      Execute a workflow step by step
  publish  Publish a workflow as a GitHub Gist
  import   Import a workflow from a Gist URL or raw TOML URL

EXAMPLES:
  dclaw workflow ls
  dclaw workflow run pre-push
  dclaw workflow run pre-push --dry-run
  dclaw workflow publish pre-push
  dclaw workflow import gist:abc123def456")]
    Workflow {
        #[command(subcommand)]
        action: commands::workflow_cmd::WorkflowAction,
    },

    /// Show LLM call quota usage for this month
    #[command(after_help = "\
EXAMPLES:
  dclaw usage")]
    Usage,

    /// View, add, and clear per-project LLM memory
    #[command(after_help = "\
SUBCOMMANDS:
  ls        Show all notes and feedback stored for this project
  note      Add a project note (e.g. stack decisions, conventions)
  feedback  Add feedback that shapes future LLM responses
  clear     Clear memory for this project (or globally with --global)

EXAMPLES:
  dclaw memory ls
  dclaw memory note \"using postgres 16 with pgvector\"
  dclaw memory note \"auth is handled by clerk\" --command review
  dclaw memory feedback \"prefer concise responses\"
  dclaw memory clear
  dclaw memory clear --global")]
    Memory {
        #[command(subcommand)]
        action: commands::memory_cmd::MemoryAction,
    },

    /// Manage API provider keys (stored encrypted at ~/dclaw/creds/)
    #[command(after_help = "\
SUBCOMMANDS:
  set-key   Store an API key in the encrypted credentials store
  list-keys List all providers that have keys stored

EXAMPLES:
  dclaw config set-key --provider deepseek    # prompted for key + passphrase
  dclaw config set-key --provider openai
  dclaw config set-key --provider anthropic
  dclaw config set-key --provider groq
  dclaw config set-key --provider mistral
  dclaw config set-key --provider ollama      # local, no key needed
  dclaw config list-keys

ALTERNATIVE — set via environment variable (no passphrase needed):
  export OPENAI_API_KEY=sk-...
  export ANTHROPIC_API_KEY=sk-ant-...
  export DEEPSEEK_API_KEY=sk-...
  export GROQ_API_KEY=gsk_...
  export MISTRAL_API_KEY=...
  export DEV_CLAW_API_KEY=...    # generic fallback for any provider")]
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },

    /// Global natural language interface — dclaw "<what you want to do>"
    ///
    /// Sends all ~40 tool schemas to the planner (~600 extra tokens).
    /// Prefer scoped NL for single-domain requests — it's cheaper and more accurate:
    ///   dclaw git "..."  |  dclaw review "..."  |  dclaw deps "..."
    #[command(external_subcommand)]
    Do(Vec<String>),
}

#[derive(Subcommand)]
enum ConfigAction {
    /// Store an API key in the encrypted credentials store
    SetKey {
        /// Provider: deepseek | openai | anthropic | groq | mistral | ollama | openrouter
        #[arg(long)]
        provider: String,
    },
    /// List all providers that have credentials stored
    ListKeys,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Doctor => commands::doctor::run().await,
        Command::Init => commands::init::run().await,
        Command::Git { action } => match action {
            commands::git_cmd::GitAction::Nl(args) => nl::run_scoped("git", &args.join(" ")).await,
            other => commands::git_cmd::run(other).await,
        },
        Command::Forensic { action } => match action {
            commands::forensic_cmd::ForensicAction::Nl(args) => {
                nl::run_scoped("forensic", &args.join(" ")).await
            }
            other => commands::forensic_cmd::run(other).await,
        },
        Command::Mock { action } => match action {
            commands::mock_cmd::MockAction::Nl(args) => {
                nl::run_scoped("mock", &args.join(" ")).await
            }
            other => commands::mock_cmd::run(other).await,
        },
        Command::Cloud { action } => match action {
            commands::cloud_cmd::CloudAction::Nl(args) => {
                nl::run_scoped("cloud", &args.join(" ")).await
            }
            other => commands::cloud_cmd::run(other).await,
        },
        Command::Review { action } => match action {
            commands::review_cmd::ReviewAction::Nl(args) => {
                nl::run_scoped("review", &args.join(" ")).await
            }
            other => commands::review_cmd::run(other).await,
        },
        Command::Deps { action } => match action {
            commands::deps_cmd::DepsAction::Nl(args) => {
                nl::run_scoped("deps", &args.join(" ")).await
            }
            other => commands::deps_cmd::run(other).await,
        },
        Command::Release { action } => match action {
            commands::release_cmd::ReleaseAction::Nl(args) => {
                nl::run_scoped("release", &args.join(" ")).await
            }
            other => commands::release_cmd::run(other).await,
        },
        Command::Standup { since, format } => commands::standup_cmd::run(&since, &format).await,
        Command::Env { action } => match action {
            commands::env_cmd::EnvAction::Nl(args) => nl::run_scoped("env", &args.join(" ")).await,
            other => commands::env_cmd::run(other).await,
        },
        Command::Workflow { action } => match action {
            commands::workflow_cmd::WorkflowAction::Nl(args) => {
                nl::run_scoped("workflow", &args.join(" ")).await
            }
            other => commands::workflow_cmd::run(other).await,
        },
        Command::Usage => commands::usage_cmd::run().await,
        Command::Memory { action } => match action {
            commands::memory_cmd::MemoryAction::Nl(args) => {
                nl::run_scoped("memory", &args.join(" ")).await
            }
            other => commands::memory_cmd::run(other).await,
        },
        Command::Config { action } => match action {
            ConfigAction::SetKey { provider } => commands::config_cmd::set_key(&provider).await,
            ConfigAction::ListKeys => commands::config_cmd::list_keys().await,
        },
        Command::Do(args) => nl::run_global(&args.join(" ")).await,
    }
}
