// Copyright © 2025-2026 rustmailer.com
// Licensed under RustMailer License Agreement v1.0
// Unauthorized copying, modification, or distribution is prohibited.

use crate::{
    modules::{
        account::{migration::AccountModel, since::DateSince, status::AccountRunningState},
        bounce::parser::{extract_bounce_report, BounceReport},
        cache::{
            imap::{
                diff, find_deleted_mailboxes, find_flag_updates, find_intersecting_mailboxes,
                find_missing_mailboxes, find_missing_remote_uids,
                mailbox::{EnvelopeFlag, MailBox},
                manager::EnvelopeFlagsManager,
                migration::EmailEnvelopeV3,
                minimal::MinimalEnvelope,
                sync::rebuild::{rebuild_mailbox_cache, rebuild_mailbox_cache_since_date},
            },
            sync_type::SyncType,
            acquire_account_sync_permit, SEMAPHORE,
        },
        common::AddrVec,
        context::executors::RUST_MAIL_CONTEXT,
        envelope::{
            detect::should_extract_bounce_report,
            extractor::{
                extract_envelope, extract_minimal_envelopes, extract_rich_envelopes,
                parse_fetch_metadata,
            },
        },
        error::{code::ErrorCode, RustMailerError, RustMailerResult},
        hook::{
            channel::{Event, EVENT_CHANNEL},
            events::{
                payload::{EmailAddedToFolder, EmailBounce, EmailFeedBackReport, MailboxChange},
                EventPayload, EventType, RustMailerEvent,
            },
            task::EventHookTask,
        },
        message::content::{retrieve_email_content, FullMessageContent, MessageContentRequest},
        metrics::RUSTMAILER_NEW_EMAIL_ARRIVAL_TOTAL,
        settings::cli::SETTINGS,
    },
    raise_error,
};
use ahash::{AHashMap, AHashSet};
use async_imap::types::Fetch;
use mail_parser::{Message, MessageParser};
use std::time::Instant;
use tracing::{debug, error, info, warn};

const ENVELOPE_BATCH_SIZE: u32 = 1000;
const UID_FLAGS_BATCH_SIZE: u32 = 10000;

/// True once the account has completed its initial sync at least once. Gates
/// event emission on the bulk ingestion paths (`fetch_and_save_since_date` /
/// `fetch_and_save_full_mailbox`): a brand-new account's first fetch must not
/// storm the webhook, but a later rebuild re-ingesting mail after local cache
/// loss must still emit email-added events — previously those paths stored
/// mail silently, losing the webhook for every message they touched
/// (INCIDENT-2718318811333577).
async fn account_completed_initial_sync(account_id: u64) -> RustMailerResult<bool> {
    Ok(AccountRunningState::get(account_id)
        .await?
        .map(|state| state.is_initial_sync_completed)
        .unwrap_or(false))
}

pub async fn fetch_and_save_since_date(
    account: &AccountModel,
    date: &str,
    mailbox: &MailBox,
    initial: bool,
    minimal_sync: bool,
) -> RustMailerResult<usize> {
    let account_id = account.id;
    let executor = RUST_MAIL_CONTEXT.imap(account_id).await?;
    let uid_list = executor
        .uid_search(&mailbox.encoded_name(), format!("SINCE {date}").as_str())
        .await?;

    let len = uid_list.len();
    if len == 0 {
        return Ok(0);
    }

    // Emit email-added events for ingested mail once the account is past its
    // initial sync and the event is watched, mirroring
    // fetch_and_store_new_envelopes_by_uid_list.
    let fire_events = account_completed_initial_sync(account_id).await?
        && EventHookTask::is_watching_email_add_event(account_id).await?;

    let folder_limit = account.folder_limit;
    // sort small -> bigger
    let mut uid_vec: Vec<u32> = uid_list.into_iter().collect();
    uid_vec.sort();

    if let Some(limit) = folder_limit {
        let limit = limit.max(100) as usize;
        if len > limit {
            uid_vec = uid_vec.split_off(len - limit as usize);
        }
    }

    // let semaphore = Arc::new(Semaphore::new(5));
    let mut handles = Vec::new();

    let uid_batches = generate_uid_sequence_hashset(uid_vec, ENVELOPE_BATCH_SIZE as usize, false);

    if initial {
        AccountRunningState::set_initial_current_syncing_folder(
            account_id,
            mailbox.name.clone(),
            Some(uid_batches.len() as u32),
        )
        .await?;
    }

    for (index, batch) in uid_batches.into_iter().enumerate() {
        let encoded_name = mailbox.encoded_name();
        let mailbox_id = mailbox.id;
        let mailbox_name = mailbox.name.clone();
        let permit = match SEMAPHORE.clone().acquire_owned().await {
            Ok(permit) => permit,
            Err(err) => {
                error!("Failed to acquire semaphore permit, error: {:#?}", err);
                continue;
            }
        };
        let account_permit = match acquire_account_sync_permit(account_id).await {
            Ok(permit) => permit,
            Err(err) => {
                error!(
                    "Failed to acquire account sync permit for account {}, error: {:#?}",
                    account_id, err
                );
                continue;
            }
        };
        if initial {
            AccountRunningState::set_current_sync_batch_number(account_id, (index + 1) as u32)
                .await?;
        }
        let handle: tokio::task::JoinHandle<Result<Vec<Fetch>, RustMailerError>> = tokio::spawn(
            async move {
                let _permit = permit; // Ensure permit is released when task finishes
                let _account_permit = account_permit;
                let executor = RUST_MAIL_CONTEXT.imap(account_id).await?;
                // Fetch metadata for the current batch of UIDs. When events
                // must fire, always fetch rich metadata (the event payload
                // needs headers), mirroring the watched incremental path.
                let fetches = executor
                    .uid_fetch_meta(&batch, &encoded_name, minimal_sync && !fire_events)
                    .await?;

                if minimal_sync {
                    let envelopes =
                        extract_minimal_envelopes(&fetches, account_id, mailbox_id)?;
                    MinimalEnvelope::batch_insert(envelopes).await?;
                    if fire_events {
                        // Mirror the watched incremental path: rich envelopes
                        // are stored as well so the event payload is complete.
                        let envelopes =
                            extract_rich_envelopes(&fetches, account_id, &mailbox_name)?;
                        EmailEnvelopeV3::save_envelopes(envelopes).await?;
                    }
                } else {
                    let envelopes = extract_rich_envelopes(&fetches, account_id, &mailbox_name)?;
                    EmailEnvelopeV3::save_envelopes(envelopes).await?;
                };
                Ok(fetches)
            },
        );
        handles.push(handle);
    }
    let mut batched_fetches = Vec::new();
    for task in handles {
        match task.await {
            Ok(Ok(fetches)) => batched_fetches.push(fetches),
            Ok(Err(err)) => return Err(err),
            Err(e) => return Err(raise_error!(format!("{:#?}", e), ErrorCode::InternalError)),
        }
    }

    // Emit events after all batches are stored, in batch order. Storage has
    // already succeeded at this point, so event emission is best-effort: a
    // failed content retrieval must not fail the whole sync (callers such as
    // the rebuild paths delete the mailbox row on error).
    if fire_events {
        info!(
            "Account {}: Mailbox '{}' ingested {} message(s) via date-window path, emitting email-added events.",
            account_id, &mailbox.name, len
        );
        for fetches in &batched_fetches {
            if let Err(e) = process_email_added_events(account, mailbox, fetches).await {
                warn!(
                    "Account {}: Failed to emit email-added events for {} message(s) in mailbox '{}': {:#?}",
                    account_id,
                    fetches.len(),
                    &mailbox.name,
                    e
                );
            }
        }
    }

    Ok(len)
}

pub async fn fetch_and_save_full_mailbox(
    account: &AccountModel,
    mailbox: &MailBox,
    total: u32,
    initial: bool,
) -> RustMailerResult<usize> {
    let folder_limit = account.folder_limit;

    let total_to_fetch = match folder_limit {
        Some(limit) if limit < total => total.min(limit.max(100)),
        _ => total,
    };
    let page_size = if let Some(limit) = folder_limit {
        limit.max(100).min(ENVELOPE_BATCH_SIZE as u32)
    } else {
        ENVELOPE_BATCH_SIZE as u32
    };

    let total_batches = total_to_fetch.div_ceil(page_size);
    let desc = folder_limit.is_some();

    let mut inserted_count = 0;

    let account_id = account.id;
    let minimal_sync = account.minimal_sync();

    // Emit email-added events for ingested mail once the account is past its
    // initial sync and the event is watched, mirroring
    // fetch_and_store_new_envelopes_by_uid_list.
    let fire_events = account_completed_initial_sync(account_id).await?
        && EventHookTask::is_watching_email_add_event(account_id).await?;

    if initial {
        AccountRunningState::set_initial_current_syncing_folder(
            account_id,
            mailbox.name.clone(),
            Some(total_batches),
        )
        .await?;
    }
    info!(
        "Account {}: Starting full mailbox sync for '{}', total={}, limit={:?}, batches={}, desc={}",
        account_id, mailbox.name, total, folder_limit, total_batches, desc
    );
    // let semaphore = Arc::new(Semaphore::new(5));
    let mut handles = Vec::new();

    for page in 1..=total_batches {
        let mailbox_id = mailbox.id;
        let mailbox_name = mailbox.name.clone();
        let encoded_name = mailbox.encoded_name();
        let permit = match SEMAPHORE.clone().acquire_owned().await {
            Ok(permit) => permit,
            Err(err) => {
                error!("Failed to acquire semaphore permit, error: {:#?}", err);
                continue;
            }
        };
        let account_permit = match acquire_account_sync_permit(account_id).await {
            Ok(permit) => permit,
            Err(err) => {
                error!(
                    "Failed to acquire account sync permit for account {}, error: {:#?}",
                    account_id, err
                );
                continue;
            }
        };
        if initial {
            AccountRunningState::set_current_sync_batch_number(account_id, page).await?;
        }
        // Spawn a task with the acquired permits
        let handle: tokio::task::JoinHandle<Result<Vec<Fetch>, RustMailerError>> = tokio::spawn(
            async move {
                let _permit = permit; // Ensure permit is released when task finishes
                let _account_permit = account_permit;
                let executor = RUST_MAIL_CONTEXT.imap(account_id).await?;
                // When events must fire, always fetch rich metadata (the
                // event payload needs headers), mirroring the watched
                // incremental path.
                let (fetches, _) = executor
                    .retrieve_metadata_paginated(
                        page as u64,
                        page_size as u64,
                        &encoded_name,
                        desc,
                        minimal_sync && !fire_events,
                    )
                    .await?;
                let count = fetches.len();
                if minimal_sync {
                    let envelopes =
                        extract_minimal_envelopes(&fetches, account_id, mailbox_id)?;
                    MinimalEnvelope::batch_insert(envelopes).await?;
                    if fire_events {
                        // Mirror the watched incremental path: rich envelopes
                        // are stored as well so the event payload is complete.
                        let envelopes =
                            extract_rich_envelopes(&fetches, account_id, &mailbox_name)?;
                        EmailEnvelopeV3::save_envelopes(envelopes).await?;
                    }
                } else {
                    let envelopes = extract_rich_envelopes(&fetches, account_id, &mailbox_name)?;
                    EmailEnvelopeV3::save_envelopes(envelopes).await?;
                };
                info!("Account {}: Batch insertion completed for mailbox: {}, current page: {}, inserted count: {}", account_id, &mailbox_name, page, count);
                Ok(fetches)
            },
        );
        handles.push(handle);
    }

    let mut batched_fetches = Vec::new();
    for task in handles {
        match task.await {
            Ok(Ok(fetches)) => {
                inserted_count += fetches.len();
                batched_fetches.push(fetches);
            }
            Ok(Err(err)) => return Err(err),
            Err(e) => return Err(raise_error!(format!("{:#?}", e), ErrorCode::InternalError)),
        }
    }

    // Emit events after all pages are stored, in page order. Storage has
    // already succeeded at this point, so event emission is best-effort: a
    // failed content retrieval must not fail the whole sync (callers such as
    // the rebuild paths delete the mailbox row on error).
    if fire_events {
        info!(
            "Account {}: Mailbox '{}' ingested {} message(s) via full-mailbox path, emitting email-added events.",
            account_id, &mailbox.name, inserted_count
        );
        for fetches in &batched_fetches {
            if let Err(e) = process_email_added_events(account, mailbox, fetches).await {
                warn!(
                    "Account {}: Failed to emit email-added events for {} message(s) in mailbox '{}': {:#?}",
                    account_id,
                    fetches.len(),
                    &mailbox.name,
                    e
                );
            }
        }
    }

    Ok(inserted_count)
}

/// # Example
///
/// ```rust
/// use std::collections::HashSet;
///
/// let mut uids = HashSet::new();
/// uids.extend([1, 2, 3, 5, 6, 7, 9, 10, 11, 15]);
///
/// let chunks = generate_uid_sequence_hashset(uids, 6, false);
/// assert_eq!(chunks, vec![
///     "1:3,5:7".to_string(),
///     "9:11,15".to_string()
/// ]);
/// ```
///
/// This splits the UIDs into chunks of 6, compresses each chunk into ranges,
/// and returns a vector like: `["1:3,5:7", "9:11,15"]`.
///
pub fn generate_uid_sequence_hashset(
    unique_nums: Vec<u32>,
    chunk_size: usize,
    desc: bool,
) -> Vec<String> {
    assert!(!unique_nums.is_empty());
    // let mut nums: Vec<u32> = unique_nums.into_iter().collect();
    // nums.sort();
    let mut nums = unique_nums;
    if desc {
        nums.reverse();
    }

    let mut result = Vec::new();

    for chunk in nums.chunks(chunk_size) {
        let compressed = compress_uid_list(chunk.to_vec());
        result.push(compressed);
    }

    result
}

pub fn compress_uid_list(nums: Vec<u32>) -> String {
    if nums.is_empty() {
        return String::new();
    }

    let mut sorted_nums = nums;
    sorted_nums.sort();

    let mut result = Vec::new();
    let mut current_range_start = sorted_nums[0];
    let mut current_range_end = sorted_nums[0];

    for &n in sorted_nums.iter().skip(1) {
        if n == current_range_end + 1 {
            current_range_end = n;
        } else {
            if current_range_start == current_range_end {
                result.push(current_range_start.to_string());
            } else {
                result.push(format!("{}:{}", current_range_start, current_range_end));
            }
            current_range_start = n;
            current_range_end = n;
        }
    }

    if current_range_start == current_range_end {
        result.push(current_range_start.to_string());
    } else {
        result.push(format!("{}:{}", current_range_start, current_range_end));
    }

    result.join(",")
}

pub async fn reconcile_mailboxes(
    account: &AccountModel,
    remote_mailboxes: &[MailBox],
    local_mailboxes: &[MailBox],
    sync_type: &SyncType,
    count: usize,
) -> RustMailerResult<()> {
    let start_time = Instant::now();
    let existing_mailboxes = find_intersecting_mailboxes(local_mailboxes, remote_mailboxes);
    let account_id = account.id;
    if !existing_mailboxes.is_empty() {
        let mut mailboxes_to_update = Vec::with_capacity(existing_mailboxes.len());
        for (local_mailbox, remote_mailbox) in &existing_mailboxes {
            if local_mailbox.uid_validity != remote_mailbox.uid_validity {
                if remote_mailbox.uid_validity.is_none() {
                    warn!(
                        "Account {}: Mailbox '{}' has invalid uid_validity (None). Skipping sync for this mailbox.",
                        account_id, local_mailbox.name
                    );
                    continue;
                }
                info!(
                    "Account {}: Mailbox '{}' detected with changed uid_validity (local: {:#?}, remote: {:#?}). \
                    The mailbox data may be invalid, resetting its envelopes and rebuilding the cache.",
                    account_id, local_mailbox.name, &local_mailbox.uid_validity, &remote_mailbox.uid_validity
                );
                if EventHookTask::is_watching_uid_validity_change(account_id).await? {
                    EVENT_CHANNEL
                        .queue(Event::new(
                            account_id,
                            &account.email,
                            RustMailerEvent::new(
                                EventType::UIDValidityChange,
                                EventPayload::UIDValidityChange(MailboxChange {
                                    account_id,
                                    account_email: account.email.clone(),
                                    mailbox_name: local_mailbox.name.clone(),
                                }),
                            ),
                        ))
                        .await;
                }
                match &account.date_since {
                    Some(date_since) => {
                        rebuild_mailbox_cache_since_date(
                            account,
                            local_mailbox.id,
                            date_since,
                            remote_mailbox,
                        )
                        .await?;
                    }
                    None => {
                        rebuild_mailbox_cache(account, local_mailbox, remote_mailbox).await?;
                    }
                }
            } else {
                match sync_type {
                    SyncType::FullSync => {
                        perform_full_sync(account, local_mailbox, remote_mailbox).await?;
                    }
                    SyncType::IncrementalSync => {
                        perform_incremental_sync(account, local_mailbox, remote_mailbox, count)
                            .await?;
                    }
                    SyncType::SkipSync => unreachable!(),
                }
            }
            mailboxes_to_update.push(remote_mailbox.clone());
        }
        //The metadata of this mailbox must only be updated after a successful synchronization;
        //otherwise, it may cause synchronization errors and result in missing emails in the local sync results.
        MailBox::batch_upsert(&mailboxes_to_update).await?;
    }
    debug!(
        "Checked mailbox folders for account ID: {}. Compared local and server folders to identify changes. Elapsed time: {} seconds",
        account.id,
        start_time.elapsed().as_secs()
    );
    let deleted_mailboxes = find_deleted_mailboxes(local_mailboxes, remote_mailboxes);
    let missing_mailboxes = find_missing_mailboxes(local_mailboxes, remote_mailboxes);

    // handle_renamed_mailboxes(account, &mut deleted_mailboxes, &mut missing_mailboxes).await?;
    //delete local
    if !deleted_mailboxes.is_empty() {
        info!(
            "Account {}: Detected {} mailboxes missing from the IMAP server (not found in the LSUB response). \
            Now cleaning up these mailboxes and their associated metadata locally.",
            account_id, deleted_mailboxes.len()
        );
        cleanup_deleted_mailboxes(account, &deleted_mailboxes).await?;
    }
    // Newly added mailboxes should also verify if a time range is set
    if !missing_mailboxes.is_empty() {
        MailBox::batch_insert(&missing_mailboxes).await?;
        for mailbox in &missing_mailboxes {
            if mailbox.exists > 0 {
                match &account.date_since {
                    Some(date_since) => {
                        match rebuild_mailbox_cache_since_date(
                            account, mailbox.id, date_since, &mailbox,
                        )
                        .await
                        {
                            Ok(_) => {}
                            Err(e) => {
                                warn!("Account {}: Failed to sync mailbox '{}'. Error: {}. Removing mailbox entry.", account.id, &mailbox.name, e);
                                if let Err(del_err) = MailBox::delete(mailbox.id).await {
                                    error!(
                                "Account {}: Failed to delete mailbox '{}' after sync error: {}",
                                account.id, &mailbox.name, del_err
                            );
                                }
                            }
                        }
                    }
                    None => match rebuild_mailbox_cache(account, &mailbox, &mailbox).await {
                        Ok(_) => {}
                        Err(e) => {
                            warn!("Account {}: Failed to sync mailbox '{}'. Error: {}. Removing mailbox entry.", account.id, &mailbox.name, e);
                            if let Err(del_err) = MailBox::delete(mailbox.id).await {
                                error!(
                                "Account {}: Failed to delete mailbox '{}' after sync error: {}",
                                account.id, &mailbox.name, del_err
                            );
                            }
                        }
                    },
                }
            }
        }
    }
    Ok(())
}

async fn cleanup_deleted_mailboxes(
    account: &AccountModel,
    deleted_mailboxes: &[MailBox],
) -> RustMailerResult<()> {
    let start_time = Instant::now();
    for mailbox in deleted_mailboxes {
        EnvelopeFlagsManager::clean_mailbox(account.id, mailbox.id).await?;
    }
    MailBox::batch_delete(deleted_mailboxes.to_vec()).await?;
    let elapsed_time = start_time.elapsed().as_secs();
    info!(
        "Account {}: Cleanup deleted mailboxes completed: {} seconds elapsed.",
        account.id, elapsed_time
    );
    Ok(())
}
/// Sync recent 200 envelopes's flags
async fn sync_recent_envelope_flags(
    account: &AccountModel,
    local_mailbox: &MailBox,
    remote_mailbox: &MailBox,
    uid_next: u32,
) -> RustMailerResult<()> {
    let min_uid = (uid_next.saturating_sub(200)).max(1);

    let local_uid_flags = EnvelopeFlagsManager::get_uid_map(account.id, local_mailbox.id, min_uid);

    let executor = RUST_MAIL_CONTEXT.imap(account.id).await?;
    let uid_set = format!("{}:*", min_uid);
    let fetches = executor
        .uid_fetch_uid_and_flags(uid_set.as_str(), &remote_mailbox.encoded_name())
        .await?;
    let remote_uid_flags = parse_fetch_metadata(fetches, false)?;
    let update_flags = find_flag_updates(&local_uid_flags, remote_uid_flags);
    if !update_flags.is_empty() {
        info!(
            "Account {}: Mailbox '{}' incremental sync - {} message flags changed (UID range {}..*)",
            &account.id,
            &local_mailbox.name,
            update_flags.len(),
            min_uid
        );

        EnvelopeFlagsManager::update_envelope_flags(account, local_mailbox.id, update_flags)
            .await?;
    }
    Ok(())
}

//only check new emails and sync
async fn perform_incremental_sync(
    account: &AccountModel,
    local_mailbox: &MailBox,
    remote_mailbox: &MailBox,
    sync_count: usize,
) -> RustMailerResult<()> {
    if let (Some(local_uid_next), Some(remote_uid_next)) =
        (local_mailbox.uid_next, remote_mailbox.uid_next)
    {
        if local_uid_next == remote_uid_next && local_mailbox.exists == remote_mailbox.exists {
            if sync_count % 10 == 0 {
                debug!(
                    "Account {}: Mailbox '{}' has the same uid_next and exists.",
                    account.id, &local_mailbox.name
                );
            }
            return sync_recent_envelope_flags(
                account,
                local_mailbox,
                remote_mailbox,
                local_uid_next,
            )
            .await;
        }
    }

    if remote_mailbox.exists > 0 {
        let local_max_uid = EnvelopeFlagsManager::get_max_uid(account.id, local_mailbox.id);
        match local_max_uid {
            Some(max_uid) => {
                let executor = RUST_MAIL_CONTEXT.imap(account.id).await?;
                let fetches = executor
                    .fetch_uid_list(
                        max_uid + 1,
                        &remote_mailbox.encoded_name(),
                        account.minimal_sync(),
                    )
                    .await?;
                let uid_list = parse_fetch_metadata(fetches, !account.minimal_sync())?;

                // This is just a precaution in case the server's behavior deviates from expectations.
                let uid_list = uid_list
                    .into_iter()
                    .filter(|i| i.0 > (max_uid as u32))
                    .map(|e| (e.0, e.1 .0))
                    .collect();
                fetch_and_store_new_envelopes_by_uid_list(
                    account,
                    local_mailbox.id,
                    remote_mailbox,
                    uid_list,
                )
                .await?;
            }
            None => {
                info!(
                    "Account {}: No maximum UID found in index for mailbox '{}', assuming local cache is missing.",
                    account.id, &local_mailbox.name
                );

                match &account.date_since {
                    Some(date_since) => {
                        fetch_and_save_since_date(
                            account,
                            date_since.since_date()?.as_str(),
                            remote_mailbox,
                            false,
                            account.minimal_sync(),
                        )
                        .await?;
                    }
                    None => {
                        fetch_and_save_full_mailbox(
                            account,
                            remote_mailbox,
                            remote_mailbox.exists,
                            false,
                        )
                        .await?;
                    }
                }
            }
        }
    }

    Ok(())
}
//
async fn perform_full_sync(
    account: &AccountModel,
    local_mailbox: &MailBox,
    remote_mailbox: &MailBox,
) -> RustMailerResult<()> {
    if remote_mailbox.exists == 0 {
        info!("Account {}: Mailbox '{}' has been cleared on the remote server (no emails). Clearing local mailbox metadata.", &account.id, &local_mailbox.name);
        EnvelopeFlagsManager::clean_mailbox(account.id, local_mailbox.id).await?;
        return Ok(());
    }

    let local_uid_flags_index = EnvelopeFlagsManager::get_uid_map(account.id, local_mailbox.id, 0);

    let (remote_uids_set, uids_with_updated_flags, new_uids_to_add) = diff_uids_and_flags(
        account,
        remote_mailbox,
        &local_uid_flags_index,
        remote_mailbox.exists,
        &account.date_since,
    )
    .await?;

    debug!(
        "Diff result for account_id={}, mailbox='{}': remote_uids_set (total on server) = {}, uids_with_updated_flags (local flags updated) = {}, new_uids_to_add (new UIDs from server) = {}",
        account.id,
        remote_mailbox.name,
        remote_uids_set.len(),
        uids_with_updated_flags.len(),
        new_uids_to_add.len()
    );

    if !new_uids_to_add.is_empty() {
        fetch_and_store_new_envelopes_by_uid_list(
            account,
            local_mailbox.id,
            remote_mailbox,
            new_uids_to_add,
        )
        .await?;
    }

    cleanup_missing_remote_emails(
        account,
        local_mailbox.id,
        &local_mailbox.name,
        &local_uid_flags_index,
        &remote_uids_set,
    )
    .await?;

    if !uids_with_updated_flags.is_empty() {
        info!("Account {}: Mailbox '{}' has {} envelopes with updated flags. Applying the changes locally.", &account.id, &local_mailbox.name, uids_with_updated_flags.len());
        EnvelopeFlagsManager::update_envelope_flags(
            account,
            local_mailbox.id,
            uids_with_updated_flags,
        )
        .await?;
    }
    Ok(())
}

//Compare the local uid and flags_hash with those from the server to identify newly added, deleted, and flag-changed messages.
async fn diff_uids_and_flags(
    account: &AccountModel,
    remote_mailbox: &MailBox,
    local_uid_flags_index: &AHashMap<u32, u64>,
    total: u32,
    date_since: &Option<DateSince>,
) -> RustMailerResult<(
    AHashSet<u32>,
    Vec<(u32, Vec<EnvelopeFlag>)>,
    Vec<(u32, u64)>,
)> {
    let account_id = account.id;
    let mut remote_uids_set = AHashSet::new();
    let mut uids_with_updated_flags = Vec::new();
    let mut new_uids_to_add = Vec::new();

    debug!(
        "Starting diff_uids_and_flags: account_id={}, mailbox={}, total={}, date_since={:?}, local_uid_flags_index_len={}",
        account_id,
        &remote_mailbox.name,
        total,
        date_since,
        local_uid_flags_index.len()
    );

    let remote_mailbox_encoded_name = &remote_mailbox.encoded_name();
    match date_since {
        Some(date_since) => {
            let date = date_since.since_date()?;
            let executor = RUST_MAIL_CONTEXT.imap(account_id).await?;
            //3627
            let uid_list = executor
                .uid_search(
                    remote_mailbox_encoded_name,
                    format!("SINCE {}", date).as_str(),
                )
                .await?;
            debug!("UID search returned {} UIDs", uid_list.len());

            // The uid_list may exceed the folder_limit; trim it before comparison
            // so that synchronization can automatically clean up local cache.
            if !uid_list.is_empty() {
                let len = uid_list.len();
                let mut nums: Vec<u32> = uid_list.into_iter().collect();
                nums.sort();
                let folder_limit = account.folder_limit;
                if let Some(limit) = folder_limit {
                    let limit = limit.max(100) as usize;
                    // If the returned count exceeds the limit, keep only the latest 'limit' UIDs
                    // (those with larger UID values) for fetching UIDs and flags.
                    if len > limit {
                        nums = nums.split_off(len - limit as usize);
                    }
                }
                let uid_batches =
                    generate_uid_sequence_hashset(nums, UID_FLAGS_BATCH_SIZE as usize, false);
                debug!("Split into {} UID batches", uid_batches.len());
                for (i, batch) in uid_batches.iter().enumerate() {
                    debug!(
                        "Fetching batch {}/{} -> UID sequence: {}",
                        i + 1,
                        uid_batches.len(),
                        batch
                    );
                    let fetches = executor
                        .uid_fetch_uid_and_flags(&batch, remote_mailbox_encoded_name)
                        .await?;
                    debug!("Fetched {} messages in batch {}", fetches.len(), i + 1);
                    let uid_flags_batch = parse_fetch_metadata(fetches, false)?;
                    debug!("Parsed {} UID-flag pairs", uid_flags_batch.len());
                    let (update_flags, add, uids) = diff(local_uid_flags_index, uid_flags_batch);
                    uids_with_updated_flags.extend(update_flags);
                    new_uids_to_add.extend(add);
                    remote_uids_set.extend(uids);
                }
            }
        }
        None => {
            if total > 0 {
                let folder_limit = account.folder_limit;
                // Calculate the actual number of messages to be fetched
                let total_to_fetch = match folder_limit {
                    Some(limit) if limit < total => total.min(limit.max(100)),
                    _ => total,
                };

                let page_size = if let Some(limit) = folder_limit {
                    limit.max(100).min(UID_FLAGS_BATCH_SIZE as u32)
                } else {
                    UID_FLAGS_BATCH_SIZE as u32
                };
                // Calculate the total number of pages to fetch
                let num_pages = total_to_fetch.div_ceil(page_size);
                let desc = folder_limit.is_some();

                for page in 1..=num_pages {
                    let executor = RUST_MAIL_CONTEXT.imap(account_id).await?;
                    let fetches = executor
                        .retrieve_paginated_uid_and_flags(
                            page,
                            page_size,
                            remote_mailbox_encoded_name,
                            desc,
                        )
                        .await?;
                    let uid_flags_batch = parse_fetch_metadata(fetches, false)?;
                    let (update_flags, add, uids) = diff(local_uid_flags_index, uid_flags_batch);
                    uids_with_updated_flags.extend(update_flags);
                    new_uids_to_add.extend(add);
                    remote_uids_set.extend(uids);
                }
            }
        }
    }
    Ok((remote_uids_set, uids_with_updated_flags, new_uids_to_add))
}

async fn cleanup_missing_remote_emails(
    account: &AccountModel,
    mailbox_id: u64,
    mailbox_name: &str,
    local_uid_flags_index: &AHashMap<u32, u64>,
    remote_uid_set: &AHashSet<u32>,
) -> RustMailerResult<()> {
    let uids_to_remove = find_missing_remote_uids(local_uid_flags_index, remote_uid_set);
    if !uids_to_remove.is_empty() {
        info!(
            "Account {}: Mailbox '{}' has {} local envelopes that are missing on the server, possibly due to deletions or sync window changes. Removing them locally.",
            account.id,
            mailbox_name,
            uids_to_remove.len()
        );

        EnvelopeFlagsManager::clean_envelopes(account.id, mailbox_id, &uids_to_remove).await?;
    }
    Ok(())
}

pub async fn fetch_and_store_new_envelopes_by_uid_list(
    account: &AccountModel,
    local_mailbox_id: u64,
    remote: &MailBox,
    uid_list: Vec<(u32, u64)>,
) -> RustMailerResult<()> {
    if uid_list.is_empty() {
        return Ok(());
    }

    let len = uid_list.len();
    RUSTMAILER_NEW_EMAIL_ARRIVAL_TOTAL.inc_by(len as u64);

    let is_email_added_watched = EventHookTask::is_watching_email_add_event(account.id).await?;
    let is_bounce_watched = EventHookTask::bounce_watched(account.id).await?;

    // Early return if no relevant events are being watched
    if !is_email_added_watched && !is_bounce_watched {
        return handle_minimal_sync_or_metadata_fetch(
            account,
            local_mailbox_id,
            remote,
            uid_list.clone(),
            len,
        )
        .await;
    }

    info!("Account {}: Mailbox '{}' has {} new message UID(s) to fetch metadata. Starting download...", account.id, &remote.name, len);

    // Process minimal sync case first
    if account.minimal_sync() {
        let envelopes: Vec<MinimalEnvelope> = uid_list
            .clone()
            .into_iter()
            .map(|(uid, flags_hash)| MinimalEnvelope {
                account_id: account.id,
                mailbox_id: local_mailbox_id,
                uid,
                flags_hash,
            })
            .collect();
        MinimalEnvelope::batch_insert(envelopes).await?;
    }

    // Process batches of UIDs
    let uid_batches = generate_uid_sequence(
        uid_list.into_iter().map(|e| e.0).collect(),
        ENVELOPE_BATCH_SIZE as usize,
    );

    for batch in uid_batches {
        let executor = RUST_MAIL_CONTEXT.imap(account.id).await?;
        let fetches = executor
            .uid_fetch_meta(&batch, &remote.encoded_name(), false)
            .await?;

        // Store rich documents if not in minimal sync mode
        let envelopes = extract_rich_envelopes(&fetches, account.id, &remote.name)?;
        EmailEnvelopeV3::save_envelopes(envelopes).await?;

        // Process bounce reports if needed
        if is_bounce_watched {
            process_bounce_reports(&account, remote, &fetches).await?;
        }

        // Process email added events if needed
        if is_email_added_watched {
            process_email_added_events(&account, remote, &fetches).await?;
        }
    }

    info!(
        "Account {}: Finished fetching and processing metadata for {} new UIDs in mailbox '{}'.",
        account.id, len, &remote.name
    );

    Ok(())
}

async fn process_email_added_events(
    account: &AccountModel,
    remote: &MailBox,
    fetches: &[Fetch],
) -> RustMailerResult<()> {
    for fetch in fetches {
        let envelope = extract_envelope(fetch, account.id, &remote.name)?;
        let thread_id = envelope.compute_thread_id();
        let message_content = match envelope.body_meta {
            Some(sections) => {
                let request = MessageContentRequest {
                    mailbox: Some(remote.name.clone()),
                    id: envelope.uid.to_string(),
                    max_length: Some(SETTINGS.rustmailer_max_email_content_length as usize),
                    sections: Some(sections),
                    inline: envelope
                        .attachments
                        .as_ref()
                        .map(|att| att.iter().filter(|a| a.inline).cloned().collect()),
                };
                retrieve_email_content(account.id, request, true).await?
            }
            None => FullMessageContent {
                plain: None,
                html: None,
                attachments: None,
            },
        };
        EVENT_CHANNEL
            .queue(Event::new(
                account.id,
                &account.email,
                RustMailerEvent::new(
                    EventType::EmailAddedToFolder,
                    EventPayload::EmailAddedToFolder(EmailAddedToFolder {
                        account_id: account.id,
                        account_email: account.email.clone(),
                        mailbox_name: remote.name.clone(),
                        id: envelope.uid.to_string(),
                        internal_date: envelope.internal_date,
                        date: envelope.date,
                        from: envelope.from,
                        subject: envelope.subject,
                        to: envelope.to,
                        size: envelope.size,
                        flags: envelope.flags.into_iter().map(|f| f.to_string()).collect(),
                        cc: envelope.cc,
                        bcc: envelope.bcc,
                        in_reply_to: envelope.in_reply_to,
                        sender: envelope.sender,
                        message_id: envelope.message_id,
                        message: message_content,
                        thread_name: envelope.thread_name,
                        reply_to: envelope.reply_to,
                        thread_id: thread_id.to_string(),
                        labels: vec![],
                    }),
                ),
            ))
            .await;
    }
    Ok(())
}

async fn handle_minimal_sync_or_metadata_fetch(
    account: &AccountModel,
    local_mailbox_id: u64,
    remote: &MailBox,
    uid_list: Vec<(u32, u64)>,
    len: usize,
) -> RustMailerResult<()> {
    if account.minimal_sync() {
        let envelopes: Vec<MinimalEnvelope> = uid_list
            .into_iter()
            .map(|(uid, flags_hash)| MinimalEnvelope {
                account_id: account.id,
                mailbox_id: local_mailbox_id,
                uid,
                flags_hash,
            })
            .collect();
        MinimalEnvelope::batch_insert(envelopes).await?;
    } else {
        info!("Account {}: Mailbox '{}' has {} new message UID(s) to fetch metadata. Starting download...", account.id, &remote.name, len);

        let uid_batches = generate_uid_sequence(
            uid_list.into_iter().map(|e| e.0).collect(),
            ENVELOPE_BATCH_SIZE as usize,
        );

        for batch in uid_batches {
            let executor = RUST_MAIL_CONTEXT.imap(account.id).await?;
            let fetches = executor
                .uid_fetch_meta(&batch, &remote.encoded_name(), false)
                .await?;
            let envelopes = extract_rich_envelopes(&fetches, account.id, &remote.name)?;
            EmailEnvelopeV3::save_envelopes(envelopes).await?;
        }

        info!(
            "Account {}: Finished fetching metadata for {} new UIDs in mailbox '{}'.",
            account.id, len, &remote.name
        );
    }
    Ok(())
}

async fn process_bounce_reports(
    account: &AccountModel,
    remote: &MailBox,
    fetches: &[Fetch],
) -> RustMailerResult<()> {
    for fetch in fetches {
        if !should_extract_bounce_report(fetch)? {
            continue;
        }

        let executor = RUST_MAIL_CONTEXT.imap(account.id).await?;
        let uid = fetch
            .uid
            .ok_or_else(|| raise_error!("Missing UID".into(), ErrorCode::InternalError))?;

        let fetch = executor
            .uid_fetch_full_message(&uid.to_string(), &remote.encoded_name())
            .await?
            .ok_or_else(|| raise_error!("Message not found".into(), ErrorCode::InternalError))?;

        let body = fetch
            .body()
            .ok_or_else(|| raise_error!("Missing message body".into(), ErrorCode::InternalError))?;
        let message = MessageParser::new().parse(body).ok_or_else(|| {
            raise_error!("Failed to parse message".into(), ErrorCode::InternalError)
        })?;

        let report = extract_bounce_report(&message);

        // Process bounce event
        if EventHookTask::is_watching_email_bounce(account.id).await?
            && report.delivery_status.is_some()
            && report.original_headers.is_some()
        {
            submit_bounce_event(account, remote, uid, &fetch, &message, &report).await?;
        }

        // Process feedback report event
        if EventHookTask::is_watching_email_feedback_report(account.id).await?
            && report.feedback_report.is_some()
            && report.original_headers.is_some()
        {
            submit_feedback_report_event(account, remote, uid, &fetch, &message, report).await?;
        }
    }
    Ok(())
}

async fn submit_bounce_event(
    account: &AccountModel,
    remote: &MailBox,
    uid: u32,
    fetch: &Fetch,
    message: &Message<'_>,
    report: &BounceReport,
) -> RustMailerResult<()> {
    EVENT_CHANNEL
        .queue(Event::new(
            account.id,
            &account.email,
            RustMailerEvent::new(
                EventType::EmailBounce,
                EventPayload::EmailBounce(EmailBounce {
                    account_id: account.id,
                    account_email: account.email.clone(),
                    mailbox_name: remote.name.clone(),
                    uid,
                    internal_date: fetch.internal_date().map(|d| d.timestamp_millis()),
                    date: message.date().map(|d| d.to_timestamp() * 1000),
                    from: message
                        .from()
                        .and_then(|addr| AddrVec::from(addr).0.first().cloned()),
                    subject: message.subject().map(String::from),
                    to: message.to().map(|addr| AddrVec::from(addr).0),
                    original_headers: report.original_headers.clone(),
                    delivery_status: report.delivery_status.clone(),
                }),
            ),
        ))
        .await;
    Ok(())
}

async fn submit_feedback_report_event(
    account: &AccountModel,
    remote: &MailBox,
    uid: u32,
    fetch: &Fetch,
    message: &Message<'_>,
    report: BounceReport,
) -> RustMailerResult<()> {
    EVENT_CHANNEL
        .queue(Event::new(
            account.id,
            &account.email,
            RustMailerEvent::new(
                EventType::EmailFeedBackReport,
                EventPayload::EmailFeedBackReport(EmailFeedBackReport {
                    account_id: account.id,
                    account_email: account.email.clone(),
                    mailbox_name: remote.name.clone(),
                    uid,
                    internal_date: fetch.internal_date().map(|d| d.timestamp_millis()),
                    date: message.date().map(|d| d.to_timestamp() * 1000),
                    from: message
                        .from()
                        .and_then(|addr| AddrVec::from(addr).0.first().cloned()),
                    subject: message.subject().map(String::from),
                    to: message.to().map(|addr| AddrVec::from(addr).0),
                    original_headers: report.original_headers,
                    feedback_report: report.feedback_report,
                }),
            ),
        ))
        .await;
    Ok(())
}

fn generate_uid_sequence(nums: Vec<u32>, chunk_size: usize) -> Vec<String> {
    assert!(!nums.is_empty());
    let unique_nums: AHashSet<u32> = nums.into_iter().collect();
    let mut nums: Vec<u32> = unique_nums.into_iter().collect();
    nums.sort();

    let mut result = Vec::new();
    for chunk in nums.chunks(chunk_size) {
        let compressed = compress_uid_list(chunk.to_vec());
        result.push(compressed);
    }

    result
}
