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
    let cwd = std::env::current_dir()?.canonicalize()?;
    let joined = cwd.join(path);
    let abs = if joined.exists() {
        joined.canonicalize()?
    } else {
        let parent = joined
            .parent()
            .ok_or_else(|| anyhow::anyhow!("Invalid output path: {path}"))?;
        let canonical_parent = parent.canonicalize().unwrap_or_else(|_| cwd.clone());
        canonical_parent.join(joined.file_name().unwrap_or_default())
    };
    if !abs.starts_with(&cwd) {
        anyhow::bail!("Output path `{path}` is outside the current directory — refusing to write.");
    }
    Ok(abs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use tempfile::TempDir;

    // CWD is process-global; serialize all tests that mutate it.
    static CWD_LOCK: Mutex<()> = Mutex::new(());

    fn in_tempdir<F: FnOnce(&std::path::Path)>(f: F) {
        let _guard = CWD_LOCK.lock().unwrap();
        let d = TempDir::new().unwrap();
        let old = std::env::current_dir().unwrap();
        std::env::set_current_dir(d.path()).unwrap();
        f(d.path());
        std::env::set_current_dir(old).unwrap();
    }

    #[test]
    fn safe_output_path_allows_file_in_cwd() {
        in_tempdir(|_| {
            let p = safe_output_path("output.txt").unwrap();
            assert!(p.ends_with("output.txt"));
        });
    }

    #[test]
    fn safe_output_path_allows_nested_subdir() {
        in_tempdir(|base| {
            std::fs::create_dir_all(base.join("a/b")).unwrap();
            let p = safe_output_path("a/b/out.json").unwrap();
            assert!(p.ends_with("out.json"));
        });
    }

    #[test]
    fn safe_output_path_rejects_parent_traversal() {
        in_tempdir(|_| {
            let result = safe_output_path("../outside.txt");
            assert!(result.is_err(), "path traversal must be rejected");
            assert!(result.unwrap_err().to_string().contains("outside"));
        });
    }

    #[test]
    fn safe_output_path_rejects_deep_traversal() {
        in_tempdir(|base| {
            // Create "sub" so the OS can resolve "sub/../.." to cwd's parent
            std::fs::create_dir(base.join("sub")).unwrap();
            let result = safe_output_path("sub/../../outside.txt");
            assert!(result.is_err(), "two-level traversal must be rejected");
        });
    }

    #[test]
    fn safe_output_path_handles_nonexistent_file_in_valid_parent() {
        in_tempdir(|base| {
            std::fs::create_dir_all(base.join("sub")).unwrap();
            let p = safe_output_path("sub/new_file.txt").unwrap();
            assert!(p.ends_with("new_file.txt"));
        });
    }
}
