// Copyright © 2025-2026 rustmailer.com
// Licensed under RustMailer License Agreement v1.0
// Unauthorized copying, modification, or distribution is prohibited.

use ahash::AHashMap;
use dashmap::DashMap;
use futures::{stream, StreamExt};
use std::collections::HashSet;
use std::sync::LazyLock;
use tracing::warn;

use crate::modules::account::migration::AccountModel;
use crate::modules::cache::imap::address::AddressEntity;
use crate::modules::cache::imap::flags_to_hash;
use crate::modules::cache::imap::mailbox::EnvelopeFlag;
use crate::modules::cache::imap::migration::EmailEnvelopeV3;
use crate::modules::cache::imap::minimal::MinimalEnvelope;
use crate::modules::cache::imap::thread::EmailThread;
use crate::modules::context::Initialize;
use crate::modules::error::RustMailerResult;
use crate::modules::hook::channel::{Event, EVENT_CHANNEL};
use crate::modules::hook::events::payload::EmailFlagsChanged;
use crate::modules::hook::events::{EventPayload, EventType, RustMailerEvent};
use crate::modules::hook::task::EventHookTask;
use crate::modules::metrics::RUSTMAILER_MAIL_FLAG_CHANGE_TOTAL;

/// Type aliases
pub type UID = u32;
pub type FlagsHash = u64;

/// Global flags state map
pub static FLAGS_STATE_MAP: LazyLock<DashMap<u64, DashMap<u64, DashMap<UID, FlagsHash>>>> =
    LazyLock::new(DashMap::new);

pub struct EnvelopeFlagsManager;

impl EnvelopeFlagsManager {
    pub async fn load_state() -> RustMailerResult<()> {
        let all_accounts = AccountModel::list_all().await?;

        stream::iter(all_accounts)
            .filter(|account| futures::future::ready(account.enabled))
            .for_each_concurrent(10, |account| async move {
                match MinimalEnvelope::list_by_account(account.id).await {
                    Ok(list) => {
                        for e in list {
                            EnvelopeFlagsManager::update_flag_change(
                                account.id,
                                e.mailbox_id,
                                e.uid,
                                e.flags_hash,
                            );
                        }
                    }
                    Err(e) => {
                        warn!(
                            "Failed to load envelopes for account {}: {:?}",
                            account.id, e
                        );
                    }
                }
            })
            .await;

        Ok(())
    }

    pub fn update_flag_change(account_id: u64, mailbox_id: u64, uid: UID, flags_hash: FlagsHash) {
        let mailbox_map = FLAGS_STATE_MAP
            .entry(account_id)
            .or_insert_with(DashMap::new);
        let uid_map = mailbox_map.entry(mailbox_id).or_insert_with(DashMap::new);
        uid_map.insert(uid, flags_hash);
    }

    pub async fn clean_account(account_id: u64) -> RustMailerResult<()> {
        FLAGS_STATE_MAP.remove(&account_id);
        EmailEnvelopeV3::clean_account(account_id).await?;
        MinimalEnvelope::clean_account(account_id).await?;
        AddressEntity::clean_account(account_id).await?;
        EmailThread::clean_account(account_id).await
    }

    pub async fn clean_envelopes(
        account_id: u64,
        mailbox_id: u64,
        to_delete_uid: &[u32],
    ) -> RustMailerResult<()> {
        EmailEnvelopeV3::clean_envelopes(account_id, mailbox_id, to_delete_uid).await?;
        MinimalEnvelope::clean_envelopes(account_id, mailbox_id, to_delete_uid).await?;
        AddressEntity::clean_envelopes(account_id, mailbox_id, to_delete_uid).await?;
        EmailThread::clean_envelopes(account_id, mailbox_id, to_delete_uid).await?;
        if let Some(mailboxes_map) = FLAGS_STATE_MAP.get(&account_id) {
            if let Some(flags_map) = mailboxes_map.get(&mailbox_id) {
                for uid in to_delete_uid {
                    flags_map.remove(uid);
                }
                if flags_map.is_empty() {
                    mailboxes_map.remove(&mailbox_id);
                }
            }
            if mailboxes_map.is_empty() {
                FLAGS_STATE_MAP.remove(&account_id);
            }
        }
        Ok(())
    }

    /// Clean all data associated with a specific mailbox for a given account.
    pub async fn clean_mailbox(account_id: u64, mailbox_id: u64) -> RustMailerResult<()> {
        if let Some(mailbox_map) = FLAGS_STATE_MAP.get(&account_id) {
            mailbox_map.remove(&mailbox_id);
        }
        EmailEnvelopeV3::clean_mailbox_envelopes(account_id, mailbox_id).await?;
        MinimalEnvelope::clean_mailbox_envelopes(account_id, mailbox_id).await?;
        AddressEntity::clean_mailbox_envelopes(account_id, mailbox_id).await?;
        EmailThread::clean_mailbox_envelopes(account_id, mailbox_id).await
    }

    pub fn get_uid_map(account_id: u64, mailbox_id: u64, min_uid: UID) -> AHashMap<UID, FlagsHash> {
        let mut result = AHashMap::new();
        if let Some(mailboxes) = FLAGS_STATE_MAP.get(&account_id) {
            if let Some(uids_map) = mailboxes.get(&mailbox_id) {
                for entry in uids_map.iter() {
                    let uid = *entry.key();
                    if uid >= min_uid {
                        result.insert(uid, *entry.value());
                    }
                }
            }
        }
        result
    }

    pub async fn modify_flags(
        uids: Vec<u32>,
        account_id: u64,
        mailbox_id: u64,
        overwrite_flags: Option<Vec<EnvelopeFlag>>,
        added_flags: Option<Vec<EnvelopeFlag>>,
        removed_flags: Option<Vec<EnvelopeFlag>>,
    ) -> RustMailerResult<()> {
        for uid in uids {
            let mail = EmailEnvelopeV3::find(account_id, mailbox_id, uid).await?;
            if let Some(mail) = mail {
                let current_flags = mail.flags;
                let new_flags = if let Some(overwrite) = &overwrite_flags {
                    overwrite.clone()
                } else {
                    let mut flags_set: std::collections::HashSet<EnvelopeFlag> =
                        current_flags.into_iter().collect();
                    if let Some(add) = &added_flags {
                        for flag in add {
                            flags_set.insert(flag.clone());
                        }
                    }
                    if let Some(remove) = &removed_flags {
                        for flag in remove {
                            flags_set.remove(&flag);
                        }
                    }
                    flags_set.into_iter().collect()
                };
                let flags_hash = flags_to_hash(&new_flags);
                EmailEnvelopeV3::update_flags(account_id, mailbox_id, uid, &new_flags, flags_hash)
                    .await?;
                MinimalEnvelope::update_flags(account_id, mailbox_id, uid, flags_hash).await?;
                Self::update_flag_change(account_id, mailbox_id, uid, flags_hash);
            }
        }
        Ok(())
    }

    pub async fn update_envelope_flags(
        account: &AccountModel,
        mailbox_id: u64,
        data: Vec<(u32, Vec<EnvelopeFlag>)>,
    ) -> RustMailerResult<()> {
        RUSTMAILER_MAIL_FLAG_CHANGE_TOTAL.inc_by(data.len() as u64);
        for (uid, flags) in data {
            if !account.minimal_sync()
                && EventHookTask::is_watching_email_flags_changed(account.id).await?
            {
                if let Some(current) = EmailEnvelopeV3::find(account.id, mailbox_id, uid).await? {
                    let (added, removed) = Self::diff_envelope_flags(&current.flags, &flags);
                    EVENT_CHANNEL
                        .queue(Event::new(
                            account.id,
                            &account.email,
                            RustMailerEvent::new(
                                EventType::EmailFlagsChanged,
                                EventPayload::EmailFlagsChanged(EmailFlagsChanged {
                                    account_id: account.id,
                                    account_email: account.email.clone(),
                                    mailbox_name: current.mailbox_name,
                                    uid: Some(uid),
                                    from: current.from,
                                    to: current.to,
                                    message_id: current.message_id,
                                    subject: current.subject,
                                    internal_date: current.internal_date,
                                    date: current.date,
                                    flags_added: added,
                                    flags_removed: removed,
                                    mid: None,
                                }),
                            ),
                        ))
                        .await;
                }
            }

            let flags_hash = flags_to_hash(&flags);
            if !account.minimal_sync() {
                EmailEnvelopeV3::update_flags(account.id, mailbox_id, uid, &flags, flags_hash)
                    .await?;
            }
            MinimalEnvelope::update_flags(account.id, mailbox_id, uid, flags_hash).await?;
            Self::update_flag_change(account.id, mailbox_id, uid, flags_hash);
        }
        Ok(())
    }

    pub fn get_max_uid(account_id: u64, mailbox_id: u64) -> Option<UID> {
        if let Some(mailboxes) = FLAGS_STATE_MAP.get(&account_id) {
            if let Some(uids_map) = mailboxes.get(&mailbox_id) {
                let max_uid = uids_map.iter().map(|entry| *entry.key()).max();
                return max_uid;
            }
        }
        None
    }

    pub fn count_account_uid_total(account_id: u64) -> usize {
        if let Some(mailboxes) = FLAGS_STATE_MAP.get(&account_id) {
            mailboxes.iter().map(|mailbox| mailbox.value().len()).sum()
        } else {
            0
        }
    }

    // Compare two slices of EnvelopeFlag and return (added, removed)
    fn diff_envelope_flags(
        old_flags: &[EnvelopeFlag],
        new_flags: &[EnvelopeFlag],
    ) -> (Vec<String>, Vec<String>) {
        // Convert EnvelopeFlag to String using Display
        let old_set: HashSet<String> = old_flags.iter().map(|f| f.to_string()).collect();
        let new_set: HashSet<String> = new_flags.iter().map(|f| f.to_string()).collect();

        // Compute added = new - old
        let added: Vec<String> = new_set.difference(&old_set).cloned().collect();

        // Compute removed = old - new
        let removed: Vec<String> = old_set.difference(&new_set).cloned().collect();

        (added, removed)
    }
}

impl Initialize for EnvelopeFlagsManager {
    async fn initialize() -> RustMailerResult<()> {
        EnvelopeFlagsManager::load_state().await
    }
}
