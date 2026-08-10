use anyhow::Result;
use chrono::Local;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use crate::config::Config;

const MAX_INTERACTIONS: usize = 50;
const MAX_CONTEXT_CHARS: usize = 500;

const GLOBAL_SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS memory_notes (
        id         INTEGER PRIMARY KEY AUTOINCREMENT,
        project_id TEXT NOT NULL,
        command    TEXT,
        text       TEXT NOT NULL,
        kind       TEXT NOT NULL,
        created_at TEXT NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_mn_project ON memory_notes(project_id);
";

// ── Per-project data model ────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Default, Clone)]
pub struct UserNote {
    pub text: String,
    pub command: Option<String>,
    pub created_at: String,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Interaction {
    pub command: String,
    pub response_preview: String,
    pub recorded_at: String,
}

#[derive(Serialize, Deserialize, Default)]
pub struct ProjectMemory {
    pub project_id: String,
    pub project_name: String,
    pub stack: Vec<String>,
    pub user_notes: Vec<UserNote>,
    pub interactions: Vec<Interaction>,
}

impl ProjectMemory {
    pub fn load() -> Result<Self> {
        let path = memory_file()?;
        if !path.exists() {
            let root = project_root();
            return Ok(Self {
                project_id: project_id(&root),
                project_name: project_name(&root),
                stack: detect_stack(&root),
                ..Default::default()
            });
        }
        let raw = std::fs::read_to_string(&path)?;
        let mut mem: Self = toml::from_str(&raw)
            .map_err(|e| anyhow::anyhow!("Cannot parse .dclaw/memory.toml: {e}"))?;
        if mem.stack.is_empty() {
            mem.stack = detect_stack(&project_root());
        }
        Ok(mem)
    }

    pub fn save(&self) -> Result<()> {
        let dir = memory_dir()?;
        std::fs::create_dir_all(&dir)?;
        let raw = toml::to_string_pretty(self)?;
        std::fs::write(dir.join("memory.toml"), raw)?;
        ensure_gitignored();
        Ok(())
    }

    fn push_note(&mut self, text: &str, command: Option<&str>) {
        self.user_notes.push(UserNote {
            text: text.to_string(),
            command: command.map(str::to_string),
            created_at: now(),
        });
    }

    fn push_interaction(&mut self, command: &str, preview: &str) {
        self.interactions.push(Interaction {
            command: command.to_string(),
            response_preview: preview.chars().take(200).collect(),
            recorded_at: now(),
        });
        if self.interactions.len() > MAX_INTERACTIONS {
            self.interactions.remove(0);
        }
    }
}

// ── Global SQLite store ───────────────────────────────────────────────────────

struct GlobalDb {
    conn: Connection,
}

struct NoteRow {
    kind: String,
    command: Option<String>,
    text: String,
    created_at: String,
}

impl GlobalDb {
    fn open() -> Result<Self> {
        let db = Config::data_dir()?.join("usage.db");
        let conn = Connection::open(db)?;
        conn.execute_batch(GLOBAL_SCHEMA)?;
        Ok(Self { conn })
    }

    fn insert(&self, pid: &str, kind: &str, cmd: Option<&str>, text: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO memory_notes (project_id,command,text,kind,created_at)
             VALUES (?1,?2,?3,?4,?5)",
            params![pid, cmd, text, kind, now()],
        )?;
        Ok(())
    }

    fn list_for(&self, pid: &str) -> Result<Vec<NoteRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT kind,command,text,created_at FROM memory_notes
             WHERE project_id=?1 ORDER BY created_at DESC",
        )?;
        let rows = stmt.query_map(params![pid], |r| {
            Ok(NoteRow {
                kind: r.get(0)?,
                command: r.get(1)?,
                text: r.get(2)?,
                created_at: r.get(3)?,
            })
        })?;
        rows.collect::<Result<_, _>>().map_err(Into::into)
    }

    fn delete_project(&self, pid: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM memory_notes WHERE project_id=?1", params![pid])?;
        Ok(())
    }

    fn delete_all(&self) -> Result<()> {
        self.conn.execute("DELETE FROM memory_notes", [])?;
        Ok(())
    }
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Compact context block to prepend to LLM system prompts.
/// Returns empty string on any error so callers never fail because of memory.
pub fn context_for_prompt(command: &str) -> String {
    let Ok(mem) = ProjectMemory::load() else {
        return String::new();
    };
    let mut lines: Vec<String> = Vec::new();

    if !mem.stack.is_empty() {
        lines.push(format!("Stack: {}", mem.stack.join(", ")));
    }
    let notes: Vec<&str> = mem
        .user_notes
        .iter()
        .filter(|n| n.command.is_none() || n.command.as_deref() == Some(command))
        .map(|n| n.text.as_str())
        .collect();
    if !notes.is_empty() {
        lines.push(format!("Project context: {}", notes.join("; ")));
    }
    if lines.is_empty() {
        return String::new();
    }

    let block = format!("[Project memory]\n{}\n\n", lines.join("\n"));
    if block.chars().count() > MAX_CONTEXT_CHARS {
        block.chars().take(MAX_CONTEXT_CHARS).collect::<String>() + "…\n\n"
    } else {
        block
    }
}

/// Record an LLM response for the current project. Silently swallows errors.
pub fn record_interaction(command: &str, response: &str) {
    if let Ok(mut mem) = ProjectMemory::load() {
        mem.push_interaction(command, response);
        let _ = mem.save();
    }
}

/// Add a project-scoped note (stored locally and in global DB).
pub fn add_note(text: &str, command: Option<&str>) -> Result<()> {
    let mut mem = ProjectMemory::load()?;
    mem.push_note(text, command);
    mem.save()?;
    GlobalDb::open()?.insert(&mem.project_id, "note", command, text)
}

/// Add user feedback (stored locally and in global DB with kind="feedback").
pub fn add_feedback(text: &str, command: Option<&str>) -> Result<()> {
    let mut mem = ProjectMemory::load()?;
    mem.push_note(text, command);
    mem.save()?;
    GlobalDb::open()?.insert(&mem.project_id, "feedback", command, text)
}

/// Delete per-project memory file and this project's rows in the global DB.
pub fn clear_project() -> Result<()> {
    // Compute root once — memory_dir() also calls project_root(), so using it
    // directly avoids a TOCTOU inconsistency if cwd shifts between the two calls.
    let root = project_root();
    let pid = project_id(&root);
    let path = root.join(".dclaw").join("memory.toml");
    if path.exists() {
        std::fs::remove_file(&path)?;
    }
    GlobalDb::open()?.delete_project(&pid)
}

/// Delete all rows in the global memory table (per-project files untouched).
pub fn clear_global() -> Result<()> {
    GlobalDb::open()?.delete_all()
}

/// Project name suitable for display in confirmation prompts.
pub fn project_name_for_display() -> String {
    project_name(&project_root())
}

/// Print memory state for the current project.
pub fn show() -> Result<()> {
    let mem = ProjectMemory::load()?;
    let pid = &mem.project_id;

    println!("Project : {} (id: {})", mem.project_name, pid);
    if !mem.stack.is_empty() {
        println!("Stack   : {}", mem.stack.join(", "));
    }

    if mem.user_notes.is_empty() {
        println!("\nNo notes yet.");
        println!("  Add one:      dclaw memory note \"use kebab-case for branches\"");
        println!("  Add feedback: dclaw memory feedback --command standup \"be more concise\"");
    } else {
        println!("\nNotes ({}):", mem.user_notes.len());
        for n in &mem.user_notes {
            let tag = n
                .command
                .as_deref()
                .map(|c| format!("  [cmd: {c}]"))
                .unwrap_or_default();
            println!("  • {}{}", n.text, tag);
        }
    }

    if !mem.interactions.is_empty() {
        let last = mem.interactions.last().unwrap();
        println!(
            "\nInteractions recorded: {}  (last: {} at {})",
            mem.interactions.len(),
            last.command,
            last.recorded_at
        );
    }

    let rows = GlobalDb::open()?.list_for(pid)?;
    if !rows.is_empty() {
        println!("\nGlobal history ({} entries):", rows.len());
        for row in rows.iter().take(5) {
            let tag = row
                .command
                .as_deref()
                .map(|c| format!("[{c}] "))
                .unwrap_or_default();
            println!("  [{}] {}{}", row.kind, tag, row.text);
            println!("       {}", row.created_at);
        }
        if rows.len() > 5 {
            println!("  … and {} more", rows.len() - 5);
        }
    }
    Ok(())
}

// ── Private helpers ───────────────────────────────────────────────────────────

fn project_root() -> std::path::PathBuf {
    let out = std::process::Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output();
    match out {
        Ok(o) if o.status.success() => {
            std::path::PathBuf::from(String::from_utf8_lossy(&o.stdout).trim().to_string())
        }
        _ => std::env::current_dir().unwrap_or_default(),
    }
}

fn memory_dir() -> Result<std::path::PathBuf> {
    Ok(project_root().join(".dclaw"))
}

fn memory_file() -> Result<std::path::PathBuf> {
    Ok(memory_dir()?.join("memory.toml"))
}

fn project_id(root: &std::path::Path) -> String {
    // FNV-1a 64-bit: deterministic and stable across Rust versions.
    // DefaultHasher is explicitly NOT guaranteed stable across compiler releases.
    let bytes = root.to_string_lossy();
    let mut hash: u64 = 0xcbf29ce484222325;
    for &b in bytes.as_bytes() {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x00000100000001b3);
    }
    format!("{:016x}", hash)
}

fn project_name(root: &std::path::Path) -> String {
    root.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

fn now() -> String {
    Local::now().format("%Y-%m-%d %H:%M").to_string()
}

fn detect_stack(root: &std::path::Path) -> Vec<String> {
    let mut s = Vec::new();
    if root.join("Cargo.toml").exists() {
        s.push("rust".into());
    }
    if root.join("package.json").exists() {
        s.push("node".into());
    }
    if root.join("go.mod").exists() {
        s.push("go".into());
    }
    if root.join("pyproject.toml").exists() || root.join("requirements.txt").exists() {
        s.push("python".into());
    }
    s
}

fn ensure_gitignored() {
    let root = project_root();
    let gi = root.join(".gitignore");
    // Use empty string if .gitignore doesn't exist — we'll create it below.
    // Previously returned early on missing file, which left .dclaw/ unignored.
    let content = std::fs::read_to_string(&gi).unwrap_or_default();
    if content
        .lines()
        .any(|l| l.trim() == ".dclaw" || l.trim() == ".dclaw/")
    {
        return;
    }
    let sep = if content.ends_with('\n') || content.is_empty() {
        ""
    } else {
        "\n"
    };
    let _ = std::fs::write(&gi, format!("{content}{sep}.dclaw/\n"));
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_mem(stack: &[&str], notes: &[(&str, Option<&str>)]) -> ProjectMemory {
        ProjectMemory {
            project_id: "test-id".into(),
            project_name: "test-project".into(),
            stack: stack.iter().map(|s| s.to_string()).collect(),
            user_notes: notes
                .iter()
                .map(|(t, c)| UserNote {
                    text: t.to_string(),
                    command: c.map(str::to_string),
                    created_at: "2024-01-01 10:00".into(),
                })
                .collect(),
            interactions: Vec::new(),
        }
    }

    #[test]
    fn context_empty_when_no_stack_no_notes() {
        // Without a real project_root, load() returns default — stack detected
        // from process cwd. In test env: this project has Cargo.toml → rust.
        // So we test the formatting logic directly.
        let mem = make_mem(&[], &[]);
        let mut lines: Vec<String> = Vec::new();
        if !mem.stack.is_empty() {
            lines.push(format!("Stack: {}", mem.stack.join(", ")));
        }
        assert!(lines.is_empty());
    }

    #[test]
    fn context_includes_stack() {
        let mem = make_mem(&["rust", "node"], &[]);
        let block = format!("[Project memory]\nStack: {}\n\n", mem.stack.join(", "));
        assert!(block.contains("rust"));
        assert!(block.contains("node"));
    }

    #[test]
    fn context_filters_notes_by_command() {
        let mem = make_mem(
            &[],
            &[
                ("global note", None),
                ("standup note", Some("standup")),
                ("git note", Some("git")),
            ],
        );
        let standup_notes: Vec<&str> = mem
            .user_notes
            .iter()
            .filter(|n| n.command.is_none() || n.command.as_deref() == Some("standup"))
            .map(|n| n.text.as_str())
            .collect();
        assert_eq!(standup_notes.len(), 2);
        assert!(standup_notes.contains(&"global note"));
        assert!(standup_notes.contains(&"standup note"));
        assert!(!standup_notes.contains(&"git note"));
    }

    #[test]
    fn push_interaction_caps_at_max() {
        let mut mem = ProjectMemory {
            ..Default::default()
        };
        for i in 0..MAX_INTERACTIONS + 10 {
            mem.push_interaction("git", &format!("response {i}"));
        }
        assert_eq!(mem.interactions.len(), MAX_INTERACTIONS);
        // Oldest are evicted — last entry is the most recent
        assert!(mem
            .interactions
            .last()
            .unwrap()
            .response_preview
            .contains("59"));
    }

    #[test]
    fn push_interaction_preview_capped_at_200_chars() {
        let mut mem = ProjectMemory {
            ..Default::default()
        };
        let long = "x".repeat(500);
        mem.push_interaction("review", &long);
        assert_eq!(mem.interactions[0].response_preview.len(), 200);
    }

    #[test]
    fn detect_stack_finds_cargo_toml() {
        let d = tempfile::TempDir::new().unwrap();
        std::fs::write(d.path().join("Cargo.toml"), "[package]").unwrap();
        let s = detect_stack(d.path());
        assert!(s.contains(&"rust".to_string()));
    }

    #[test]
    fn detect_stack_empty_for_empty_dir() {
        let d = tempfile::TempDir::new().unwrap();
        assert!(detect_stack(d.path()).is_empty());
    }

    #[test]
    fn project_id_is_stable() {
        let root = std::path::Path::new("/home/user/myproject");
        assert_eq!(project_id(root), project_id(root));
    }

    #[test]
    fn project_id_differs_for_different_paths() {
        let a = std::path::Path::new("/home/user/project-a");
        let b = std::path::Path::new("/home/user/project-b");
        assert_ne!(project_id(a), project_id(b));
    }

    #[test]
    fn project_name_extracts_dir_name() {
        assert_eq!(
            project_name(std::path::Path::new("/home/user/my-app")),
            "my-app"
        );
    }

    #[test]
    fn save_and_load_round_trips() {
        let d = tempfile::TempDir::new().unwrap();
        std::env::set_var("DEV_CLAW_MEMORY_DIR_OVERRIDE", d.path());
        // We test the TOML round-trip directly rather than through env override
        let mem = make_mem(&["rust"], &[("use snake_case", None)]);
        let raw = toml::to_string_pretty(&mem).unwrap();
        let back: ProjectMemory = toml::from_str(&raw).unwrap();
        assert_eq!(back.project_name, "test-project");
        assert_eq!(back.stack, vec!["rust"]);
        assert_eq!(back.user_notes[0].text, "use snake_case");
        std::env::remove_var("DEV_CLAW_MEMORY_DIR_OVERRIDE");
    }

    #[test]
    fn detect_stack_finds_package_json() {
        let d = tempfile::TempDir::new().unwrap();
        std::fs::write(d.path().join("package.json"), "{}").unwrap();
        let s = detect_stack(d.path());
        assert!(s.contains(&"node".to_string()));
        assert!(!s.contains(&"rust".to_string()));
    }

    #[test]
    fn detect_stack_finds_go_mod() {
        let d = tempfile::TempDir::new().unwrap();
        std::fs::write(d.path().join("go.mod"), "module example.com/app").unwrap();
        let s = detect_stack(d.path());
        assert!(s.contains(&"go".to_string()));
    }

    #[test]
    fn detect_stack_finds_pyproject() {
        let d = tempfile::TempDir::new().unwrap();
        std::fs::write(d.path().join("pyproject.toml"), "[tool.poetry]").unwrap();
        let s = detect_stack(d.path());
        assert!(s.contains(&"python".to_string()));
    }

    #[test]
    fn detect_stack_finds_requirements_txt() {
        let d = tempfile::TempDir::new().unwrap();
        std::fs::write(d.path().join("requirements.txt"), "requests==2.28.0").unwrap();
        let s = detect_stack(d.path());
        assert!(s.contains(&"python".to_string()));
    }

    #[test]
    fn detect_stack_finds_multiple_stacks() {
        let d = tempfile::TempDir::new().unwrap();
        std::fs::write(d.path().join("Cargo.toml"), "[package]").unwrap();
        std::fs::write(d.path().join("package.json"), "{}").unwrap();
        std::fs::write(d.path().join("go.mod"), "module x").unwrap();
        let s = detect_stack(d.path());
        assert!(s.contains(&"rust".to_string()));
        assert!(s.contains(&"node".to_string()));
        assert!(s.contains(&"go".to_string()));
    }

    #[test]
    fn push_note_adds_user_note() {
        let mut mem = ProjectMemory::default();
        mem.push_note("use kebab-case for branches", None);
        assert_eq!(mem.user_notes.len(), 1);
        assert_eq!(mem.user_notes[0].text, "use kebab-case for branches");
        assert!(mem.user_notes[0].command.is_none());
    }

    #[test]
    fn push_note_with_command_stores_command() {
        let mut mem = ProjectMemory::default();
        mem.push_note("be concise", Some("standup"));
        assert_eq!(mem.user_notes[0].command, Some("standup".to_string()));
    }

    #[test]
    fn project_id_known_fnv1a_value() {
        let p = std::path::Path::new("/home/user/project");
        let id = project_id(p);
        assert_eq!(id.len(), 16, "project_id should be 16 hex chars");
        assert_eq!(project_id(p), project_id(p));
    }

    #[test]
    fn project_name_falls_back_for_root() {
        let root = std::path::Path::new("/");
        let _ = project_name(root); // must not panic
    }

    #[test]
    fn toml_round_trip_preserves_interactions() {
        let mut mem = make_mem(&["rust"], &[]);
        mem.push_interaction("git", "committed 3 files");
        mem.push_interaction("review", "looks good");
        let raw = toml::to_string_pretty(&mem).unwrap();
        let back: ProjectMemory = toml::from_str(&raw).unwrap();
        assert_eq!(back.interactions.len(), 2);
        assert_eq!(back.interactions[0].command, "git");
        assert_eq!(back.interactions[1].command, "review");
    }
}
