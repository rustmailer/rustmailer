// Copyright © 2025-2026 rustmailer.com
// Licensed under RustMailer License Agreement v1.0
// Unauthorized copying, modification, or distribution is prohibited.

use crate::modules::{
    account::{
        dispatcher::STATUS_DISPATCHER, entity::MailerType, migration::AccountModel,
        status::AccountRunningState,
    },
    cache::{imap::{mailbox::MailBox, manager::EnvelopeFlagsManager}, sync_type::{determine_sync_type, DEFAULT_FULL_SYNC_INTERVAL_MIN, SyncType}},
    error::RustMailerResult,
    hook::{
        channel::{Event, EVENT_CHANNEL},
        events::{payload::AccountChange, EventPayload, EventType, RustMailerEvent},
        task::EventHookTask,
    },
};
use crate::utc_now;
use dashmap::DashMap;
use flow::reconcile_mailboxes;
use rebuild::{rebuild_cache, rebuild_cache_since_date, should_rebuild_cache};
use std::{
    sync::atomic::{AtomicUsize, Ordering},
    sync::LazyLock,
    time::Instant,
};
use sync_folders::get_sync_folders;
use tracing::{debug, info, warn};

pub mod flow;
pub mod rebuild;
pub mod sync_folders;

static SYNC_COUNTER: AtomicUsize = AtomicUsize::new(0);
/// Per-account timestamp (ms) of the last staleness report, used to rate-limit
/// [`report_sync_staleness`].
static LAST_STALENESS_REPORT: LazyLock<DashMap<u64, i64>> = LazyLock::new(DashMap::new);

/// Report (at most once per stale window per account) when no sync of any kind
/// has completed for far longer than the account's configured intervals.
///
/// The per-run watchdog in `task.rs` only covers a single sync run. It cannot
/// see "the task ticks but no run ever records a completion" — e.g. a code
/// path that returns without setting a sync-end timestamp, which previously
/// surfaced as hours of silent account dormancy with green health checks
/// (INCIDENT-2718318811333577).
async fn report_sync_staleness(account: &AccountModel) -> RustMailerResult<()> {
    let Some(state) = AccountRunningState::get(account.id).await? else {
        return Ok(());
    };
    // Before the first sync completes there is no baseline to compare against.
    if !state.is_initial_sync_completed {
        return Ok(());
    }
    let last_completed = [
        state.last_full_sync_end,
        state.last_incremental_sync_end,
        state.initial_sync_end_time,
    ]
    .into_iter()
    .flatten()
    .max();
    let Some(last_completed) = last_completed else {
        return Ok(());
    };
    let now = utc_now!();
    let full_sync_ms = account
        .full_sync_interval_min
        .unwrap_or(DEFAULT_FULL_SYNC_INTERVAL_MIN)
        .saturating_mul(60 * 1000);
    let incremental_sync_ms = account.incremental_sync_interval_sec.saturating_mul(1000);
    // Allow one full-sync interval of overrun; never check faster than every
    // 10 minutes.
    let threshold_ms = (full_sync_ms * 2)
        .max(incremental_sync_ms * 10)
        .max(600_000);
    if now - last_completed <= threshold_ms {
        return Ok(());
    }
    let last_report = LAST_STALENESS_REPORT
        .get(&account.id)
        .map(|entry| *entry.value())
        .unwrap_or(0);
    if now - last_report <= threshold_ms {
        return Ok(());
    }
    LAST_STALENESS_REPORT.insert(account.id, now);
    let message = format!(
        "account sync staleness: no sync has completed for {} minutes (threshold is {} minutes); \
        the account appears dormant although its sync task is still scheduled",
        (now - last_completed) / 60_000,
        threshold_ms / 60_000
    );
    warn!("Account{{{}}}: {}", &account.email, &message);
    STATUS_DISPATCHER.append_error(account.id, message).await;
    Ok(())
}

pub async fn execute_imap_sync(account: &AccountModel) -> RustMailerResult<()> {
    assert!(
        matches!(account.mailer_type, MailerType::ImapSmtp),
        "Bug: Unexpected mailer type, expected ImapSmtp, found: {:?}",
        account.mailer_type
    );
    let start_time = Instant::now();
    let account_id = account.id;

    let sync_type = determine_sync_type(account).await?;
    if matches!(sync_type, SyncType::SkipSync) {
        debug!(
            "Account{{{}}}: skipping sync, next sync window not reached yet.",
            &account.email
        );
        return Ok(());
    }
    report_sync_staleness(account).await?;

    let remote_mailboxes = get_sync_folders(account).await?;
    let local_mailboxes = MailBox::list_all(account_id).await?;
    let mailboxes_count = local_mailboxes.len();
    let total_envelope_count = EnvelopeFlagsManager::count_account_uid_total(account_id);
    debug!(
        "Account ID: {}, fetched total count of local cached emails, elapsed time: {} seconds",
        account_id,
        start_time.elapsed().as_secs()
    );

    if should_rebuild_cache(account, mailboxes_count, total_envelope_count).await? {
        AccountRunningState::set_initial_sync_folders(
            account_id,
            remote_mailboxes.iter().map(|m| m.name.clone()).collect(),
        )
        .await?;
        match &account.date_since {
            Some(date_since) => {
                rebuild_cache_since_date(account, &remote_mailboxes, date_since).await?;
            }
            None => {
                rebuild_cache(account, &remote_mailboxes).await?;
            }
        }
        // A rebuild is a full data pass, but only its FIRST completion flips
        // the initial-sync state and emits the AccountFirstSyncCompleted
        // event. Later rebuilds (local cache lost) merely record a full-sync
        // end: previously every rebuild re-emitted the first-sync event and
        // refreshed initial-sync timestamps, masking staleness from
        // monitoring (INCIDENT-2718318811333577).
        let first_sync_pending = !AccountRunningState::get(account_id)
            .await?
            .map(|state| state.is_initial_sync_completed)
            .unwrap_or(false);
        if first_sync_pending {
            AccountRunningState::set_initial_sync_completed(account_id).await?;
            if EventHookTask::is_watching_account_first_sync_completed(account_id).await? {
                EVENT_CHANNEL
                    .queue(Event::new(
                        account.id,
                        &account.email,
                        RustMailerEvent::new(
                            EventType::AccountFirstSyncCompleted,
                            EventPayload::AccountFirstSyncCompleted(AccountChange {
                                account_id: account_id,
                                account_email: account.email.clone(),
                            }),
                        ),
                    ))
                    .await;
            }
        } else {
            AccountRunningState::set_full_sync_end(account_id).await?;
        }

        return Ok(());
    }
    let sync_count = SYNC_COUNTER.fetch_add(1, Ordering::SeqCst);
    reconcile_mailboxes(
        account,
        &remote_mailboxes,
        &local_mailboxes,
        &sync_type,
        sync_count,
    )
    .await?;

    let elapsed_time = start_time.elapsed().as_secs();
    match sync_type {
        SyncType::FullSync => {
            info!(
                "Account{{{}}} full sync completed: {} seconds elapsed.",
                account.email, elapsed_time
            );
            AccountRunningState::set_full_sync_end(account_id).await?;
        }
        SyncType::IncrementalSync => {
            if sync_count % 10 == 0 {
                debug!(
                    "Account{{{}}} incremental sync completed: {} seconds elapsed.",
                    account.email, elapsed_time
                );
            }
            AccountRunningState::set_incremental_sync_end(account_id).await?;
        }
        SyncType::SkipSync => {
            unreachable!()
        }
    }
    Ok(())
}
