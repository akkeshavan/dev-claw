use anyhow::Result;
use clap::Subcommand;

use crate::{memory, utils};

// ── Subcommand enum ───────────────────────────────────────────────────────────

#[derive(Subcommand)]
pub enum MemoryAction {
    /// Show stored memory for the current project workspace
    Ls,
    /// Add a persistent learning note for this project
    Note {
        /// The note text (injected into future LLM calls for this project)
        text: String,
        /// Scope this note to one command only (e.g. --command standup)
        #[arg(long)]
        command: Option<String>,
    },
    /// Record feedback about LLM output quality
    Feedback {
        /// What was wrong or what you'd prefer instead
        text: String,
        /// Tag this feedback to a specific command (e.g. --command review)
        #[arg(long)]
        command: Option<String>,
    },
    /// Clear memory
    Clear {
        /// Clear global cross-project memory only (leaves per-project files)
        #[arg(long)]
        global: bool,
        /// Clear both per-project and global memory for this workspace
        #[arg(long)]
        all: bool,
    },
}

// ── Public entry point ────────────────────────────────────────────────────────

pub fn run(action: MemoryAction) -> Result<()> {
    match action {
        MemoryAction::Ls => memory::show(),

        MemoryAction::Note { text, command } => {
            memory::add_note(&text, command.as_deref())?;
            let scope = command.as_deref()
                .map(|c| format!(" (scoped to `{c}`)"))
                .unwrap_or_default();
            println!("✓  Note saved{scope}. Will be injected into future LLM calls.");
            Ok(())
        }

        MemoryAction::Feedback { text, command } => {
            memory::add_feedback(&text, command.as_deref())?;
            let scope = command.as_deref()
                .map(|c| format!(" (tagged to `{c}`)"))
                .unwrap_or_default();
            println!("✓  Feedback saved{scope}.");
            Ok(())
        }

        MemoryAction::Clear { global, all } => clear(global, all),
    }
}

// ── Clear subcommand ──────────────────────────────────────────────────────────

fn clear(global: bool, all: bool) -> Result<()> {
    if all {
        if !utils::confirm("Clear ALL memory (this project + global)? [y/N] ")? {
            println!("Aborted.");
            return Ok(());
        }
        memory::clear_project()?;
        memory::clear_global()?;
        println!("✓  All memory cleared.");
        return Ok(());
    }

    if global {
        if !utils::confirm("Clear all global cross-project memory? [y/N] ")? {
            println!("Aborted.");
            return Ok(());
        }
        memory::clear_global()?;
        println!("✓  Global memory cleared.");
        return Ok(());
    }

    // Default: clear only the current project workspace
    let name = memory::project_name_for_display();
    if !utils::confirm(&format!(
        "Clear memory for project `{name}`? This only affects this workspace. [y/N] "
    ))? {
        println!("Aborted.");
        return Ok(());
    }
    memory::clear_project()?;
    println!("✓  Memory cleared for `{name}`.");
    Ok(())
}
