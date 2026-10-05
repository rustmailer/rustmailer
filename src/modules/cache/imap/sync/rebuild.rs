// Copyright © 2025-2026 rustmailer.com
// Licensed under RustMailer License Agreement v1.0
// Unauthorized copying, modification, or distribution is prohibited.

use crate::modules::{
    account::{since::DateSince, migration::AccountModel},
    cache::imap::{
        mailbox::MailBox,
        manager::EnvelopeFlagsManager,
        sync::flow::{fetch_and_save_full_mailbox, fetch_and_save_since_date},
    },
    error::RustMailerResult,
};
use std::time::Instant;
use tracing::{error, info, warn};

pub async fn rebuild_cache(
    account: &AccountModel,
    remote_mailboxes: &[MailBox],
) -> RustMailerResult<()> {
    let start_time = Instant::now();
    let mut total_inserted = 0;

    MailBox::batch_insert(remote_mailboxes).await?;
    for mailbox in remote_mailboxes {
        if mailbox.exists == 0 {
            info!(
                "Account {}: Mailbox '{}' on the remote server has no emails. Skipping fetch for this mailbox.",
                account.id, &mailbox.name
            );
            continue;
        }
        // total_inserted +=
        //     fetch_and_save_full_mailbox(account, mailbox, mailbox.exists, true).await?;
        match fetch_and_save_full_mailbox(account, mailbox, mailbox.exists, true).await {
            Ok(inserted) => {
                total_inserted += inserted;
            }
            Err(e) => {
                warn!(
                    "Account {}: Failed to sync mailbox '{}'. Error: {}. Removing mailbox entry.",
                    account.id, &mailbox.name, e
                );
                if let Err(del_err) = MailBox::delete(mailbox.id).await {
                    error!(
                        "Account {}: Failed to delete mailbox '{}' after sync error: {}",
                        account.id, &mailbox.name, del_err
                    );
                }
            }
        }
    }
    // commit(account).await?;

    let elapsed_time = start_time.elapsed().as_secs();
    info!(
        "Account {}: Rebuild account cache completed: {} envelopes inserted. {} secs elapsed. \
        This is a full data fetch as there was no local cache data available.",
        account.id, total_inserted, elapsed_time
    );
    Ok(())
}

pub async fn rebuild_cache_since_date(
    account: &AccountModel,
    remote_mailboxes: &[MailBox],
    date_since: &DateSince,
) -> RustMailerResult<()> {
    let start_time = Instant::now();
    let mut total_inserted = 0;
    let date = date_since.since_date()?;

    MailBox::batch_insert(remote_mailboxes).await?;

    for mailbox in remote_mailboxes {
        if mailbox.exists == 0 {
            info!(
                "Account {}: Mailbox '{}' on the remote server has no emails. Skipping fetch for this mailbox.",
                account.id, &mailbox.name
            );
            continue;
        }

        match fetch_and_save_since_date(
            account,
            date.as_str(),
            mailbox,
            true,
            account.minimal_sync(),
        )
        .await
        {
            Ok(inserted) => {
                total_inserted += inserted;
            }
            Err(e) => {
                warn!(
                    "Account {}: Failed to sync mailbox '{}'. Error: {}. Removing mailbox entry.",
                    account.id, &mailbox.name, e
                );
                if let Err(del_err) = MailBox::delete(mailbox.id).await {
                    error!(
                        "Account {}: Failed to delete mailbox '{}' after sync error: {}",
                        account.id, &mailbox.name, del_err
                    );
                }
            }
        }
    }

    let elapsed_time = start_time.elapsed().as_secs();
    info!(
        "Account {}: Rebuild account cache completed: {} envelopes inserted. {} secs elapsed. \
        Data fetched from server starting from the specified date: {}.",
        account.id, total_inserted, elapsed_time, date
    );
    Ok(())
}

pub async fn should_rebuild_cache(
    account: &AccountModel,
    mailbox_count: usize,
    local_envelope_count: usize,
) -> RustMailerResult<bool> {
    // If both local mailboxes and local envelopes exist, no rebuild is needed.
    if mailbox_count > 0 && local_envelope_count > 0 {
        return Ok(false);
    }
    // An empty envelope index alongside intact mailbox rows is a legitimate
    // steady state when a sync window (`date_since`) is configured: every
    // cached message may simply have aged out of the window. Treating it as
    // cache loss used to delete all MailBox rows and re-run a rebuild that
    // inserts nothing — an endless, silent rebuild loop during which newly
    // arriving mail was ingested without email-added events
    // (INCIDENT-2718318811333577). The regular sync paths still self-heal a
    // genuinely lost index (no max UID → date-window/full fetch).
    if mailbox_count > 0 && local_envelope_count == 0 && account.date_since.is_some() {
        return Ok(false);
    }
    // If there are local mailboxes but no local envelopes, clear the mailboxes.
    if mailbox_count > 0 {
        let mailboxes = MailBox::list_all(account.id).await?;
        MailBox::batch_delete(mailboxes).await?;
    }
    if local_envelope_count > 0 {
        EnvelopeFlagsManager::clean_account(account.id).await?
    }
    // If either local mailboxes or local envelopes were missing, cache rebuild is required.
    Ok(true)
}

pub async fn rebuild_mailbox_cache(
    account: &AccountModel,
    local_mailbox: &MailBox,
    remote_mailbox: &MailBox,
) -> RustMailerResult<()> {
    EnvelopeFlagsManager::clean_mailbox(account.id, local_mailbox.id).await?;
    if remote_mailbox.exists == 0 {
        info!(
            "Account {}: Mailbox '{}' has no emails on the remote server. The mailbox is empty, no envelopes to fetch.",
            account.id,
            &local_mailbox.name
        );
        return Ok(()); // Skip if the mailbox has no emails
    }

    let inserted_count =
        fetch_and_save_full_mailbox(account, remote_mailbox, remote_mailbox.exists, false).await?;
    info!(
        "Account {}: Successfully rebuild mailbox cache, inserted {} envelopes for mailbox '{}'.",
        account.id, inserted_count, &local_mailbox.name
    );
    Ok(())
}

pub async fn rebuild_mailbox_cache_since_date(
    account: &AccountModel,
    local_mailbox_id: u64,
    date_since: &DateSince,
    remote: &MailBox,
) -> RustMailerResult<()> {
    EnvelopeFlagsManager::clean_mailbox(account.id, local_mailbox_id).await?;
    if remote.exists == 0 {
        info!(
            "Account {}: Mailbox '{}' has no emails on the remote server. The mailbox is empty, no envelopes to fetch.",
            account.id,
            &remote.name
        );
        return Ok(()); // Skip if the mailbox has no emails
    }

    let count = fetch_and_save_since_date(
        account,
        date_since.since_date()?.as_str(),
        remote,
        false,
        account.minimal_sync(),
    )
    .await?;
    info!(
        "Account {}: Successfully rebuild mailbox cache, inserted {} envelopes for mailbox '{}'.",
        account.id, count, &remote.name
    );
    Ok(())
}
