// Copyright © 2025-2026 rustmailer.com
// Licensed under RustMailer License Agreement v1.0
// Unauthorized copying, modification, or distribution is prohibited.

use crate::modules::{
    account::{entity::MailerType, migration::AccountModel},
    context::{executors::RUST_MAIL_CONTEXT, RustMailTask},
    error::RustMailerResult,
    scheduler::periodic::PeriodicTask,
    settings::cli::SETTINGS,
};
use std::time::Duration;
use tracing::{debug, warn};

const TASK_INTERVAL: Duration = Duration::from_secs(2 * 60);

pub struct ImapHeartBeatTask;

impl RustMailTask for ImapHeartBeatTask {
    fn start() {
        let periodic_task = PeriodicTask::new("imap-heartbeat");

        let task = move |_: Option<u64>| {
            Box::pin(async move {
                if SETTINGS.rustmailer_imap_keepalive_enabled {
                    if let Err(e) = touch_connection().await {
                        warn!("IMAP heartbeat task encountered an error: {:?}", e);
                    }
                }
                Ok(())
            })
        };

        periodic_task.start(task, None, TASK_INTERVAL, false, false);
    }
}

async fn touch_connection() -> RustMailerResult<()> {
    let accounts = AccountModel::list_all().await?;
    let imap_account_ids: Vec<u64> = accounts
        .into_iter()
        .filter(|a| a.enabled && matches!(a.mailer_type, MailerType::ImapSmtp))
        .map(|a| a.id)
        .collect();

    debug!(
        "Starting IMAP heartbeat for {} accounts",
        imap_account_ids.len()
    );

    for account_id in imap_account_ids {
        match RUST_MAIL_CONTEXT.imap(account_id).await {
            Ok(executor) => {
                if let Err(e) = executor.touch_connection().await {
                    warn!(account_id, "Failed to touch IMAP connection: {:?}", e);
                } else {
                    debug!(account_id, "IMAP connection touched successfully");
                }
            }
            Err(e) => {
                warn!(
                    account_id,
                    "Failed to get IMAP executor for heartbeat: {:?}", e
                );
            }
        }
    }

    Ok(())
}
