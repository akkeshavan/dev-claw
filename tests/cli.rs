//! CLI integration tests — exercises the compiled binary end-to-end.

use assert_cmd::Command;

fn dev_claw() -> Command {
    Command::cargo_bin("dev-claw").expect("binary not found")
}

#[test]
fn help_flag_exits_zero() {
    dev_claw().arg("--help").assert().success();
}

#[test]
fn version_flag_exits_zero() {
    dev_claw().arg("--version").assert().success();
}

#[test]
fn git_help_exits_zero() {
    dev_claw().args(["git", "--help"]).assert().success();
}

#[test]
fn review_help_exits_zero() {
    dev_claw().args(["review", "--help"]).assert().success();
}

#[test]
fn deps_help_exits_zero() {
    dev_claw().args(["deps", "--help"]).assert().success();
}

#[test]
fn config_help_exits_zero() {
    dev_claw().args(["config", "--help"]).assert().success();
}

#[test]
fn release_help_exits_zero() {
    dev_claw().args(["release", "--help"]).assert().success();
}

#[test]
fn env_help_exits_zero() {
    dev_claw().args(["env", "--help"]).assert().success();
}

#[test]
fn memory_help_exits_zero() {
    dev_claw().args(["memory", "--help"]).assert().success();
}

#[test]
fn workflow_help_exits_zero() {
    dev_claw().args(["workflow", "--help"]).assert().success();
}

#[test]
fn cloud_help_exits_zero() {
    dev_claw().args(["cloud", "--help"]).assert().success();
}

#[test]
fn forensic_help_exits_zero() {
    dev_claw().args(["forensic", "--help"]).assert().success();
}

#[test]
fn mock_help_exits_zero() {
    dev_claw().args(["mock", "--help"]).assert().success();
}

#[test]
fn standup_help_exits_zero() {
    dev_claw().args(["standup", "--help"]).assert().success();
}

#[test]
fn usage_help_exits_zero() {
    dev_claw().args(["usage", "--help"]).assert().success();
}

#[test]
fn help_output_mentions_natural_language() {
    let output = dev_claw().arg("--help").output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("NATURAL LANGUAGE") || stdout.contains("natural language"),
        "help should mention the NL interface"
    );
}

#[test]
fn git_help_mentions_all_subcommands() {
    let output = dev_claw().args(["git", "--help"]).output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    for sub in &[
        "commit", "pr", "branch", "push", "squash", "resolve", "sync", "log", "rebase", "stash",
    ] {
        assert!(stdout.contains(sub), "git --help missing subcommand: {sub}");
    }
}

#[test]
fn git_check_exits_zero_with_no_staged_changes() {
    // git check is the only git subcommand that doesn't require an LLM
    dev_claw()
        .args(["git", "check"])
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("HOME", std::env::var("HOME").unwrap_or_default())
        .assert()
        .success();
}
