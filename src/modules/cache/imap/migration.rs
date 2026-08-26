// Copyright © 2025-2026 rustmailer.com
// Licensed under RustMailer License Agreement v1.0
// Unauthorized copying, modification, or distribution is prohibited.

use std::{collections::HashSet, sync::Arc};

use native_db::*;
use native_model::{native_model, Model};
use poem_openapi::Object;
use serde::{Deserialize, Serialize};
use tracing::{error, info};

use crate::{
    calculate_hash, id,
    modules::{
        cache::{
            imap::{
                address::AddressEntity,
                envelope::{EmailEnvelope, Received},
                mailbox::EnvelopeFlag,
                manager::EnvelopeFlagsManager,
                minimal::MinimalEnvelope,
                thread::{EmailThread, EmailThreadKey},
            },
            model::Envelope,
        },
        common::Addr,
        database::{
            enqueue_delete_secondary_impl, filter_by_secondary_key_impl, manager::DB_MANAGER,
            paginate_secondary_scan_impl, safe_delete::RowFilter, secondary_find_impl, update_impl,
            with_transaction,
        },
        error::{code::ErrorCode, RustMailerResult},
        imap::section::{EmailBodyPart, ImapAttachment},
        rest::response::DataPage,
        utils::envelope_hash,
    },
    raise_error, utc_now,
};

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize, Object)]
#[native_model(id = 1, version = 2, from = EmailEnvelope)]
#[native_db(primary_key(pk -> String), secondary_key(create_envelope_id -> u64, unique))]
pub struct EmailEnvelopeV2 {
    /// The ID of the account owning the email.
    #[secondary_key]
    pub account_id: u64,
    /// The unique identifier of the mailbox where the email is stored (e.g., `MailBox::id`).
    /// Used for indexing to avoid updating indexes when mailboxes are renamed.
    #[secondary_key]
    pub mailbox_id: u64,
    /// The decoded, human-readable name of the mailbox (e.g., "INBOX", "Sent").
    pub mailbox_name: String,
    /// The unique identifier (IMAP UID) of the email within the mailbox.
    pub uid: u32,
    /// The date and time the email was received by the server, as a Unix timestamp in milliseconds.
    /// If `None`, the internal date is unavailable.
    pub internal_date: Option<i64>,
    /// The size of the email in bytes.
    pub size: u32,
    /// The flags associated with the email (e.g., `\Seen`, `\Answered`, `\Flagged`).
    /// Represented as a list of `EnvelopeFlag` for standard or custom flags.
    pub flags: Vec<EnvelopeFlag>,
    /// A hash of the email's flags for efficient comparison or indexing.
    pub flags_hash: u64,
    /// The blind carbon copy (BCC) recipient(s) of the email, if any.
    pub bcc: Option<Vec<Addr>>,
    /// The carbon copy (CC) recipient(s) of the email, if any.
    pub cc: Option<Vec<Addr>>,
    /// The date the email was sent, as a Unix timestamp in milliseconds, if available.
    pub date: Option<i64>,
    /// The sender's address, including name and email, if available.
    pub from: Option<Addr>,
    /// The message ID of the email to which this email is a reply, if applicable.
    pub in_reply_to: Option<String>,
    /// The actual sender's address, if different from the `from` field.
    pub sender: Option<Addr>,
    /// The return address for undeliverable emails, if specified.
    pub return_address: Option<String>,
    /// The unique message ID of the email, typically used for threading.
    pub message_id: Option<String>,
    /// The subject of the email, if available.
    pub subject: Option<String>,
    /// The name of the thread this email belongs to, if applicable.
    pub thread_name: Option<String>,
    /// The identifier of the thread this email belongs to.
    /// This is computed based on `in_reply_to` / `references` / `message_id`.
    #[secondary_key]
    pub thread_id: u64,
    /// The MIME version of the email (e.g., "1.0"), if specified.
    pub mime_version: Option<String>,
    /// A list of message IDs referenced by this email, used for threading.
    pub references: Option<Vec<String>>,
    /// The address(es) to which replies should be sent, if specified.
    pub reply_to: Option<Vec<Addr>>,
    /// The primary recipient(s) of the email, if any.
    pub to: Option<Vec<Addr>>,
    /// A list of attachments included in the email, if any.
    ///
    /// Each `ImapAttachment` item contains metadata including the part ID and MIME type,
    /// which indicates the exact location of the attachment in the raw message structure.
    /// This allows the backend to directly fetch specific attachments without retrieving
    /// the entire message content.
    ///
    /// This is particularly useful for accounts configured with minimal sync, where full
    /// message bodies are not cached locally. By including this data in the API response,
    /// the client can request to download only the required attachment via a follow-up
    /// API call, improving both efficiency and user experience.
    ///
    /// Developers do not need to understand the internal IMAP part structure — this
    /// metadata provides a clean abstraction for fetching specific attachments.
    pub attachments: Option<Vec<ImapAttachment>>,
    /// Metadata for the email's body parts (e.g., plain text, HTML), if available.
    ///
    /// Each `EmailBodyPart` contains detailed metadata (such as part ID, content type,
    /// and charset) describing a portion of the email body. This enables precise access
    /// to body content, such as plain text or HTML sections, without downloading the full
    /// raw message from the server.
    ///
    /// This is especially helpful for lightweight clients or minimized-sync accounts that
    /// do not cache full email content. The frontend can pass this metadata back to the
    /// server to retrieve only the desired portion of the message (e.g., the HTML body),
    /// which significantly reduces bandwidth and latency.
    ///
    /// By abstracting the complexity of MIME part navigation, developers can efficiently
    /// retrieve specific parts of an email without handling the low-level IMAP structure.
    pub body_meta: Option<Vec<EmailBodyPart>>,
    /// Details about how the email was received, if available.
    pub received: Option<Received>,
}

impl EmailEnvelopeV2 {
    pub fn pk(&self) -> String {
        format!(
            "{}_{}",
            self.internal_date.unwrap_or(utc_now!()),
            envelope_hash(self.account_id, self.mailbox_id, self.uid)
        )
    }

    pub fn create_envelope_id(&self) -> u64 {
        envelope_hash(self.account_id, self.mailbox_id, self.uid)
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize, Object)]
#[native_model(id = 1, version = 3, from = EmailEnvelopeV2)]
#[native_db(primary_key(pk -> String), secondary_key(create_envelope_id -> u64, unique))]
pub struct EmailEnvelopeV3 {
    /// The ID of the account owning the email.
    #[secondary_key]
    pub account_id: u64,
    /// The unique identifier of the mailbox where the email is stored (e.g., `MailBox::id`).
    /// Used for indexing to avoid updating indexes when mailboxes are renamed.
    #[secondary_key]
    pub mailbox_id: u64,
    /// The decoded, human-readable name of the mailbox (e.g., "INBOX", "Sent").
    pub mailbox_name: String,
    /// The unique identifier (IMAP UID) of the email within the mailbox.
    pub uid: u32,
    /// The date and time the email was received by the server, as a Unix timestamp in milliseconds.
    /// If `None`, the internal date is unavailable.
    pub internal_date: Option<i64>,
    /// The size of the email in bytes.
    pub size: u32,
    /// The flags associated with the email (e.g., `\Seen`, `\Answered`, `\Flagged`).
    /// Represented as a list of `EnvelopeFlag` for standard or custom flags.
    pub flags: Vec<EnvelopeFlag>,
    /// A hash of the email's flags for efficient comparison or indexing.
    pub flags_hash: u64,
    /// The blind carbon copy (BCC) recipient(s) of the email, if any.
    pub bcc: Option<Vec<Addr>>,
    /// The carbon copy (CC) recipient(s) of the email, if any.
    pub cc: Option<Vec<Addr>>,
    /// The date the email was sent, as a Unix timestamp in milliseconds, if available.
    pub date: Option<i64>,
    /// The sender's address, including name and email, if available.
    pub from: Option<Addr>,
    /// The message ID of the email to which this email is a reply, if applicable.
    pub in_reply_to: Option<String>,
    /// The actual sender's address, if different from the `from` field.
    pub sender: Option<Addr>,
    /// The return address for undeliverable emails, if specified.
    pub return_address: Option<String>,
    /// The unique message ID of the email, typically used for threading.
    pub message_id: Option<String>,
    /// The subject of the email, if available.
    pub subject: Option<String>,
    /// The name of the thread this email belongs to, if applicable.
    pub thread_name: Option<String>,
    /// The identifier of the thread this email belongs to.
    /// This is computed based on `in_reply_to` / `references` / `message_id`.
    #[secondary_key]
    pub thread_id: u64,
    /// The MIME version of the email (e.g., "1.0"), if specified.
    pub mime_version: Option<String>,
    /// A list of message IDs referenced by this email, used for threading.
    pub references: Option<Vec<String>>,
    /// The address(es) to which replies should be sent, if specified.
    pub reply_to: Option<Vec<Addr>>,
    /// The primary recipient(s) of the email, if any.
    pub to: Option<Vec<Addr>>,
    /// A list of attachments included in the email, if any.
    ///
    /// Each `ImapAttachment` item contains metadata including the part ID and MIME type,
    /// which indicates the exact location of the attachment in the raw message structure.
    /// This allows the backend to directly fetch specific attachments without retrieving
    /// the entire message content.
    ///
    /// This is particularly useful for accounts configured with minimal sync, where full
    /// message bodies are not cached locally. By including this data in the API response,
    /// the client can request to download only the required attachment via a follow-up
    /// API call, improving both efficiency and user experience.
    ///
    /// Developers do not need to understand the internal IMAP part structure — this
    /// metadata provides a clean abstraction for fetching specific attachments.
    pub attachments: Option<Vec<ImapAttachment>>,
    /// Metadata for the email's body parts (e.g., plain text, HTML), if available.
    ///
    /// Each `EmailBodyPart` contains detailed metadata (such as part ID, content type,
    /// and charset) describing a portion of the email body. This enables precise access
    /// to body content, such as plain text or HTML sections, without downloading the full
    /// raw message from the server.
    ///
    /// This is especially helpful for lightweight clients or minimized-sync accounts that
    /// do not cache full email content. The frontend can pass this metadata back to the
    /// server to retrieve only the desired portion of the message (e.g., the HTML body),
    /// which significantly reduces bandwidth and latency.
    ///
    /// By abstracting the complexity of MIME part navigation, developers can efficiently
    /// retrieve specific parts of an email without handling the low-level IMAP structure.
    pub body_meta: Option<Vec<EmailBodyPart>>,
    /// Details about how the email was received, if available.
    pub received: Option<Received>,
    /// The `mid` field is reserved for potential integration with other backend models.
    /// For instance, it can be used to store the email index or ID from external services like the Gmail API.
    /// This ID could be used for reference or identification purposes in scenarios where an external service
    /// provides an identifier for the email in question.
    ///
    /// This field is optional, meaning that it may be `None` if no external service identifier is available.
    pub mid: Option<String>,
    /// A list of labels applied to the message.
    ///
    /// Each element is a string representing a Gmail label name (e.g., "INBOX", "UNREAD").
    /// This field reflects the current labels associated with the email.
    ///
    /// Note: This field is populated only for Gmail API accounts. For other account types, it will be empty.
    pub labels: Vec<String>,
}

impl EmailEnvelopeV3 {
    pub fn pk(&self) -> String {
        format!(
            "{}_{}",
            self.internal_date.unwrap_or(utc_now!()),
            envelope_hash(self.account_id, self.mailbox_id, self.uid)
        )
    }

    pub fn create_envelope_id(&self) -> u64 {
        envelope_hash(self.account_id, self.mailbox_id, self.uid)
    }

    pub fn compute_thread_id(&self) -> u64 {
        if self.in_reply_to.is_some() && self.references.as_ref().map_or(false, |r| !r.is_empty()) {
            return calculate_hash!(&self.references.as_ref().unwrap()[0]);
        }
        if let Some(message_id) = self.message_id.as_ref() {
            return calculate_hash!(message_id);
        }
        id!(128)
    }

    pub async fn find(
        account_id: u64,
        mailbox_id: u64,
        uid: u32,
    ) -> RustMailerResult<Option<EmailEnvelopeV3>> {
        secondary_find_impl(
            DB_MANAGER.envelope_db(),
            EmailEnvelopeV3Key::create_envelope_id,
            envelope_hash(account_id, mailbox_id, uid),
        )
        .await
    }

    pub async fn get_thread(account_id: u64, thread_id: u64) -> RustMailerResult<Vec<Envelope>> {
        let envelopes = filter_by_secondary_key_impl::<EmailEnvelopeV3>(
            DB_MANAGER.envelope_db(),
            EmailEnvelopeV3Key::thread_id,
            thread_id,
        )
        .await?;

        let mut result = Vec::with_capacity(envelopes.len());
        for e in envelopes {
            if e.account_id == account_id {
                result.push(e);
            }
        }
        // Sort by internal_date in descending order
        result.sort_by(|a, b| {
            match (a.internal_date, b.internal_date) {
                (Some(da), Some(db)) => db.cmp(&da),         // Descending
                (Some(_), None) => std::cmp::Ordering::Less, // None is smaller
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            }
        });

        Ok(result.into_iter().map(Envelope::from).collect())
    }

    pub async fn get(envelope_id: u64) -> RustMailerResult<Option<EmailEnvelopeV3>> {
        secondary_find_impl(
            DB_MANAGER.envelope_db(),
            EmailEnvelopeV3Key::create_envelope_id,
            envelope_id,
        )
        .await
    }

    pub async fn save_envelopes(envelopes: Vec<EmailEnvelopeV3>) -> RustMailerResult<()> {
        with_transaction(DB_MANAGER.envelope_db(), move |rw| {
            for mut e in envelopes {
                // --- Preprocessing ---
                let minimal = MinimalEnvelope::from(&e);
                EnvelopeFlagsManager::update_flag_change(
                    e.account_id,
                    e.mailbox_id,
                    e.uid,
                    e.flags_hash,
                );
                let address_entities = AddressEntity::extract(&e);
                e.thread_id = e.compute_thread_id();

                let thread = EmailThread::new(
                    e.thread_id,
                    e.create_envelope_id(),
                    e.account_id,
                    e.mailbox_id,
                    e.internal_date,
                    e.date,
                );

                // --- Store full & minimal envelope ---
                rw.insert::<EmailEnvelopeV3>(e)
                    .map_err(|err| raise_error!(format!("{:#?}", err), ErrorCode::InternalError))?;
                rw.insert::<MinimalEnvelope>(minimal)
                    .map_err(|err| raise_error!(format!("{:#?}", err), ErrorCode::InternalError))?;

                // --- Thread upsert ---
                match rw
                    .get()
                    .secondary::<EmailThread>(EmailThreadKey::thread_id, thread.thread_id)
                    .map_err(|err| raise_error!(format!("{:#?}", err), ErrorCode::InternalError))?
                {
                    Some(current) => {
                        // Only replace if current.internal_date is older than new internal_date
                        if current.need_update(&thread) {
                            rw.remove(current).map_err(|err| {
                                raise_error!(format!("{:#?}", err), ErrorCode::InternalError)
                            })?;
                            rw.insert::<EmailThread>(thread).map_err(|err| {
                                raise_error!(format!("{:#?}", err), ErrorCode::InternalError)
                            })?;
                        }
                    }
                    None => {
                        rw.insert::<EmailThread>(thread).map_err(|err| {
                            raise_error!(format!("{:#?}", err), ErrorCode::InternalError)
                        })?;
                    }
                }

                // --- Store address entities ---
                for addr in address_entities {
                    rw.insert::<AddressEntity>(addr).map_err(|err| {
                        raise_error!(format!("{:#?}", err), ErrorCode::InternalError)
                    })?;
                }
            }
            Ok(())
        })
        .await
    }

    pub async fn list_messages_in_mailbox(
        mailbox_id: u64,
        page: u64,
        page_size: u64,
        desc: bool,
    ) -> RustMailerResult<DataPage<EmailEnvelopeV3>> {
        paginate_secondary_scan_impl(
            DB_MANAGER.envelope_db(),
            Some(page),
            Some(page_size),
            Some(desc),
            EmailEnvelopeV3Key::mailbox_id,
            mailbox_id,
        )
        .await
        .map(DataPage::from)
    }

    pub async fn update_flags(
        account_id: u64,
        mailbox_id: u64,
        uid: u32,
        flags: &[EnvelopeFlag],
        flags_hash: u64,
    ) -> RustMailerResult<()> {
        let flags = flags.to_vec();

        update_impl(
            DB_MANAGER.envelope_db(),
            move |rw| {
                rw.get()
                    .secondary::<EmailEnvelopeV3>(
                        EmailEnvelopeV3Key::create_envelope_id,
                        envelope_hash(account_id, mailbox_id, uid),
                    )
                    .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
                    .ok_or_else(|| {
                        raise_error!(
                            "The EmailEnvelope that you want to modify was not found.".to_string(),
                            ErrorCode::ResourceNotFound
                        )
                    })
            },
            move |current| {
                let mut updated = current.clone();
                updated.flags = flags;
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

    pub async fn clean_mailbox_envelopes(account_id: u64, mailbox_id: u64) -> RustMailerResult<()> {
        const BATCH_SIZE: usize = 50;
        let filter: RowFilter<EmailEnvelopeV3> =
            Arc::new(move |e: &EmailEnvelopeV3| e.account_id == account_id);
        enqueue_delete_secondary_impl(
            DB_MANAGER.envelope_db(),
            EmailEnvelopeV3Key::mailbox_id,
            mailbox_id,
            filter,
            BATCH_SIZE,
            format!(
                "EmailEnvelopeV3::clean_mailbox_envelopes account_id={} mailbox_id={}",
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
        const BATCH_SIZE: usize = 50;
        let to_delete_set: HashSet<u32> = to_delete_uid.iter().copied().collect();
        let to_delete_set = Arc::new(to_delete_set);
        let filter: RowFilter<EmailEnvelopeV3> = Arc::new(move |e: &EmailEnvelopeV3| {
            e.account_id == account_id && to_delete_set.contains(&e.uid)
        });
        enqueue_delete_secondary_impl(
            DB_MANAGER.envelope_db(),
            EmailEnvelopeV3Key::mailbox_id,
            mailbox_id,
            filter,
            BATCH_SIZE,
            format!(
                "EmailEnvelopeV3::clean_envelopes account_id={} mailbox_id={}",
                account_id, mailbox_id
            ),
        )?;

        info!(
            "Enqueued deletion of envelopes for account_id={} mailbox_id={}",
            account_id, mailbox_id
        );
        Ok(())
    }

    pub async fn clean_account(account_id: u64) -> RustMailerResult<()> {
        const BATCH_SIZE: usize = 50;
        let filter: RowFilter<EmailEnvelopeV3> = Arc::new(|_: &EmailEnvelopeV3| true);
        enqueue_delete_secondary_impl(
            DB_MANAGER.envelope_db(),
            EmailEnvelopeV3Key::account_id,
            account_id,
            filter,
            BATCH_SIZE,
            format!(
                "EmailEnvelopeV3::clean_account account_id={}",
                account_id
            ),
        )?;

        info!(
            "Enqueued deletion of envelopes for account_id={}",
            account_id
        );
        Ok(())
    }
}

impl From<EmailEnvelope> for EmailEnvelopeV2 {
    fn from(value: EmailEnvelope) -> Self {
        Self {
            account_id: value.account_id,
            mailbox_id: value.mailbox_id,
            mailbox_name: value.mailbox_name,
            uid: value.uid,
            internal_date: value.internal_date,
            size: value.size,
            flags: value.flags,
            flags_hash: value.flags_hash,
            bcc: value.bcc,
            cc: value.cc,
            date: value.date,
            from: value.from,
            in_reply_to: value.in_reply_to,
            sender: value.sender,
            return_address: value.return_address,
            message_id: value.message_id,
            subject: value.subject,
            thread_name: value.thread_name,
            thread_id: id!(64),
            mime_version: value.mime_version,
            references: value.references,
            reply_to: value.reply_to,
            to: value.to,
            attachments: value.attachments,
            body_meta: value.body_meta,
            received: value.received,
        }
    }
}

impl From<EmailEnvelopeV2> for EmailEnvelope {
    fn from(value: EmailEnvelopeV2) -> Self {
        Self {
            account_id: value.account_id,
            mailbox_id: value.mailbox_id,
            mailbox_name: value.mailbox_name,
            uid: value.uid,
            internal_date: value.internal_date,
            size: value.size,
            flags: value.flags,
            flags_hash: value.flags_hash,
            bcc: value.bcc,
            cc: value.cc,
            date: value.date,
            from: value.from,
            in_reply_to: value.in_reply_to,
            sender: value.sender,
            return_address: value.return_address,
            message_id: value.message_id,
            subject: value.subject,
            thread_name: value.thread_name,
            mime_version: value.mime_version,
            references: value.references,
            reply_to: value.reply_to,
            to: value.to,
            attachments: value.attachments,
            body_meta: value.body_meta,
            received: value.received,
        }
    }
}

impl From<EmailEnvelopeV2> for EmailEnvelopeV3 {
    fn from(value: EmailEnvelopeV2) -> Self {
        Self {
            account_id: value.account_id,
            mailbox_id: value.mailbox_id,
            mailbox_name: value.mailbox_name,
            uid: value.uid,
            internal_date: value.internal_date,
            size: value.size,
            flags: value.flags,
            flags_hash: value.flags_hash,
            bcc: value.bcc,
            cc: value.cc,
            date: value.date,
            from: value.from,
            in_reply_to: value.in_reply_to,
            sender: value.sender,
            return_address: value.return_address,
            message_id: value.message_id,
            subject: value.subject,
            thread_name: value.thread_name,
            thread_id: value.thread_id,
            mime_version: value.mime_version,
            references: value.references,
            reply_to: value.reply_to,
            to: value.to,
            attachments: value.attachments,
            body_meta: value.body_meta,
            received: value.received,
            mid: None,
            labels: vec![],
        }
    }
}

impl From<EmailEnvelopeV3> for EmailEnvelopeV2 {
    fn from(value: EmailEnvelopeV3) -> Self {
        Self {
            account_id: value.account_id,
            mailbox_id: value.mailbox_id,
            mailbox_name: value.mailbox_name,
            uid: value.uid,
            internal_date: value.internal_date,
            size: value.size,
            flags: value.flags,
            flags_hash: value.flags_hash,
            bcc: value.bcc,
            cc: value.cc,
            date: value.date,
            from: value.from,
            in_reply_to: value.in_reply_to,
            sender: value.sender,
            return_address: value.return_address,
            message_id: value.message_id,
            subject: value.subject,
            thread_name: value.thread_name,
            thread_id: value.thread_id,
            mime_version: value.mime_version,
            references: value.references,
            reply_to: value.reply_to,
            to: value.to,
            attachments: value.attachments,
            body_meta: value.body_meta,
            received: value.received,
        }
    }
}
