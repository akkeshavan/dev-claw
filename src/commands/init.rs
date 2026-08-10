use anyhow::{Context, Result};
use std::path::Path;

// ── Coding-style templates ────────────────────────────────────────────────────

const RUST_STYLE: &str = "\
- Use Result<T, E> and ? for error propagation; avoid unwrap() outside tests.
- Prefer owned types in struct fields; use references in function signatures.
- Write unit tests in #[cfg(test)] mod tests at the bottom of each file.
- Keep functions under 40 lines; max 2 levels of nesting.
- Prefer iterators and combinators over explicit loops where readable.";

const GO_STYLE: &str = "\
- Return errors explicitly; never ignore them with _.
- Accept context.Context as the first parameter in all public functions.
- Write table-driven tests using t.Run subtests.
- Avoid global mutable state; inject dependencies via interfaces.
- Use defer for cleanup and handle errors from deferred calls.";

const NEXTJS_STYLE: &str = "\
- Use functional components with explicitly typed props interfaces.
- Do not use 'any'; prefer 'unknown' and narrow with type guards.
- Fetch data in Server Components; use 'use client' only when required.
- Wrap async API calls in try-catch; surface errors via error.tsx boundaries.
- Use Tailwind CSS; avoid inline styles.";

const REACT_STYLE: &str = "\
- Use functional components with explicitly typed props interfaces.
- Do not use 'any'; prefer 'unknown' and narrow the type explicitly.
- Co-locate component tests with the component (ComponentName.test.tsx).
- Use React Query or SWR for server state; useState only for local UI state.
- Wrap async calls in try-catch; propagate errors to an error boundary.";

const VUE_STYLE: &str = "\
- Use the Composition API with <script setup> and typed defineProps.
- Do not use 'any'; declare interface types for all props and emits.
- Keep component logic in composables; co-locate tests with each composable.
- Use Pinia for shared state; avoid direct mutations outside store actions.
- Handle async errors in setup() with try-catch or onErrorCaptured.";

const PYTHON_STYLE: &str = "\
- Add type hints to all function signatures (PEP 484).
- Prefer dataclasses or pydantic models over plain dicts for structured data.
- Use pathlib.Path over os.path.
- Write pytest tests; use fixtures for shared setup; avoid unittest.TestCase.
- Avoid mutable default arguments; use None and assign defaults in the body.";

const GENERIC_STYLE: &str = "\
- Write functions that do one thing; keep them short and flat.
- Validate inputs at system boundaries (user input, external APIs, env vars).
- Prefer explicit error handling over silent failures.
- Co-locate tests with the code they test.";

// ── TOML template ─────────────────────────────────────────────────────────────

const TOML_TEMPLATE: &str = r#"[stack]
name            = "{NAME}"
package_manager = "{PM}"

[standards]
coding_style = """
{STANDARDS}
"""

[git]
commit_style      = "conventional"
block_on_keywords = ["console.log", "debugger", "FIXME", "TODO"]

[doctor]
auto_ignore_patterns = [{IGNORE}]
max_log_lines        = 100

[usage]
monthly_limit   = 200
doctor_limit    = 75
git_limit       = 60
warn_at_percent = 80
reset_day       = 1
"#;

// ── Detected-stack value object ───────────────────────────────────────────────

struct DetectedStack {
    name: String,
    package_manager: String,
    ignore_patterns: Vec<String>,
    coding_style: String,
}

// ── Public entry point ────────────────────────────────────────────────────────

pub async fn run() -> Result<()> {
    let dir = std::env::current_dir()?;
    let output = dir.join(".devclawrc");

    if output.exists() {
        confirm_overwrite()?;
    }

    let stack = detect(&dir);
    let toml = generate_toml(&stack);

    std::fs::write(&output, &toml).with_context(|| format!("Cannot write {}", output.display()))?;

    println!("✓  Detected : {} ({})", stack.name, stack.package_manager);
    println!("✓  Created  : .devclawrc");
    println!();
    println!("   Edit .devclawrc to tune coding standards and quota limits.");
    println!("   Next: dclaw config set-key --provider deepseek");
    Ok(())
}

// ── Interactive helpers ───────────────────────────────────────────────────────

fn confirm_overwrite() -> Result<()> {
    print!(".devclawrc already exists — overwrite? [y/N] ");
    std::io::Write::flush(&mut std::io::stdout())?;
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    if input.trim().eq_ignore_ascii_case("y") {
        return Ok(());
    }
    anyhow::bail!("Aborted — existing .devclawrc kept.")
}

// ── Stack detection ───────────────────────────────────────────────────────────

fn detect(dir: &Path) -> DetectedStack {
    if dir.join("Cargo.toml").exists() {
        return rust_stack();
    }
    if dir.join("go.mod").exists() {
        return go_stack();
    }
    if dir.join("package.json").exists() {
        return detect_node(dir);
    }
    if dir.join("pyproject.toml").exists() || dir.join("requirements.txt").exists() {
        return python_stack();
    }
    generic_stack()
}

fn detect_node(dir: &Path) -> DetectedStack {
    let pm = detect_package_manager(dir);
    let pkg = read_package_json(dir);
    let framework = pkg.as_ref().map(detect_framework).unwrap_or("node");
    node_variant(framework, pm)
}

fn detect_package_manager(dir: &Path) -> &'static str {
    if dir.join("pnpm-lock.yaml").exists() {
        return "pnpm";
    }
    if dir.join("yarn.lock").exists() {
        return "yarn";
    }
    if dir.join("bun.lockb").exists() {
        return "bun";
    }
    "npm"
}

fn read_package_json(dir: &Path) -> Option<serde_json::Value> {
    let raw = std::fs::read_to_string(dir.join("package.json")).ok()?;
    serde_json::from_str(&raw).ok()
}

fn detect_framework(pkg: &serde_json::Value) -> &'static str {
    let deps = merged_deps(pkg);
    if deps.contains_key("next") {
        return "nextjs";
    }
    if deps.contains_key("react") {
        return "react";
    }
    if deps.contains_key("vue") {
        return "vue";
    }
    "node"
}

fn merged_deps(pkg: &serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
    let mut all = serde_json::Map::new();
    for key in ["dependencies", "devDependencies"] {
        if let Some(obj) = pkg.get(key).and_then(|v| v.as_object()) {
            all.extend(obj.clone());
        }
    }
    all
}

fn node_variant(framework: &str, pm: &str) -> DetectedStack {
    match framework {
        "nextjs" => nextjs_stack(pm),
        "react" => react_stack(pm),
        "vue" => vue_stack(pm),
        _ => generic_node_stack(pm),
    }
}

// ── Stack constructors ────────────────────────────────────────────────────────

fn rust_stack() -> DetectedStack {
    DetectedStack {
        name: "rust".into(),
        package_manager: "cargo".into(),
        ignore_patterns: vec!["target/".into()],
        coding_style: RUST_STYLE.into(),
    }
}

fn go_stack() -> DetectedStack {
    DetectedStack {
        name: "go".into(),
        package_manager: "go".into(),
        ignore_patterns: vec!["vendor/".into()],
        coding_style: GO_STYLE.into(),
    }
}

fn python_stack() -> DetectedStack {
    DetectedStack {
        name: "python".into(),
        package_manager: "pip".into(),
        ignore_patterns: vec!["__pycache__".into(), ".venv".into(), "dist/".into()],
        coding_style: PYTHON_STYLE.into(),
    }
}

fn generic_stack() -> DetectedStack {
    DetectedStack {
        name: "generic".into(),
        package_manager: "unknown".into(),
        ignore_patterns: vec![],
        coding_style: GENERIC_STYLE.into(),
    }
}

fn nextjs_stack(pm: &str) -> DetectedStack {
    DetectedStack {
        name: "typescript-nextjs".into(),
        package_manager: pm.into(),
        ignore_patterns: vec!["node_modules".into(), ".next".into(), "dist".into()],
        coding_style: NEXTJS_STYLE.into(),
    }
}

fn react_stack(pm: &str) -> DetectedStack {
    DetectedStack {
        name: "typescript-react".into(),
        package_manager: pm.into(),
        ignore_patterns: vec!["node_modules".into(), "dist".into(), "build".into()],
        coding_style: REACT_STYLE.into(),
    }
}

fn vue_stack(pm: &str) -> DetectedStack {
    DetectedStack {
        name: "typescript-vue".into(),
        package_manager: pm.into(),
        ignore_patterns: vec!["node_modules".into(), "dist".into(), ".nuxt".into()],
        coding_style: VUE_STYLE.into(),
    }
}

fn generic_node_stack(pm: &str) -> DetectedStack {
    DetectedStack {
        name: "node".into(),
        package_manager: pm.into(),
        ignore_patterns: vec!["node_modules".into(), "dist".into()],
        coding_style: GENERIC_STYLE.into(),
    }
}

// ── TOML generation ───────────────────────────────────────────────────────────

fn generate_toml(stack: &DetectedStack) -> String {
    let ignore = format_ignore_patterns(&stack.ignore_patterns);
    TOML_TEMPLATE
        .replace("{NAME}", &stack.name)
        .replace("{PM}", &stack.package_manager)
        .replace("{STANDARDS}", &stack.coding_style)
        .replace("{IGNORE}", &ignore)
}

fn format_ignore_patterns(patterns: &[String]) -> String {
    patterns
        .iter()
        .map(|p| format!(r#""{p}""#))
        .collect::<Vec<_>>()
        .join(", ")
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn tmp() -> TempDir {
        tempfile::tempdir().expect("temp dir")
    }

    // --- Stack detection ---

    #[test]
    fn detects_rust_from_cargo_toml() {
        let d = tmp();
        fs::write(d.path().join("Cargo.toml"), "[package]\nname=\"t\"").unwrap();
        let s = detect(d.path());
        assert_eq!(s.name, "rust");
        assert_eq!(s.package_manager, "cargo");
    }

    #[test]
    fn detects_go_from_go_mod() {
        let d = tmp();
        fs::write(d.path().join("go.mod"), "module example.com/t").unwrap();
        assert_eq!(detect(d.path()).name, "go");
    }

    #[test]
    fn detects_python_from_requirements_txt() {
        let d = tmp();
        fs::write(d.path().join("requirements.txt"), "flask==3.0").unwrap();
        assert_eq!(detect(d.path()).name, "python");
    }

    #[test]
    fn detects_python_from_pyproject_toml() {
        let d = tmp();
        fs::write(d.path().join("pyproject.toml"), "[project]\nname=\"t\"").unwrap();
        assert_eq!(detect(d.path()).name, "python");
    }

    #[test]
    fn detects_nextjs_from_dependencies() {
        let d = tmp();
        fs::write(
            d.path().join("package.json"),
            r#"{"dependencies":{"next":"14","react":"18"}}"#,
        )
        .unwrap();
        assert_eq!(detect(d.path()).name, "typescript-nextjs");
    }

    #[test]
    fn detects_nextjs_from_dev_dependencies() {
        let d = tmp();
        fs::write(
            d.path().join("package.json"),
            r#"{"devDependencies":{"next":"14"}}"#,
        )
        .unwrap();
        assert_eq!(detect(d.path()).name, "typescript-nextjs");
    }

    #[test]
    fn detects_react_without_next() {
        let d = tmp();
        fs::write(
            d.path().join("package.json"),
            r#"{"dependencies":{"react":"18","react-dom":"18"}}"#,
        )
        .unwrap();
        assert_eq!(detect(d.path()).name, "typescript-react");
    }

    #[test]
    fn detects_vue() {
        let d = tmp();
        fs::write(
            d.path().join("package.json"),
            r#"{"dependencies":{"vue":"3"}}"#,
        )
        .unwrap();
        assert_eq!(detect(d.path()).name, "typescript-vue");
    }

    #[test]
    fn falls_back_to_generic_node_with_no_known_framework() {
        let d = tmp();
        fs::write(d.path().join("package.json"), r#"{"dependencies":{}}"#).unwrap();
        assert_eq!(detect(d.path()).name, "node");
    }

    #[test]
    fn falls_back_to_generic_with_no_indicators() {
        let d = tmp();
        assert_eq!(detect(d.path()).name, "generic");
    }

    // --- Package manager detection ---

    #[test]
    fn prefers_pnpm_over_yarn() {
        let d = tmp();
        fs::write(d.path().join("package.json"), "{}").unwrap();
        fs::write(d.path().join("pnpm-lock.yaml"), "").unwrap();
        fs::write(d.path().join("yarn.lock"), "").unwrap();
        assert_eq!(detect(d.path()).package_manager, "pnpm");
    }

    #[test]
    fn detects_yarn_lock() {
        let d = tmp();
        fs::write(d.path().join("package.json"), "{}").unwrap();
        fs::write(d.path().join("yarn.lock"), "").unwrap();
        assert_eq!(detect(d.path()).package_manager, "yarn");
    }

    #[test]
    fn detects_bun_lockb() {
        let d = tmp();
        fs::write(d.path().join("package.json"), "{}").unwrap();
        fs::write(d.path().join("bun.lockb"), "").unwrap();
        assert_eq!(detect(d.path()).package_manager, "bun");
    }

    #[test]
    fn falls_back_to_npm() {
        let d = tmp();
        fs::write(d.path().join("package.json"), "{}").unwrap();
        assert_eq!(detect(d.path()).package_manager, "npm");
    }

    // --- Priority ---

    #[test]
    fn cargo_toml_wins_over_package_json() {
        let d = tmp();
        fs::write(d.path().join("Cargo.toml"), "[package]\nname=\"t\"").unwrap();
        fs::write(
            d.path().join("package.json"),
            r#"{"dependencies":{"react":"18"}}"#,
        )
        .unwrap();
        assert_eq!(detect(d.path()).name, "rust");
    }

    // --- TOML generation ---

    #[test]
    fn generated_toml_is_valid() {
        let toml_str = generate_toml(&rust_stack());
        let parsed: toml::Value = toml::from_str(&toml_str).expect("invalid TOML generated");
        assert_eq!(parsed["stack"]["name"].as_str().unwrap(), "rust");
        assert_eq!(
            parsed["stack"]["package_manager"].as_str().unwrap(),
            "cargo"
        );
    }

    #[test]
    fn generated_toml_includes_ignore_patterns() {
        let toml_str = generate_toml(&nextjs_stack("pnpm"));
        assert!(toml_str.contains("node_modules"));
        assert!(toml_str.contains(".next"));
    }

    #[test]
    fn generated_toml_includes_usage_section() {
        let toml_str = generate_toml(&rust_stack());
        let parsed: toml::Value = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed["usage"]["monthly_limit"].as_integer().unwrap(), 200);
        assert_eq!(parsed["usage"]["git_limit"].as_integer().unwrap(), 60);
    }

    #[test]
    fn format_ignore_patterns_empty_list() {
        assert_eq!(format_ignore_patterns(&[]), "");
    }

    #[test]
    fn format_ignore_patterns_single() {
        let out = format_ignore_patterns(&["target/".to_string()]);
        assert_eq!(out, r#""target/""#);
    }

    #[test]
    fn format_ignore_patterns_multiple() {
        let pats = vec!["a".to_string(), "b".to_string()];
        assert_eq!(format_ignore_patterns(&pats), r#""a", "b""#);
    }
}
