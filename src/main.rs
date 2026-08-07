use anyhow::Result;
use clap::{Parser, Subcommand};

mod commands;
mod config;
mod creds;
mod llm;
mod usage;
mod utils;

#[derive(Parser)]
#[command(
    name = "dev-claw",
    version,
    about = "AI-powered developer daemon — surgical SDLC automation"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Diagnose terminal errors — pipe your build stderr here
    Doctor,
    /// Auto-detect your stack and write a starter .devclawrc
    Init,
    /// Scan staged diffs, generate commit messages, draft PR descriptions
    Git {
        #[command(subcommand)]
        action: commands::git_cmd::GitAction,
    },
    /// Explain why code exists using git blame and LLM context analysis
    Forensic {
        #[command(subcommand)]
        action: commands::forensic_cmd::ForensicAction,
    },
    /// Generate fixture data from a schema file
    Mock {
        #[command(subcommand)]
        action: commands::mock_cmd::MockAction,
    },
    /// Provision and manage ephemeral cloud VMs
    Cloud {
        #[command(subcommand)]
        action: commands::cloud_cmd::CloudAction,
    },
    /// Audit dependencies for vulnerabilities and outdated versions
    Deps {
        #[command(subcommand)]
        action: commands::deps_cmd::DepsAction,
    },
    /// AI code review of diffs or GitHub pull requests
    Review {
        #[command(subcommand)]
        action: commands::review_cmd::ReviewAction,
    },
    /// Draft release notes, bump version, and create git tags
    Release {
        #[command(subcommand)]
        action: commands::release_cmd::ReleaseAction,
    },
    /// Generate a daily standup update from your git activity
    Standup {
        /// How far back to look (e.g. "yesterday", "2 days ago", "2024-01-15")
        #[arg(long, default_value = "yesterday")]
        since: String,
        /// Output format: plain | slack | markdown
        #[arg(long, default_value = "plain")]
        format: String,
    },
    /// Audit .env against .env.example and guard against secret leaks
    Env {
        #[command(subcommand)]
        action: commands::env_cmd::EnvAction,
    },
    /// Define, run, publish, and import multi-step automation workflows
    Workflow {
        #[command(subcommand)]
        action: commands::workflow_cmd::WorkflowAction,
    },
    /// Show free-tier quota usage for this month
    Usage,
    /// Configure API provider keys and settings
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
}

#[derive(Subcommand)]
enum ConfigAction {
    /// Store an API key in the encrypted credentials store
    SetKey {
        /// Provider: deepseek | openai | claude | sarvam | mistral
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
        Command::Doctor     => commands::doctor::run().await,
        Command::Init       => commands::init::run().await,
        Command::Git { action }     => commands::git_cmd::run(action).await,
        Command::Forensic { action } => commands::forensic_cmd::run(action).await,
        Command::Mock { action }    => commands::mock_cmd::run(action).await,
        Command::Cloud { action }   => commands::cloud_cmd::run(action).await,
        Command::Review { action }  => commands::review_cmd::run(action).await,
        Command::Deps { action }    => commands::deps_cmd::run(action).await,
        Command::Release { action } => commands::release_cmd::run(action).await,
        Command::Standup { since, format } => commands::standup_cmd::run(&since, &format).await,
        Command::Env { action }     => commands::env_cmd::run(action).await,
        Command::Workflow { action } => commands::workflow_cmd::run(action).await,
        Command::Usage => commands::usage_cmd::run().await,
        Command::Config { action } => match action {
            ConfigAction::SetKey { provider } => commands::config_cmd::set_key(&provider).await,
            ConfigAction::ListKeys            => commands::config_cmd::list_keys().await,
        },
    }
}
