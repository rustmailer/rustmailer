// Copyright © 2025-2026 rustmailer.com
// Licensed under RustMailer License Agreement v1.0
// Unauthorized copying, modification, or distribution is prohibited.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tracing::{error, info};

use crate::utc_now;

/// Last time the tokio runtime proved it was alive (milliseconds since UNIX_EPOCH).
/// Updated by the heartbeat task; checked by the watchdog thread.
pub static LAST_HEARTBEAT: AtomicU64 = AtomicU64::new(0);

/// Starts the runtime watchdog.
///
/// A tokio task refreshes `LAST_HEARTBEAT` on an interval. A native OS thread
/// (independent of the tokio runtime) aborts the process with a non-zero exit
/// code when the heartbeat is stale for longer than `timeout_secs`, so that
/// orchestrators (Docker restart policies, supervisors) can recover the
/// process from a wedged runtime. No-op when `timeout_secs` is 0.
pub fn init_watchdog(timeout_secs: u64) {
    if timeout_secs == 0 {
        info!("[watchdog] disabled (timeout=0)");
        return;
    }

    let heartbeat_interval = Duration::from_secs(5);
    let timeout = Duration::from_secs(timeout_secs);

    tokio::spawn(async move {
        let mut tick = tokio::time::interval(heartbeat_interval);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            LAST_HEARTBEAT.store(now_ms(), Ordering::Relaxed);
        }
    });

    std::thread::spawn(move || {
        info!(
            "[watchdog] started, heartbeat interval {:?}, timeout {:?}",
            heartbeat_interval, timeout
        );
        loop {
            std::thread::sleep(heartbeat_interval);
            let last = LAST_HEARTBEAT.load(Ordering::Relaxed);
            if last != 0 && now_ms().saturating_sub(last) > timeout.as_millis() as u64 {
                error!(
                    "[watchdog] runtime heartbeat stalled for {:?}, aborting the process",
                    timeout
                );
                std::process::exit(1);
            }
        }
    });
}

fn now_ms() -> u64 {
    utc_now!() as u64
}
