use anyhow::Result;

use crate::{config::Config, usage::UsageTracker};

pub async fn run() -> Result<()> {
    let cfg = Config::load()?;
    let use_cfg = cfg.usage.as_ref();

    let total_limit = use_cfg.map(|u| u.monthly_limit()).unwrap_or(200);
    let doctor_limit = use_cfg.map(|u| u.doctor_limit()).unwrap_or(75);

    let tracker = UsageTracker::open()?;
    let total = tracker.count_total_this_month()?;
    let doctor = tracker.count_this_month("doctor")?;

    if total_limit == 0 {
        print_unlimited(total, doctor);
    } else {
        print_limited(total, total_limit, doctor, doctor_limit);
    }
    Ok(())
}

fn print_unlimited(total: u32, doctor: u32) {
    println!("Dev-Claw usage (unlimited mode)");
    println!("  Total this month : {total}");
    println!("  doctor           : {doctor}");
}

fn print_limited(total: u32, total_limit: u32, doctor: u32, doctor_limit: u32) {
    let pct = (total * 100) / total_limit.max(1);
    let bar = progress_bar(pct, 20);

    println!("Dev-Claw free-tier usage — this month");
    println!("  Total   [{bar}] {total}/{total_limit} ({pct}%)");
    println!("  doctor  {doctor}/{doctor_limit}");

    if pct >= 80 {
        println!();
        println!("  Approaching limit — upgrade for unlimited:");
        println!("  https://dev-claw.dev/upgrade");
    }
}

fn progress_bar(pct: u32, width: u32) -> String {
    let filled = ((pct * width) / 100).min(width) as usize;
    let empty = (width as usize).saturating_sub(filled);
    format!("{}{}", "█".repeat(filled), "░".repeat(empty))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_bar_empty_at_zero() {
        let bar = progress_bar(0, 20);
        assert_eq!(bar.chars().filter(|&c| c == '░').count(), 20);
        assert_eq!(bar.chars().filter(|&c| c == '█').count(), 0);
    }

    #[test]
    fn progress_bar_full_at_100() {
        let bar = progress_bar(100, 20);
        assert_eq!(bar.chars().filter(|&c| c == '█').count(), 20);
        assert_eq!(bar.chars().filter(|&c| c == '░').count(), 0);
    }

    #[test]
    fn progress_bar_half_at_50() {
        let bar = progress_bar(50, 20);
        assert_eq!(bar.chars().filter(|&c| c == '█').count(), 10);
        assert_eq!(bar.chars().filter(|&c| c == '░').count(), 10);
    }

    #[test]
    fn progress_bar_does_not_overflow_at_110() {
        let bar = progress_bar(110, 20);
        assert_eq!(bar.chars().filter(|&c| c == '█').count(), 20);
    }

    #[test]
    fn progress_bar_width_is_always_correct() {
        for pct in [0, 25, 50, 75, 100] {
            let bar = progress_bar(pct, 20);
            let total = bar.chars().filter(|&c| c == '█' || c == '░').count();
            assert_eq!(total, 20, "bar width wrong at {pct}%");
        }
    }
}
