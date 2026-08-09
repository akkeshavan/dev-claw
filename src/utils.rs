use anyhow::Result;
use std::io::{self, Write};

/// Print `prompt` and return true iff the user types "y" or "Y".
pub fn confirm(prompt: &str) -> Result<bool> {
    print!("{prompt}");
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    Ok(matches!(input.trim(), "y" | "Y"))
}

/// Resolve `path` relative to cwd and verify it stays inside cwd.
/// Works for both existing and not-yet-created files (canonicalizes the
/// parent dir when the file itself doesn't exist yet).
pub fn safe_output_path(path: &str) -> Result<std::path::PathBuf> {
    // Canonicalize cwd so that symlinked directories (e.g. /Users → /private/Users
    // on macOS) resolve to the same physical root as canonicalize() below.
    let cwd    = std::env::current_dir()?.canonicalize()?;
    let joined = cwd.join(path);
    let abs    = if joined.exists() {
        joined.canonicalize()?
    } else {
        let parent = joined
            .parent()
            .ok_or_else(|| anyhow::anyhow!("Invalid output path: {path}"))?;
        let canonical_parent = parent.canonicalize().unwrap_or_else(|_| cwd.clone());
        canonical_parent.join(joined.file_name().unwrap_or_default())
    };
    if !abs.starts_with(&cwd) {
        anyhow::bail!(
            "Output path `{path}` is outside the current directory — refusing to write."
        );
    }
    Ok(abs)
}
