// Copyright © 2025-2026 rustmailer.com
// Licensed under RustMailer License Agreement v1.0
// Unauthorized copying, modification, or distribution is prohibited.

use std::sync::Arc;

use native_db::*;
use native_model::{native_model, Model};
use poem_openapi::Object;
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::{
    id,
    modules::{
        cache::{
            imap::migration::EmailEnvelopeV3,
            vendor::{
                gmail::sync::envelope::GmailEnvelope, outlook::sync::envelope::OutlookEnvelope,
            },
        },
        database::{
            batch_delete_impl, enqueue_delete_secondary_impl, filter_by_secondary_key_impl,
            manager::DB_MANAGER, safe_delete::RowFilter,
        },
        error::{code::ErrorCode, RustMailerResult},
        utils::envelope_hash,
    },
    raise_error,
};

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize, Object)]
#[native_model(id = 4, version = 1)]
#[native_db]
pub struct AddressEntity {
    #[primary_key]
    pub id: u64,
    #[secondary_key]
    pub account_id: u64,
    #[secondary_key]
    pub mailbox_id: u64,
    #[secondary_key(optional)]
    pub from: Option<String>,
    #[secondary_key(optional)]
    pub to: Option<String>,
    #[secondary_key(optional)]
    pub cc: Option<String>,
    #[secondary_key]
    pub envelope_hash: u64,
    pub date: Option<i64>,
    pub internal_date: Option<i64>,
}

impl AddressEntity {
    pub async fn from(email: &str) -> RustMailerResult<Vec<AddressEntity>> {
        filter_by_secondary_key_impl::<AddressEntity>(
            DB_MANAGER.envelope_db(),
            AddressEntityKey::from,
            Some(email.to_string()),
        )
        .await
    }

    pub async fn to(email: &str) -> RustMailerResult<Vec<AddressEntity>> {
        filter_by_secondary_key_impl::<AddressEntity>(
            DB_MANAGER.envelope_db(),
            AddressEntityKey::to,
            Some(email.to_string()),
        )
        .await
    }

    pub async fn cc(email: &str) -> RustMailerResult<Vec<AddressEntity>> {
        filter_by_secondary_key_impl::<AddressEntity>(
            DB_MANAGER.envelope_db(),
            AddressEntityKey::cc,
            Some(email.to_string()),
        )
        .await
    }

    pub async fn clean_account(account_id: u64) -> RustMailerResult<()> {
        const BATCH_SIZE: usize = 50;
        let filter: RowFilter<AddressEntity> = Arc::new(|_: &AddressEntity| true);
        enqueue_delete_secondary_impl(
            DB_MANAGER.envelope_db(),
            AddressEntityKey::account_id,
            account_id,
            filter,
            BATCH_SIZE,
            format!("AddressEntity::clean_account account_id={}", account_id),
        )?;

        info!(
            "Enqueued deletion of address entities for account_id={}",
            account_id
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
            let mut to_delete = Vec::new();
            for hash in hashes {
                let entities: Vec<AddressEntity> = rw
                    .scan()
                    .secondary(AddressEntityKey::envelope_hash)
                    .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
                    .start_with(hash)
                    .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
                    .filter_map(Result::ok)
                    .collect();
                to_delete.extend(entities);
            }
            Ok(to_delete)
        })
        .await?;

        info!(
            "Deleted {} address entities for mailbox_id={} account_id={}",
            deleted, mailbox_id, account_id
        );
        Ok(())
    }

    pub async fn clean_mailbox_envelopes(account_id: u64, mailbox_id: u64) -> RustMailerResult<()> {
        const BATCH_SIZE: usize = 50;
        let filter: RowFilter<AddressEntity> =
            Arc::new(move |e: &AddressEntity| e.account_id == account_id);
        enqueue_delete_secondary_impl(
            DB_MANAGER.envelope_db(),
            AddressEntityKey::mailbox_id,
            mailbox_id,
            filter,
            BATCH_SIZE,
            format!(
                "AddressEntity::clean_mailbox_envelopes account_id={} mailbox_id={}",
                account_id, mailbox_id
            ),
        )?;

        info!(
            "Enqueued deletion of address entities for mailbox_id={} account_id={}",
            mailbox_id, account_id
        );
        Ok(())
    }

    pub fn extract(envelope: &EmailEnvelopeV3) -> Vec<AddressEntity> {
        let from = envelope.from.as_ref().map(|f| f.address.clone()).flatten();
        let envelope_hash = envelope.create_envelope_id();
        let date = envelope.date.clone();
        let internal_date = envelope.internal_date.clone();
        let account_id = envelope.account_id;
        let mailbox_id = envelope.mailbox_id;
        let mut entities = Vec::new();

        match (&envelope.to, &envelope.cc) {
            (None, None) => {
                entities.push(AddressEntity {
                    account_id,
                    mailbox_id,
                    id: id!(96),
                    from,
                    to: None,
                    cc: None,
                    envelope_hash,
                    date,
                    internal_date,
                });
            }
            (None, Some(cc)) => {
                entities.extend(cc.iter().map(|c| {
                    let from = from.clone();
                    AddressEntity {
                        account_id,
                        mailbox_id,
                        id: id!(96),
                        from,
                        to: None,
                        cc: c.address.clone(),
                        envelope_hash,
                        date: date.clone(),
                        internal_date: internal_date.clone(),
                    }
                }));
            }
            (Some(to), None) => {
                entities.extend(to.iter().map(|t| {
                    let from = from.clone();
                    AddressEntity {
                        account_id,
                        mailbox_id,
                        id: id!(96),
                        from,
                        to: t.address.clone(),
                        cc: None,
                        envelope_hash,
                        date: date.clone(),
                        internal_date: internal_date.clone(),
                    }
                }));
            }
            (Some(to), Some(cc)) => {
                entities.extend(to.iter().flat_map(|t| {
                    let from = from.clone();
                    cc.iter().map(move |c| AddressEntity {
                        account_id,
                        mailbox_id,
                        id: id!(96),
                        from: from.clone(),
                        to: t.address.clone(),
                        cc: c.address.clone(),
                        envelope_hash,
                        date: date.clone(),
                        internal_date: internal_date.clone(),
                    })
                }));
            }
        }

        entities
    }

    pub fn extract2(envelope: &GmailEnvelope) -> Vec<AddressEntity> {
        let from = envelope.from.as_ref().map(|f| f.address.clone()).flatten();
        let envelope_hash = envelope.create_envelope_id();
        let date = envelope.date.clone();
        let internal_date = Some(envelope.internal_date.clone());
        let account_id = envelope.account_id;
        let mailbox_id = envelope.label_id;
        let mut entities = Vec::new();

        match (&envelope.to, &envelope.cc) {
            (None, None) => {
                entities.push(AddressEntity {
                    account_id,
                    mailbox_id,
                    id: id!(96),
                    from: from,
                    to: None,
                    cc: None,
                    envelope_hash,
                    date,
                    internal_date,
                });
            }
            (None, Some(cc)) => {
                entities.extend(cc.iter().map(|c| {
                    let from = from.clone();
                    AddressEntity {
                        account_id,
                        mailbox_id,
                        id: id!(96),
                        from,
                        to: None,
                        cc: c.address.clone(),
                        envelope_hash,
                        date: date.clone(),
                        internal_date: internal_date.clone(),
                    }
                }));
            }
            (Some(to), None) => {
                entities.extend(to.iter().map(|t| {
                    let from = from.clone();
                    AddressEntity {
                        account_id,
                        mailbox_id,
                        id: id!(96),
                        from,
                        to: t.address.clone(),
                        cc: None,
                        envelope_hash,
                        date: date.clone(),
                        internal_date: internal_date.clone(),
                    }
                }));
            }
            (Some(to), Some(cc)) => {
                entities.extend(to.iter().flat_map(|t| {
                    let from = from.clone();
                    cc.iter().map(move |c| AddressEntity {
                        account_id,
                        mailbox_id,
                        id: id!(96),
                        from: from.clone(),
                        to: t.address.clone(),
                        cc: c.address.clone(),
                        envelope_hash,
                        date: date.clone(),
                        internal_date: internal_date.clone(),
                    })
                }));
            }
        }

        entities
    }

    pub fn extract3(envelope: &OutlookEnvelope) -> Vec<AddressEntity> {
        let from = envelope.from.as_ref().map(|f| f.address.clone()).flatten();
        let envelope_hash = envelope.create_envelope_id();
        let date = envelope.date.clone();
        let internal_date = envelope.internal_date.clone();
        let account_id = envelope.account_id;
        let mailbox_id = envelope.folder_id;
        let mut entities = Vec::new();

        match (&envelope.to, &envelope.cc) {
            (None, None) => {
                entities.push(AddressEntity {
                    account_id,
                    mailbox_id,
                    id: id!(96),
                    from: from,
                    to: None,
                    cc: None,
                    envelope_hash,
                    date,
                    internal_date,
                });
            }
            (None, Some(cc)) => {
                entities.extend(cc.iter().map(|c| {
                    let from = from.clone();
                    AddressEntity {
                        account_id,
                        mailbox_id,
                        id: id!(96),
                        from,
                        to: None,
                        cc: c.address.clone(),
                        envelope_hash,
                        date: date.clone(),
                        internal_date: internal_date.clone(),
                    }
                }));
            }
            (Some(to), None) => {
                entities.extend(to.iter().map(|t| {
                    let from = from.clone();
                    AddressEntity {
                        account_id,
                        mailbox_id,
                        id: id!(96),
                        from,
                        to: t.address.clone(),
                        cc: None,
                        envelope_hash,
                        date: date.clone(),
                        internal_date: internal_date.clone(),
                    }
                }));
            }
            (Some(to), Some(cc)) => {
                entities.extend(to.iter().flat_map(|t| {
                    let from = from.clone();
                    cc.iter().map(move |c| AddressEntity {
                        account_id,
                        mailbox_id,
                        id: id!(96),
                        from: from.clone(),
                        to: t.address.clone(),
                        cc: c.address.clone(),
                        envelope_hash,
                        date: date.clone(),
                        internal_date: internal_date.clone(),
                    })
                }));
            }
        }

        entities
    }
}
