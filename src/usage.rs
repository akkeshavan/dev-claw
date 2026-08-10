use anyhow::Result;
use chrono::{Datelike, Local};
use rusqlite::{params, Connection};

use crate::config::Config;

const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS usage (
        id       INTEGER PRIMARY KEY AUTOINCREMENT,
        command  TEXT NOT NULL,
        used_at  TEXT NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_used_at ON usage(used_at);
";

pub struct UsageTracker {
    conn: Connection,
}

impl UsageTracker {
    pub fn open() -> Result<Self> {
        let db = Config::data_dir()?.join("usage.db");
        Self::from_conn(Connection::open(db)?)
    }

    #[cfg(test)]
    pub fn in_memory() -> Result<Self> {
        Self::from_conn(Connection::open_in_memory()?)
    }

    fn from_conn(conn: Connection) -> Result<Self> {
        conn.execute_batch(SCHEMA)?;
        Ok(Self { conn })
    }

    fn month_prefix() -> String {
        let now = Local::now();
        format!("{}-{:02}", now.year(), now.month())
    }

    pub fn count_this_month(&self, command: &str) -> Result<u32> {
        let prefix = format!("{}%", Self::month_prefix());
        self.conn
            .query_row(
                "SELECT COUNT(*) FROM usage WHERE command = ?1 AND used_at LIKE ?2",
                params![command, prefix],
                |row| row.get(0),
            )
            .map_err(Into::into)
    }

    pub fn count_total_this_month(&self) -> Result<u32> {
        let prefix = format!("{}%", Self::month_prefix());
        self.conn
            .query_row(
                "SELECT COUNT(*) FROM usage WHERE used_at LIKE ?1",
                params![prefix],
                |row| row.get(0),
            )
            .map_err(Into::into)
    }

    fn record(&self, command: &str) -> Result<()> {
        let now = Local::now().to_rfc3339();
        self.conn.execute(
            "INSERT INTO usage (command, used_at) VALUES (?1, ?2)",
            params![command, now],
        )?;
        Ok(())
    }

    fn check_total_quota(total: u32, limit: u32) -> Result<()> {
        if limit > 0 && total >= limit {
            anyhow::bail!(
                "Monthly quota exhausted ({total}/{limit} invocations).\n\
                 Resets on the 1st. Upgrade: https://dev-claw.dev/upgrade"
            );
        }
        Ok(())
    }

    fn check_command_quota(used: u32, limit: u32, command: &str) -> Result<()> {
        if limit > 0 && used >= limit {
            anyhow::bail!(
                "`{command}` limit reached ({used}/{limit} this month).\n\
                 Upgrade: https://dev-claw.dev/upgrade"
            );
        }
        Ok(())
    }

    fn warn_message(new_total: u32, total_limit: u32, warn_pct: u32) -> Option<String> {
        if total_limit == 0 {
            return None;
        }
        let threshold = (total_limit * warn_pct) / 100;
        (new_total >= threshold).then(|| {
            format!(
                "{new_total}/{total_limit} free-tier invocations used  \
                 https://dev-claw.dev/upgrade"
            )
        })
    }

    pub fn check_and_record(
        &self,
        command: &str,
        command_limit: u32,
        total_limit: u32,
        warn_at_percent: u32,
    ) -> Result<Option<String>> {
        let total = self.count_total_this_month()?;
        let cmd_used = self.count_this_month(command)?;

        Self::check_total_quota(total, total_limit)?;
        Self::check_command_quota(cmd_used, command_limit, command)?;
        self.record(command)?;

        Ok(Self::warn_message(total + 1, total_limit, warn_at_percent))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tracker() -> UsageTracker {
        UsageTracker::in_memory().expect("in-memory DB")
    }

    #[test]
    fn records_and_counts() {
        let t = tracker();
        assert_eq!(t.count_total_this_month().unwrap(), 0);
        t.check_and_record("doctor", 75, 200, 80).unwrap();
        assert_eq!(t.count_total_this_month().unwrap(), 1);
        assert_eq!(t.count_this_month("doctor").unwrap(), 1);
        assert_eq!(t.count_this_month("git").unwrap(), 0);
    }

    #[test]
    fn total_limit_blocks_at_cap() {
        let t = tracker();
        for _ in 0..5 {
            t.check_and_record("doctor", 0, 5, 80).unwrap();
        }
        let err = t.check_and_record("doctor", 0, 5, 80).unwrap_err();
        assert!(err.to_string().contains("exhausted"));
    }

    #[test]
    fn command_limit_blocks_at_cap() {
        let t = tracker();
        for _ in 0..3 {
            t.check_and_record("doctor", 3, 100, 80).unwrap();
        }
        let err = t.check_and_record("doctor", 3, 100, 80).unwrap_err();
        assert!(err.to_string().contains("limit reached"));
    }

    #[test]
    fn warn_returned_when_approaching_limit() {
        let t = tracker();
        // limit=5, warn_at=80% → threshold=4; 4th call triggers warning
        for _ in 0..3 {
            assert!(t.check_and_record("doctor", 0, 5, 80).unwrap().is_none());
        }
        let warn = t.check_and_record("doctor", 0, 5, 80).unwrap();
        assert!(warn.is_some());
        assert!(warn.unwrap().contains("4/5"));
    }

    #[test]
    fn unlimited_mode_never_blocks() {
        let t = tracker();
        for _ in 0..500 {
            t.check_and_record("doctor", 0, 0, 80).unwrap();
        }
        assert_eq!(t.count_total_this_month().unwrap(), 500);
    }

    #[test]
    fn warn_message_none_when_unlimited() {
        assert!(UsageTracker::warn_message(9999, 0, 80).is_none());
    }

    #[test]
    fn warn_message_contains_counts() {
        let w = UsageTracker::warn_message(80, 100, 80).unwrap();
        assert!(w.contains("80/100"));
    }

    #[test]
    fn per_command_counts_are_independent() {
        let t = tracker();
        t.check_and_record("doctor", 0, 0, 80).unwrap();
        t.check_and_record("doctor", 0, 0, 80).unwrap();
        t.check_and_record("git", 0, 0, 80).unwrap();
        assert_eq!(t.count_this_month("doctor").unwrap(), 2);
        assert_eq!(t.count_this_month("git").unwrap(), 1);
        assert_eq!(t.count_total_this_month().unwrap(), 3);
    }

    #[test]
    fn check_and_record_returns_none_below_threshold() {
        let t = tracker();
        // limit=10, warn_at=80% → threshold=8; first 7 calls return None
        for _ in 0..7 {
            let w = t.check_and_record("git", 0, 10, 80).unwrap();
            assert!(w.is_none(), "no warning expected below threshold");
        }
    }

    #[test]
    fn blocked_call_does_not_record() {
        let t = tracker();
        for _ in 0..3 {
            t.check_and_record("git", 0, 3, 80).unwrap();
        }
        // This call should fail and NOT increment the count
        let _ = t.check_and_record("git", 0, 3, 80);
        assert_eq!(t.count_total_this_month().unwrap(), 3);
    }

    #[test]
    fn zero_command_limit_means_unlimited_per_command() {
        let t = tracker();
        for _ in 0..100 {
            t.check_and_record("doctor", 0, 0, 80).unwrap();
        }
        assert_eq!(t.count_this_month("doctor").unwrap(), 100);
    }

    #[test]
    fn warn_at_100_percent_triggers_only_at_cap() {
        let t = tracker();
        // limit=5, warn_at=100% → threshold=5; first 4 calls produce no warn
        for _ in 0..4 {
            assert!(t.check_and_record("git", 0, 5, 100).unwrap().is_none());
        }
        let warn = t.check_and_record("git", 0, 5, 100).unwrap();
        assert!(warn.is_some());
    }
}
