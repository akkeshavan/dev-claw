use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct WorkflowDef {
    pub name:        String,
    pub description: Option<String>,
    pub steps:       Vec<String>,
}

#[derive(Deserialize, Default, Debug)]
pub struct Config {
    #[allow(dead_code)] // used by future `dev-claw git` / `dev-claw forensic`
    pub stack:     Option<StackConfig>,
    pub doctor:    Option<DoctorConfig>,
    #[allow(dead_code)] // used by future `dev-claw git`
    pub git:       Option<GitConfig>,
    pub usage:     Option<UsageLimits>,
    pub workflows: Option<Vec<WorkflowDef>>,
}

#[allow(dead_code)] // used by future `dev-claw git` / `dev-claw forensic`
#[derive(Deserialize, Debug)]
pub struct StackConfig {
    pub name:            Option<String>,
    pub package_manager: Option<String>,
}

#[derive(Deserialize, Debug)]
pub struct DoctorConfig {
    pub auto_ignore_patterns: Option<Vec<String>>,
    pub max_log_lines:        Option<usize>,
}

#[allow(dead_code)] // used by future `dev-claw git`
#[derive(Deserialize, Debug)]
pub struct GitConfig {
    pub commit_style:      Option<String>,
    pub block_on_keywords: Option<Vec<String>>,
}

#[derive(Deserialize, Debug)]
pub struct UsageLimits {
    pub monthly_limit:              Option<u32>,
    pub doctor_limit:               Option<u32>,
    pub git_limit:                  Option<u32>,
    #[allow(dead_code)] // future `dev-claw env`
    pub env_limit:                  Option<u32>,
    pub forensic_limit:             Option<u32>,
    pub mock_limit:                 Option<u32>,
    pub cloud_limit:                Option<u32>,
    pub warn_at_percent:            Option<u32>,
    #[allow(dead_code)] // future scheduled-reset logic
    pub reset_day:                  Option<u32>,
}

impl UsageLimits {
    pub fn monthly_limit(&self) -> u32   { self.monthly_limit.unwrap_or(200) }
    pub fn doctor_limit(&self) -> u32    { self.doctor_limit.unwrap_or(75) }
    pub fn git_limit(&self) -> u32       { self.git_limit.unwrap_or(60) }
    pub fn forensic_limit(&self) -> u32  { self.forensic_limit.unwrap_or(50) }
    pub fn mock_limit(&self) -> u32      { self.mock_limit.unwrap_or(30) }
    pub fn cloud_limit(&self) -> u32     { self.cloud_limit.unwrap_or(3) }
    pub fn warn_at_percent(&self) -> u32 { self.warn_at_percent.unwrap_or(80) }
}

impl Config {
    /// Searches upward from cwd until it finds a `.devclawrc` file.
    pub fn load() -> Result<Self> {
        let mut dir = std::env::current_dir()?;
        loop {
            let candidate = dir.join(".devclawrc");
            if candidate.exists() {
                let raw = std::fs::read_to_string(&candidate)
                    .with_context(|| format!("Cannot read {}", candidate.display()))?;
                return toml::from_str(&raw).context("Invalid .devclawrc syntax");
            }
            if !dir.pop() {
                break;
            }
        }
        Ok(Config::default())
    }

    /// Returns the platform-appropriate data dir, creating it if needed.
    pub fn data_dir() -> Result<PathBuf> {
        let base = dirs::config_dir()
            .ok_or_else(|| anyhow::anyhow!("Cannot determine config directory"))?;
        let dir = base.join("dev-claw");
        std::fs::create_dir_all(&dir)?;
        Ok(dir)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blank_limits() -> UsageLimits {
        UsageLimits {
            monthly_limit: None, doctor_limit: None, git_limit: None,
            env_limit: None, forensic_limit: None, mock_limit: None,
            cloud_limit: None, warn_at_percent: None, reset_day: None,
        }
    }

    #[test]
    fn usage_limits_defaults() {
        let l = blank_limits();
        assert_eq!(l.monthly_limit(), 200);
        assert_eq!(l.doctor_limit(), 75);
        assert_eq!(l.git_limit(), 60);
        assert_eq!(l.warn_at_percent(), 80);
    }

    #[test]
    fn usage_limits_custom_values() {
        let l = UsageLimits {
            monthly_limit: Some(500), doctor_limit: Some(100),
            git_limit: Some(30), warn_at_percent: Some(90),
            ..blank_limits()
        };
        assert_eq!(l.monthly_limit(), 500);
        assert_eq!(l.doctor_limit(), 100);
        assert_eq!(l.git_limit(), 30);
        assert_eq!(l.warn_at_percent(), 90);
    }

    #[test]
    fn parses_usage_toml() {
        let src = r#"
[usage]
monthly_limit   = 300
doctor_limit    = 100
warn_at_percent = 70
"#;
        let cfg: Config = toml::from_str(src).unwrap();
        let u = cfg.usage.unwrap();
        assert_eq!(u.monthly_limit(), 300);
        assert_eq!(u.doctor_limit(), 100);
        assert_eq!(u.warn_at_percent(), 70);
    }

    #[test]
    fn parses_stack_toml() {
        let src = r#"
[stack]
name            = "typescript-nextjs"
package_manager = "pnpm"
"#;
        let cfg: Config = toml::from_str(src).unwrap();
        let s = cfg.stack.unwrap();
        assert_eq!(s.name.unwrap(), "typescript-nextjs");
        assert_eq!(s.package_manager.unwrap(), "pnpm");
    }

    #[test]
    fn parses_doctor_ignore_patterns() {
        let src = r#"
[doctor]
auto_ignore_patterns = ["node_modules", ".next"]
max_log_lines        = 50
"#;
        let cfg: Config = toml::from_str(src).unwrap();
        let d = cfg.doctor.unwrap();
        assert_eq!(d.max_log_lines.unwrap(), 50);
        let pats = d.auto_ignore_patterns.unwrap();
        assert!(pats.contains(&"node_modules".to_string()));
    }

    #[test]
    fn empty_toml_gives_defaults() {
        let cfg: Config = toml::from_str("").unwrap();
        assert!(cfg.stack.is_none());
        assert!(cfg.usage.is_none());
        assert!(cfg.doctor.is_none());
    }
}
