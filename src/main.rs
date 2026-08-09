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
    name = "dev-claw",
    version,
    about = "AI-powered developer daemon — surgical SDLC automation",
    after_help = "\
QUICK START:
  dev-claw config set-key --provider deepseek   # store your API key (once)
  cargo build 2>&1 | dev-claw doctor            # diagnose a build error
  dev-claw git commit                           # AI commit message
  dev-claw standup                              # today's standup from git

NATURAL LANGUAGE INTERFACE:
  # Scoped — cheaper (~150 tokens), single domain (recommended):
  dev-claw git \"squash last 3 commits and open a PR\"
  dev-claw review \"check staged changes for security issues\"
  dev-claw deps \"audit and triage anything critical\"
  dev-claw release \"draft notes and cut v2.0.0\"

  # Global — more expensive (~600 tokens), for cross-domain workflows:
  dev-claw \"review staged changes and if clean commit and push\"
  dev-claw \"sync with main, resolve conflicts, then push\"

COMMON WORKFLOWS (structured):
  # Before every push
  dev-claw env check && dev-claw review diff --staged && dev-claw deps audit

  # Release day
  dev-claw deps audit --triage
  dev-claw release notes
  dev-claw release cut v1.2.0

  # Understand old code
  dev-claw forensic explain src/legacy.rs
  dev-claw forensic blame src/legacy.rs:87

API KEY SETUP (pick any provider):
  export OPENAI_API_KEY=sk-...        # or set via config set-key
  export ANTHROPIC_API_KEY=sk-ant-...
  export DEEPSEEK_API_KEY=sk-...
  dev-claw config set-key --provider deepseek   # encrypted local store

Run 'dev-claw <COMMAND> --help' for examples specific to each command."
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
  cargo build 2>&1 | dev-claw doctor
  npm run build 2>&1 | dev-claw doctor
  cat ci-failure.log | dev-claw doctor
  pytest 2>&1 | dev-claw doctor")]
    Doctor,

    /// Auto-detect your stack and write a starter .devclawrc
    #[command(after_help = "\
EXAMPLES:
  dev-claw init                 # detects Cargo.toml / package.json / go.mod
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
  hook         Install dev-claw as a git pre-commit hook

EXAMPLES:
  git add -p && dev-claw git commit
  dev-claw git branch \"add dark mode toggle\" --apply
  dev-claw git squash 3 --apply
  dev-claw git pr --create
  dev-claw git push
  dev-claw git resolve
  dev-claw git sync
  dev-claw git log --since \"2 days ago\"
  dev-claw git rebase 5
  dev-claw git stash
  dev-claw git cherry-pick abc1234")]
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
  dev-claw forensic explain src/auth/token.rs
  dev-claw forensic explain src/auth/token.rs --lines 40-80
  dev-claw forensic blame src/auth/token.rs")]
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
  dev-claw mock gen schema.sql --count 20 --format sql
  dev-claw mock gen types.ts --format json --out fixtures.json
  dev-claw mock factory src/types/user.ts --out src/__tests__/factories.ts")]
    Mock {
        #[command(subcommand)]
        action: commands::mock_cmd::MockAction,
    },

    /// Provision and manage ephemeral cloud VMs
    #[command(after_help = "\
SUBCOMMANDS:
  up    Spin up a new VM
  ls    List all VMs created by dev-claw
  ssh   SSH into a VM
  down  Destroy a VM (prompts for confirmation)

EXAMPLES:
  dev-claw cloud up --provider aws --size small
  dev-claw cloud up --provider gcp --region us-central1-a
  dev-claw cloud ls
  dev-claw cloud ssh my-vm
  dev-claw cloud down my-vm

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
  dev-claw deps audit                # cargo audit + npm audit + govulncheck + pip-audit
  dev-claw deps audit --triage       # + AI risk ranking: fix now / later / ignore
  dev-claw deps outdated

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
  dev-claw review diff                       # all uncommitted changes
  dev-claw review diff --staged              # only staged changes
  dev-claw review diff --focus security      # OWASP Top 10 lens
  dev-claw review diff --focus performance
  dev-claw review pr 142
  dev-claw review pr 142 --focus style")]
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
  dev-claw release notes
  dev-claw release notes --since v1.1.0
  dev-claw release cut v2.0.0")]
    Release {
        #[command(subcommand)]
        action: commands::release_cmd::ReleaseAction,
    },

    /// Generate a daily standup update from your git activity
    #[command(after_help = "\
EXAMPLES:
  dev-claw standup
  dev-claw standup --since yesterday
  dev-claw standup --since \"2 days ago\"
  dev-claw standup --since \"last friday\"
  dev-claw standup --format slack      # emoji bullets, *bold* headers
  dev-claw standup --format markdown")]
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
  dev-claw env check
  dev-claw env guard
  dev-claw env hook")]
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
  dev-claw workflow ls
  dev-claw workflow run pre-push
  dev-claw workflow run pre-push --dry-run
  dev-claw workflow publish pre-push
  dev-claw workflow import gist:abc123def456")]
    Workflow {
        #[command(subcommand)]
        action: commands::workflow_cmd::WorkflowAction,
    },

    /// Show LLM call quota usage for this month
    #[command(after_help = "\
EXAMPLES:
  dev-claw usage")]
    Usage,

    /// View, add, and clear per-project LLM memory
    #[command(after_help = "\
SUBCOMMANDS:
  ls        Show all notes and feedback stored for this project
  note      Add a project note (e.g. stack decisions, conventions)
  feedback  Add feedback that shapes future LLM responses
  clear     Clear memory for this project (or globally with --global)

EXAMPLES:
  dev-claw memory ls
  dev-claw memory note \"using postgres 16 with pgvector\"
  dev-claw memory note \"auth is handled by clerk\" --command review
  dev-claw memory feedback \"prefer concise responses\"
  dev-claw memory clear
  dev-claw memory clear --global")]
    Memory {
        #[command(subcommand)]
        action: commands::memory_cmd::MemoryAction,
    },

    /// Manage API provider keys (stored encrypted at ~/dev-claw/creds/)
    #[command(after_help = "\
SUBCOMMANDS:
  set-key   Store an API key in the encrypted credentials store
  list-keys List all providers that have keys stored

EXAMPLES:
  dev-claw config set-key --provider deepseek    # prompted for key + passphrase
  dev-claw config set-key --provider openai
  dev-claw config set-key --provider anthropic
  dev-claw config set-key --provider groq
  dev-claw config set-key --provider mistral
  dev-claw config set-key --provider ollama      # local, no key needed
  dev-claw config list-keys

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

    /// Global natural language interface — dev-claw "<what you want to do>"
    ///
    /// Sends all ~40 tool schemas to the planner (~600 extra tokens).
    /// Prefer scoped NL for single-domain requests — it's cheaper and more accurate:
    ///   dev-claw git "..."  |  dev-claw review "..."  |  dev-claw deps "..."
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
