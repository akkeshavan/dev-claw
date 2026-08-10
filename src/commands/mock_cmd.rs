use anyhow::{bail, Result};
use clap::Subcommand;

use crate::{config::Config, creds, llm, memory, usage::UsageTracker, utils};

// ── Subcommand enum ───────────────────────────────────────────────────────────

#[derive(Subcommand)]
pub enum MockAction {
    /// Generate fixture records from a schema file
    Gen {
        /// Schema file (SQL DDL, TypeScript types, JSON Schema, Protobuf, etc.)
        schema: String,
        /// Number of records to generate
        #[arg(short = 'n', long, default_value = "5")]
        count: u32,
        /// Output format: json | ts | sql | csv
        #[arg(short, long, default_value = "json")]
        format: String,
        /// Write output to a file (default: stdout)
        #[arg(short, long)]
        out: Option<String>,
    },
    /// Generate TypeScript factory functions from a type definition file
    Factory {
        /// TypeScript types file
        types_file: String,
        /// Write output to a file (default: stdout)
        #[arg(short, long)]
        out: Option<String>,
    },
    /// Natural language query — dclaw mock "<what you want to generate>"
    #[command(external_subcommand)]
    Nl(Vec<String>),
}

// ── Constants ─────────────────────────────────────────────────────────────────

const MAX_SCHEMA_CHARS: usize = 8_000;
const VALID_FORMATS: &[&str] = &["json", "ts", "sql", "csv"];

// ── Public entry point ────────────────────────────────────────────────────────

pub async fn run(action: MockAction) -> Result<()> {
    match action {
        MockAction::Gen {
            schema,
            count,
            format,
            out,
        } => gen(&schema, count, &format, out.as_deref()).await,
        MockAction::Factory { types_file, out } => factory(&types_file, out.as_deref()).await,
        MockAction::Nl(_) => unreachable!("Nl handled in main"),
    }
}

// ── Gen subcommand ────────────────────────────────────────────────────────────

async fn gen(schema_file: &str, count: u32, format: &str, out: Option<&str>) -> Result<()> {
    validate_format(format)?;
    let cfg = Config::load()?;
    enforce_quota(&cfg)?;
    let schema = read_schema(schema_file)?;
    let system = gen_system_prompt(format);
    let prompt = build_gen_prompt(&schema, count, format);
    println!("Generating {count} {format} fixture(s) from {schema_file}...\n");
    let raw = call_llm(&system, &prompt).await?;
    write_output(&extract_code_block(&raw), out)
}

// ── Factory subcommand ────────────────────────────────────────────────────────

async fn factory(types_file: &str, out: Option<&str>) -> Result<()> {
    let cfg = Config::load()?;
    enforce_quota(&cfg)?;
    let types = read_schema(types_file)?;
    let system = factory_system_prompt();
    let prompt = build_factory_prompt(&types);
    println!("Generating TypeScript factories from {types_file}...\n");
    let raw = call_llm(system, &prompt).await?;
    write_output(&extract_code_block(&raw), out)
}

// ── I/O helpers ───────────────────────────────────────────────────────────────

fn read_schema(path: &str) -> Result<String> {
    let raw =
        std::fs::read_to_string(path).map_err(|e| anyhow::anyhow!("Cannot read {path}: {e}"))?;
    if raw.trim().is_empty() {
        bail!("{path} is empty");
    }
    Ok(truncate_schema(&raw))
}

fn write_output(content: &str, out: Option<&str>) -> Result<()> {
    let Some(path) = out else {
        println!("{content}");
        return Ok(());
    };
    let abs = utils::safe_output_path(path)?;
    if abs.exists()
        && !utils::confirm(&format!("File `{path}` already exists — overwrite? [y/N] "))?
    {
        println!("Aborted.");
        return Ok(());
    }
    std::fs::write(&abs, content).map_err(|e| anyhow::anyhow!("Cannot write {path}: {e}"))?;
    println!("Written to {path}");
    Ok(())
}

fn truncate_schema(s: &str) -> String {
    if s.chars().count() <= MAX_SCHEMA_CHARS {
        return s.to_string();
    }
    let trimmed: String = s.chars().take(MAX_SCHEMA_CHARS).collect();
    format!("{trimmed}\n-- [schema truncated at {MAX_SCHEMA_CHARS} chars]")
}

// ── Format validation ─────────────────────────────────────────────────────────

fn validate_format(fmt: &str) -> Result<()> {
    if VALID_FORMATS.contains(&fmt) {
        return Ok(());
    }
    bail!(
        "Unknown format `{fmt}`. Valid options: {}",
        VALID_FORMATS.join(", ")
    )
}

// ── Prompt builders ───────────────────────────────────────────────────────────

fn gen_system_prompt(format: &str) -> String {
    let fmt_hint = match format {
        "sql" => "Output valid SQL INSERT INTO statements. Include all columns. One statement per record.",
        "ts"  => "Output a TypeScript const array with properly typed objects. No imports needed.",
        "csv" => "Output CSV with a header row followed by data rows. No extra commentary.",
        _     => "Output a JSON array of objects. Use proper JSON syntax. No extra commentary.",
    };
    format!(
        "You are a test data generator. Given a schema, generate realistic fixture records.\n\
         Use realistic values: real-looking names, valid emails, plausible dates and amounts.\n\
         Never use placeholder text like 'string', 'value', or 'example'.\n\
         {fmt_hint}"
    )
}

fn factory_system_prompt() -> &'static str {
    "You are an expert TypeScript developer. Given TypeScript type definitions, generate \
     factory functions that create test instances with realistic default values. \
     Each factory accepts an optional Partial<T> parameter for overrides and returns T. \
     Export all factories as named exports. Do not import from external packages."
}

fn build_gen_prompt(schema: &str, count: u32, format: &str) -> String {
    format!(
        "Schema:\n```\n{schema}\n```\n\n\
         Generate exactly {count} realistic test record(s) in {format} format."
    )
}

fn build_factory_prompt(types: &str) -> String {
    format!(
        "TypeScript types:\n```typescript\n{types}\n```\n\n\
         Generate TypeScript factory functions for each exported type or interface."
    )
}

// ── Code block extraction ─────────────────────────────────────────────────────

fn extract_code_block(response: &str) -> String {
    let trimmed = response.trim();
    let Some(fence_start) = trimmed.find("```") else {
        return trimmed.to_string();
    };
    let after_fence = &trimmed[fence_start + 3..];
    let content_start = after_fence.find('\n').map(|i| i + 1).unwrap_or(0);
    let content = &after_fence[content_start..];
    if let Some(fence_end) = content.rfind("```") {
        return content[..fence_end].trim().to_string();
    }
    trimmed.to_string()
}

// ── LLM helpers ───────────────────────────────────────────────────────────────

async fn call_llm(system: &str, user: &str) -> Result<String> {
    let ctx = memory::context_for_prompt("mock");
    let provider = resolve_provider()?;
    let api_key = creds::load(&provider)?;
    let result = llm::client_for(&provider, &api_key)
        .complete(&format!("{ctx}{system}"), user)
        .await?;
    memory::record_interaction("mock", &result);
    Ok(result)
}

fn resolve_provider() -> Result<String> {
    if let Ok(p) = std::env::var("DEV_CLAW_PROVIDER") {
        return Ok(p);
    }
    creds::auto_detect_provider().ok_or_else(|| {
        anyhow::anyhow!("No API provider configured. Run: dclaw config set-key --provider deepseek")
    })
}

// ── Quota enforcement ─────────────────────────────────────────────────────────

fn enforce_quota(cfg: &Config) -> Result<()> {
    let tracker = UsageTracker::open()?;
    let limits = cfg.usage.as_ref();
    let monthly = limits.map(|l| l.monthly_limit()).unwrap_or(200);
    let cmd_limit = limits.map(|l| l.mock_limit()).unwrap_or(30);
    let warn_at = limits.map(|l| l.warn_at_percent()).unwrap_or(80);
    if let Some(w) = tracker.check_and_record("mock", cmd_limit, monthly, warn_at)? {
        eprintln!("{w}");
    }
    Ok(())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // --- format validation ---

    #[test]
    fn validate_format_accepts_all_known() {
        for fmt in VALID_FORMATS {
            assert!(validate_format(fmt).is_ok(), "{fmt} should be valid");
        }
    }

    #[test]
    fn validate_format_rejects_unknown() {
        assert!(validate_format("xml").is_err());
        assert!(validate_format("yaml").is_err());
        assert!(validate_format("").is_err());
    }

    // --- prompt building ---

    #[test]
    fn build_gen_prompt_includes_count_format_and_schema() {
        let p = build_gen_prompt("CREATE TABLE users (id INT);", 10, "sql");
        assert!(p.contains("10"));
        assert!(p.contains("sql"));
        assert!(p.contains("CREATE TABLE"));
    }

    #[test]
    fn build_gen_prompt_wraps_schema_in_fence() {
        let p = build_gen_prompt("type User = { id: number }", 3, "json");
        assert!(p.contains("```"));
        assert!(p.contains("User"));
    }

    #[test]
    fn build_factory_prompt_includes_types_and_keyword() {
        let types = "export interface Order { id: number; total: number; }";
        let p = build_factory_prompt(types);
        assert!(p.contains("Order"));
        assert!(p.contains("factory"));
    }

    // --- system prompts ---

    #[test]
    fn gen_system_prompt_sql_mentions_insert() {
        assert!(gen_system_prompt("sql").contains("INSERT"));
    }

    #[test]
    fn gen_system_prompt_csv_mentions_header() {
        assert!(gen_system_prompt("csv").contains("header"));
    }

    #[test]
    fn gen_system_prompt_json_mentions_array() {
        assert!(gen_system_prompt("json").contains("JSON array"));
    }

    #[test]
    fn gen_system_prompt_ts_mentions_typescript() {
        assert!(gen_system_prompt("ts").contains("TypeScript"));
    }

    // --- code block extraction ---

    #[test]
    fn extract_plain_response_unchanged() {
        let r = r#"[{"id": 1, "name": "Alice"}]"#;
        assert_eq!(extract_code_block(r), r);
    }

    #[test]
    fn extract_json_fenced_block() {
        let r = "```json\n[{\"id\": 1}]\n```";
        assert_eq!(extract_code_block(r), r#"[{"id": 1}]"#);
    }

    #[test]
    fn extract_typescript_fenced_block() {
        let r = "```typescript\nconst x = 1;\n```";
        assert_eq!(extract_code_block(r), "const x = 1;");
    }

    #[test]
    fn extract_block_with_leading_preamble() {
        let r = "Here is your data:\n```json\n[]\n```";
        assert_eq!(extract_code_block(r), "[]");
    }

    #[test]
    fn extract_unclosed_fence_falls_back_to_trimmed() {
        let r = "```json\n[{\"id\": 1}]";
        assert_eq!(extract_code_block(r), r.trim());
    }

    // --- schema reading ---

    #[test]
    fn read_schema_reads_file_content() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("schema.sql");
        std::fs::write(&p, "CREATE TABLE t (id INT);").unwrap();
        let content = read_schema(p.to_str().unwrap()).unwrap();
        assert!(content.contains("CREATE TABLE"));
    }

    #[test]
    fn read_schema_errors_on_empty_file() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("empty.sql");
        std::fs::write(&p, "   \n  ").unwrap();
        assert!(read_schema(p.to_str().unwrap()).is_err());
    }

    #[test]
    fn read_schema_errors_on_missing_file() {
        assert!(read_schema("/nonexistent/schema.sql").is_err());
    }

    // --- schema truncation ---

    #[test]
    fn truncate_schema_short_is_unchanged() {
        let s = "short schema";
        assert_eq!(truncate_schema(s), s);
    }

    #[test]
    fn truncate_schema_long_adds_marker() {
        let s = "x".repeat(10_000);
        let out = truncate_schema(&s);
        assert!(out.contains("truncated"));
        assert!(out.chars().count() < 10_000);
    }
}
