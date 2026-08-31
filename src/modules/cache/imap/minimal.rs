// Copyright © 2025-2026 rustmailer.com
// Licensed under RustMailer License Agreement v1.0
// Unauthorized copying, modification, or distribution is prohibited.

use std::sync::Arc;

use native_db::*;
use native_model::{native_model, Model};
use serde::{Deserialize, Serialize};
use tracing::{error, info};

use crate::{
    modules::{
        cache::imap::{manager::EnvelopeFlagsManager, migration::EmailEnvelopeV3},
        database::{
            batch_delete_impl, batch_insert_impl, enqueue_delete_secondary_impl,
            filter_by_secondary_key_impl, manager::DB_MANAGER, safe_delete::RowFilter, update_impl,
        },
        error::{code::ErrorCode, RustMailerResult},
        utils::envelope_hash,
    },
    raise_error,
};

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
#[native_model(id = 3, version = 1)]
#[native_db(primary_key(pk -> u64))]
pub struct MinimalEnvelope {
    /// The ID of the account owning the email.
    #[secondary_key]
    pub account_id: u64,
    /// The unique identifier of the mailbox where the email is stored (e.g., `MailBox::id`).
    /// Used for indexing to avoid updating indexes when mailboxes are renamed.
    #[secondary_key]
    pub mailbox_id: u64,
    /// The unique identifier (IMAP UID) of the email within the mailbox.
    pub uid: u32,
    /// A hash of the email's flags for efficient comparison or indexing.
    pub flags_hash: u64,
}

impl MinimalEnvelope {
    /// Generates a primary key to ensure ordered storage of email metadata by internal_date.
    pub fn pk(&self) -> u64 {
        envelope_hash(self.account_id, self.mailbox_id, self.uid)
    }

    pub async fn list_by_account(account_id: u64) -> RustMailerResult<Vec<MinimalEnvelope>> {
        filter_by_secondary_key_impl(
            DB_MANAGER.envelope_db(),
            MinimalEnvelopeKey::account_id,
            account_id,
        )
        .await
    }

    pub async fn batch_insert(envelopes: Vec<MinimalEnvelope>) -> RustMailerResult<()> {
        for e in &envelopes {
            EnvelopeFlagsManager::update_flag_change(
                e.account_id,
                e.mailbox_id,
                e.uid,
                e.flags_hash,
            );
        }
        batch_insert_impl(DB_MANAGER.envelope_db(), envelopes).await?;
        Ok(())
    }

    pub async fn clean_mailbox_envelopes(account_id: u64, mailbox_id: u64) -> RustMailerResult<()> {
        const BATCH_SIZE: usize = 50;
        let filter: RowFilter<MinimalEnvelope> =
            Arc::new(move |e: &MinimalEnvelope| e.account_id == account_id);
        enqueue_delete_secondary_impl(
            DB_MANAGER.envelope_db(),
            MinimalEnvelopeKey::mailbox_id,
            mailbox_id,
            filter,
            BATCH_SIZE,
            format!(
                "MinimalEnvelope::clean_mailbox_envelopes account_id={} mailbox_id={}",
                account_id, mailbox_id
            ),
        )?;

        info!(
            "Enqueued deletion of envelopes for mailbox_id={} account_id={}",
            mailbox_id, account_id
        );
        Ok(())
    }

    pub async fn clean_envelopes(
        account_id: u64,
        mailbox_id: u64,
        to_delete_uid: &[u32],
    ) -> RustMailerResult<()> {
        let hashes: Vec<u64> = to_delete_uid
            .iter()
            .map(|uid| envelope_hash(account_id, mailbox_id, *uid))
            .collect();

        let deleted = batch_delete_impl(DB_MANAGER.envelope_db(), move |rw| {
            let mut to_delete = Vec::with_capacity(hashes.len());
            for hash in hashes {
                if let Some(envelope) = rw
                    .get()
                    .primary::<MinimalEnvelope>(hash)
                    .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
                {
                    to_delete.push(envelope);
                }
            }
            Ok(to_delete)
        })
        .await?;

        info!(
            "Deleted {} minimal envelopes for account_id={} mailbox_id={}",
            deleted, account_id, mailbox_id
        );
        Ok(())
    }

    pub async fn update_flags(
        account_id: u64,
        mailbox_id: u64,
        uid: u32,
        flags_hash: u64,
    ) -> RustMailerResult<()> {
        update_impl(
            DB_MANAGER.envelope_db(),
            move |rw| {
                rw.get()
                    .primary::<MinimalEnvelope>(envelope_hash(account_id, mailbox_id, uid))
                    .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
                    .ok_or_else(|| {
                        raise_error!(
                            "The MinimalEnvelope that you want to modify was not found."
                                .to_string(),
                            ErrorCode::ResourceNotFound
                        )
                    })
            },
            move |current| {
                let mut updated = current.clone();
                updated.flags_hash = flags_hash;
                Ok(updated)
            },
        )
        .await
        .map_err(|e| {
            error!(
                "Failed to update flags: account_id={}, mailbox_id={}, uid={}, error={:?}",
                account_id, mailbox_id, uid, e
            );
            e
        })?;

        Ok(())
    }

    pub async fn clean_account(account_id: u64) -> RustMailerResult<()> {
        const BATCH_SIZE: usize = 50;
        let filter: RowFilter<MinimalEnvelope> = Arc::new(|_: &MinimalEnvelope| true);
        enqueue_delete_secondary_impl(
            DB_MANAGER.envelope_db(),
            MinimalEnvelopeKey::account_id,
            account_id,
            filter,
            BATCH_SIZE,
            format!("MinimalEnvelope::clean_account account_id={}", account_id),
        )?;

        info!(
            "Enqueued deletion of envelopes for account_id={}",
            account_id
        );
        Ok(())
    }
}

impl From<&EmailEnvelopeV3> for MinimalEnvelope {
    fn from(value: &EmailEnvelopeV3) -> Self {
        Self {
            account_id: value.account_id,
            mailbox_id: value.mailbox_id,
            uid: value.uid,
            flags_hash: value.flags_hash,
        }
    }
}
