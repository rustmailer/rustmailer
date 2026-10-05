// Copyright © 2025-2026 rustmailer.com
// Licensed under RustMailer License Agreement v1.0
// Unauthorized copying, modification, or distribution is prohibited.

use crate::modules::account::entity::{AuthType, MailerType};
use crate::modules::cache::imap::sync::execute_imap_sync;
use crate::modules::cache::vendor::gmail::sync::execute_gmail_sync;
use crate::modules::cache::vendor::outlook::sync::execute_outlook_sync;
use crate::modules::oauth2::token::OAuth2AccessToken;
use crate::modules::scheduler::periodic::TaskHandle;
use crate::modules::{
    account::{dispatcher::STATUS_DISPATCHER, migration::AccountModel},
    error::RustMailerResult,
    scheduler::periodic::PeriodicTask,
};
use crate::utc_now;
use dashmap::DashMap;
use std::future::Future;
use std::sync::atomic::{AtomicI64, Ordering};
use std::{sync::LazyLock, time::Duration};
use tracing::{error, warn};

static _DESCRIPTION: &str = "This task periodically synchronizes mailbox data for a specified account, ensuring that all local data is up-to-date.";
const TASK_INTERVAL: Duration = Duration::from_secs(10);
/// Upper bound for a single account sync run.
///
/// A sync future that never completes (e.g. one blocked in sync code inside a
/// lock) would otherwise stall the periodic task forever: PeriodicTask awaits
/// the future inline, so the loop simply stops ticking — no error, no abort,
/// while the container still reports healthy. This is exactly the 1.7.2
/// "sync pipeline hangs after local-deletion batch" failure mode. The sync is
/// therefore spawned onto its own task and awaited with a timeout, so a wedged
/// run is abandoned, reported through the status dispatcher, and the next
/// tick still fires. 30 minutes is deliberately generous: an inline
/// `tokio::time::timeout` could never fire against sync-code deadlock anyway,
/// because the polling thread itself is stuck — the timeout must live on a
/// separate task.
const SYNC_TIMEOUT: Duration = Duration::from_secs(1800);
pub static SYNC_TASKS: LazyLock<AccountSyncTask> = LazyLock::new(AccountSyncTask::new);
static LAST_WARN_TIME: AtomicI64 = AtomicI64::new(0);
const WARN_INTERVAL_MS: i64 = 600_000;

/// Run one account sync tick (pre-sync DB reads included) on its own task
/// under a watchdog.
///
/// Success and error reporting (status dispatcher + log text) is identical to
/// the previous inline-await behaviour; the only new case is the timeout arm,
/// which previously surfaced as silence. `handle.abort()` in that arm only
/// takes effect at await points — a run wedged in sync code cannot be killed,
/// but it is now loudly reported instead of freezing the loop forever.
async fn run_sync_with_watchdog<F>(account_id: u64, sync: F)
where
    F: Future<Output = RustMailerResult<()>> + Send + 'static,
{
    let mut handle = tokio::spawn(sync);
    match tokio::time::timeout(SYNC_TIMEOUT, &mut handle).await {
        Ok(Ok(result)) => {
            if let Err(e) = result {
                STATUS_DISPATCHER
                    .append_error(
                        account_id,
                        format!("error in account sync task: {:#?}", e),
                    )
                    .await;
                error!(
                    "Failed to synchronize mailbox data for '{}': {:?}",
                    account_id, e
                )
            }
        }
        Ok(Err(join_err)) => {
            error!(
                "Account sync task for '{}' was cancelled or panicked: {:?}",
                account_id, join_err
            );
        }
        Err(_) => {
            handle.abort();
            let message = format!(
                "account sync task did not finish within {} seconds and was abandoned; if this keeps happening, the sync pipeline is wedged and should be investigated",
                SYNC_TIMEOUT.as_secs()
            );
            STATUS_DISPATCHER
                .append_error(account_id, message.clone())
                .await;
            error!("Account '{}': {}", account_id, message);
        }
    }
}

pub struct AccountSyncTask {
    tasks: DashMap<u64, TaskHandle>,
}

impl AccountSyncTask {
    pub fn new() -> Self {
        Self {
            tasks: DashMap::new(),
        }
    }

    pub async fn start_account_sync_task(&self, account_id: u64, email: String) {
        let task_name = format!("account-sync-task-{}-{}", account_id, &email);
        let periodic_task = PeriodicTask::new(&task_name);
        let task = move |param: Option<u64>| {
            let account_id = param.unwrap();
            Box::pin(async move {
                // The entire tick body — including the pre-sync DB reads — runs
                // under the watchdog. A hang in AccountModel::get or
                // OAuth2AccessToken::get used to stall this loop silently:
                // the watchdog only wrapped execute_*_sync, so a wedged
                // pre-sync await meant the task stopped ticking with no error
                // logged and nothing to restart it (INCIDENT-2718318811333577).
                run_sync_with_watchdog(account_id, async move {
                    let account = AccountModel::get(account_id).await.ok();
                    match account {
                        Some(account) => {
                            if !account.enabled {
                                let last = LAST_WARN_TIME.load(Ordering::Relaxed);
                                let now = utc_now!();
                                if now - last >= WARN_INTERVAL_MS {
                                    LAST_WARN_TIME.store(now, Ordering::Relaxed);
                                    warn!(
                                        "Account {}: Sync aborted. Account is currently disabled.",
                                        account_id
                                    );
                                }
                            } else {
                                match account.mailer_type {
                                    MailerType::ImapSmtp => {
                                        if let AuthType::OAuth2 = account.imap.as_ref().expect("BUG: account.imap is None, but this should never happen here").auth.auth_type {
                                            if OAuth2AccessToken::get(account.id).await?.is_none() {
                                                if utc_now!() % 300_000 == 0 {
                                                    warn!("Account {}: Sync aborted. OAuth2 authorization not completed. Please visit the rustmailer admin page to authorize this account.", account_id);
                                                }
                                                return Ok(());
                                            }
                                        }
                                        let acct = account.clone();
                                        execute_imap_sync(&acct).await?;
                                    }
                                    MailerType::GmailApi => {
                                        if OAuth2AccessToken::get(account.id).await?.is_none() {
                                            if utc_now!() % 300_000 == 0 {
                                                warn!("Account {}: Sync aborted. OAuth2 authorization not completed. Please visit the rustmailer admin page to authorize this account.", account_id);
                                            }
                                            return Ok(());
                                        }
                                        let acct = account.clone();
                                        execute_gmail_sync(&acct).await?;
                                    }
                                    MailerType::GraphApi => {
                                        if OAuth2AccessToken::get(account.id).await?.is_none() {
                                            if utc_now!() % 300_000 == 0 {
                                                warn!("Account {}: Sync aborted. OAuth2 authorization not completed. Please visit the rustmailer admin page to authorize this account.", account_id);
                                            }
                                            return Ok(());
                                        }
                                        let acct = account.clone();
                                        execute_outlook_sync(&acct).await?;
                                    }
                                }
                            }
                        }
                        None => {
                            error!(
                                "Account {}: Sync aborted. Account entity not found.",
                                account_id
                            );
                        }
                    }
                    Ok(())
                })
                .await;
                Ok(())
            })
        };
        let handler = periodic_task.start(task, Some(account_id), TASK_INTERVAL, true, true);
        self.tasks.insert(account_id, handler);
    }

    pub async fn stop(&self, account_id: u64) -> RustMailerResult<()> {
        if let Some((_, handler)) = self.tasks.remove(&account_id) {
            handler.cancel().await;
        } else {
            warn!("No sync task found for account: {}", account_id);
        }
        Ok(())
    }
}
