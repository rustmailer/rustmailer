// Copyright © 2025-2026 rustmailer.com
// Licensed under RustMailer License Agreement v1.0
// Unauthorized copying, modification, or distribution is prohibited.

use crate::modules::account::entity::MailerType;
use crate::modules::cache::vendor::gmail::model::messages::PartBody;
use crate::modules::cache::vendor::gmail::sync::client::GmailClient;
use crate::modules::cache::vendor::outlook::sync::client::OutlookClient;
use crate::modules::error::code::ErrorCode;
use crate::modules::message::content::{AttachmentInfo, FullMessageContent};
use crate::modules::message::get_minimal_meta;
use crate::{base64_decode, base64_decode_url_safe};
use crate::{
    encode_mailbox_name,
    modules::account::migration::AccountModel,
    modules::cache::disk::DISK_CACHE,
    modules::context::executors::RUST_MAIL_CONTEXT,
    modules::error::RustMailerResult,
    modules::imap::section::{ImapAttachment, SegmentPath},
    raise_error,
};
use poem_openapi::Object;
use serde::{Deserialize, Serialize};

const MAX_ATTACHMENT_SIZE: usize = 52_428_800; // 50MB

/// Represents a request to fetch an attachment from a message in a mailbox.
#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize, Object)]
pub struct AttachmentRequest {
    /// IMAP only: The name of the mailbox where the message is located.
    /// Not used for Gmail/Graph API accounts.
    pub mailbox: Option<String>,
    /// IMAP only: The metadata describing the attachment to fetch.
    /// Not used for Gmail/Graph API accounts.
    pub attachment: Option<ImapAttachment>,
    /// The unique ID of the message, either IMAP UID or Gmail API MID.
    ///
    /// - For IMAP accounts, this is the UID converted to a string. It must be a valid numeric string
    ///   that can be parsed back to a `u32`.
    /// - For Gmail/Graph API accounts, this is the message ID (`mid`) returned by the API.
    pub id: String,
    /// Gmail API only: attachment info used to fetch it via Gmail API.
    /// Not used for IMAP accounts.
    pub attachment_info: Option<AttachmentInfo>,
    /// Optional: The original filename of the attachment, if available.  
    /// - Gmail API only.  
    pub filename: Option<String>,
    /// Graph API only: attachment id used to fetch it via Graph API.
    /// Not used for IMAP accounts.
    pub attachment_id: Option<String>,
}

impl AttachmentRequest {
    pub fn validate(&self, account: &AccountModel) -> RustMailerResult<()> {
        match account.mailer_type {
            MailerType::ImapSmtp => {
                if self.mailbox.is_none() || self.attachment.is_none() {
                    return Err(raise_error!(
                        format!(
                            "Current account type is `ImapSmtp`. Downloading attachments requires `uid`, `mailbox`, and `attachment` metadata."
                        ),
                        ErrorCode::InvalidParameter
                    ));
                }
                if self.id.parse::<u32>().is_err() {
                    return Err(raise_error!(
                        "Invalid IMAP UID: `id` must be a numeric string".into(),
                        ErrorCode::InvalidParameter
                    ));
                }
            }
            MailerType::GmailApi => {
                if self.attachment_info.is_none() {
                    return Err(raise_error!(
                        "Current account type is `Gmail API`. Downloading attachments requires `attachment_info`.".into(),
                        ErrorCode::InvalidParameter
                    ));
                }
            }
            MailerType::GraphApi => {
                if self.attachment_id.is_none() {
                    return Err(raise_error!(
                        "Current account type is `Graph API`. Downloading attachments requires `attachment_id`.".into(),
                        ErrorCode::InvalidParameter
                    ));
                }
            }
        }
        Ok(())
    }
}

pub fn attachment_diskcache_key(
    account_id: u64,
    mailbox_name: &str,
    uid: u32,
    segment_path: SegmentPath,
) -> String {
    format!(
        "email_attachment_{}_{}_{}_{}",
        account_id, mailbox_name, uid, segment_path
    )
}

pub fn inline_attachment_diskcache_key(
    account_id: u64,
    mailbox_name: &str,
    uid: u32,
    segment_path: SegmentPath,
) -> String {
    format!(
        "email_inline_attachment_{}_{}_{}_{}",
        account_id, mailbox_name, uid, segment_path
    )
}

pub fn gmail_attachment_diskcache_key(
    account_id: u64,
    mid: &str,
    attachment_info: &AttachmentInfo,
) -> String {
    format!(
        "gmail_attachment_{}_{}_{}",
        account_id,
        mid,
        attachment_info.hash()
    )
}

pub fn graph_attachment_diskcache_key(account_id: u64, mid: &str, attachment_id: &str) -> String {
    format!("graph_attachment_{}_{}_{}", account_id, mid, attachment_id)
}

pub fn gmail_inline_attachment_diskcache_key(
    account_id: u64,
    mid: &str,
    attachment_info: &AttachmentInfo,
) -> String {
    format!(
        "gmail_inline_attachment_{}_{}_{}",
        account_id,
        mid,
        attachment_info.hash()
    )
}

pub async fn retrieve_email_attachment(
    account_id: u64,
    request: AttachmentRequest,
) -> RustMailerResult<(cacache::Reader, Option<String>)> {
    let account = AccountModel::get(account_id).await?;
    if !account.enabled {
        return Err(raise_error!(
            format!("Account id='{account_id}' is disabled"),
            ErrorCode::AccountDisabled
        ));
    }
    request.validate(&account)?;
    match account.mailer_type {
        MailerType::ImapSmtp => {
            let mut attachment = request.attachment.ok_or_else(|| {
                raise_error!(
                    "`attachment` is required when retrieving attachments for IMAP/SMTP accounts."
                        .into(),
                    ErrorCode::InvalidParameter
                )
            })?;
            let mailbox = request.mailbox.ok_or_else(|| {
                raise_error!(
                    "`mailbox` is required when retrieving attachments for IMAP/SMTP accounts."
                        .into(),
                    ErrorCode::InvalidParameter
                )
            })?;
            let uid = request.id.parse::<u32>().ok().ok_or_else(|| {
                raise_error!(
                    "Invalid IMAP UID: `id` must be a numeric string".into(),
                    ErrorCode::InvalidParameter
                )
            })?;
            let filename = attachment.filename.take();
            let reader = retrieve_imap_attachment(account_id, attachment, mailbox, uid).await?;
            Ok((reader, filename))
        }
        MailerType::GmailApi => {
            let attachment_info = request.attachment_info.as_ref().ok_or_else(|| {
                raise_error!(
                    "`attachment_info` is required when retrieving attachments for Gmail API accounts.".into(),
                    ErrorCode::InvalidParameter
                )
            })?;
            let filename = request.filename;
            let reader = retrieve_gmail_attachment(&account, &request.id, &attachment_info).await?;
            Ok((reader, filename))
        }
        MailerType::GraphApi => {
            let reader =
                retrieve_graph_attachment(&account, &request.id, &request.attachment_id.unwrap())
                    .await?;
            Ok((reader, request.filename))
        }
    }
}

async fn retrieve_imap_attachment(
    account_id: u64,
    attachment: ImapAttachment,
    mailbox: String,
    uid: u32,
) -> RustMailerResult<cacache::Reader> {
    if attachment.size >= MAX_ATTACHMENT_SIZE {
        return Err(raise_error!(
            format!(
                "Attachment size {} bytes exceeds the maximum allowed size of {} bytes",
                attachment.size, MAX_ATTACHMENT_SIZE
            ),
            ErrorCode::ExceedsLimitation
        ));
    }

    let cache_key = attachment_diskcache_key(account_id, &mailbox, uid, attachment.path.clone());
    if let Some(reader) = DISK_CACHE.get_cache(&cache_key).await? {
        return Ok(reader);
    }

    let meta = get_minimal_meta(account_id, &mailbox, uid).await?;
    let attachments = meta.attachments.ok_or_else(|| {
        raise_error!(
            "No attachments found in the message".into(),
            ErrorCode::ResourceNotFound
        )
    })?;

    let target = attachments
        .into_iter()
        .find(|a| a.path == attachment.path)
        .ok_or_else(|| {
            raise_error!(
                format!("Attachment not found with path: {}", attachment.path),
                ErrorCode::ResourceNotFound
            )
        })?;

    if target.size >= MAX_ATTACHMENT_SIZE {
        return Err(raise_error!(format!(
            "Attachment size {} bytes exceeds the maximum allowed size of {} bytes. Attachment path: {}",
            target.size,
            MAX_ATTACHMENT_SIZE,
            target.path
        ), ErrorCode::ExceedsLimitation));
    }

    let executor = RUST_MAIL_CONTEXT.imap(account_id).await?;
    // Fetch the attachment from the server
    let result = executor
        .uid_fetch_single_part(
            &uid.to_string(),
            &encode_mailbox_name!(&mailbox),
            &attachment.path.to_string(),
        )
        .await?;
    // Find the corresponding result
    let target = result.iter().find(|f| f.uid == Some(uid)).ok_or_else(|| {
        raise_error!(
            "Failed to fetch attachment from server".into(),
            ErrorCode::InternalError
        )
    })?;
    // Decode the attachment
    let decoded = attachment.decode(target).ok_or_else(|| {
        raise_error!(
            "Failed to parse attachment content from result".into(),
            ErrorCode::InternalError
        )
    })?;
    // Cache the result and return it
    DISK_CACHE.put_cache(&cache_key, &decoded, false).await?;

    // Cache the original inline attachment for replace cid with attachment content
    if attachment.inline {
        let encoded = attachment.encoded(target).ok_or_else(|| {
            raise_error!(
                "Failed to parse inline attachment content from result".into(),
                ErrorCode::InternalError
            )
        })?;
        let inline_cache_key =
            inline_attachment_diskcache_key(account_id, &mailbox, uid, attachment.path.clone());
        DISK_CACHE
            .put_cache(&inline_cache_key, &encoded, false)
            .await?;
    }

    DISK_CACHE
        .get_cache(&cache_key)
        .await?
        .ok_or_else(|| raise_error!("Unexpected cache miss".into(), ErrorCode::InternalError))
}

async fn retrieve_gmail_attachment(
    account: &AccountModel,
    mid: &str,
    attachment_info: &AttachmentInfo,
) -> RustMailerResult<cacache::Reader> {
    let cache_key = gmail_attachment_diskcache_key(account.id, mid, attachment_info);
    if let Some(reader) = DISK_CACHE.get_cache(&cache_key).await? {
        return Ok(reader);
    }

    //Fetching a full message twice for attachment details may yield different ids for the same attachment when checking its size against the safety threshold.
    let full_message = GmailClient::get_full_messages(account.id, account.use_proxy, mid).await?;
    let message: FullMessageContent = full_message.try_into()?;
    let attachments = message.attachments.as_ref().ok_or_else(|| {
        raise_error!(
            "Expected attachments metadata from Gmail API, but none was found.".into(),
            ErrorCode::InternalError
        )
    })?;

    let target_hash = attachment_info.hash();
    //Attachments cannot be compared using the id obtained from two separate fetches, because they may differ.
    //In other words, the Gmail API may return different ids for the same attachment content, but local caching cannot rely on these ids;
    //instead, a hash should be calculated based on the attachment's basic information.
    let attachment = attachments
        .iter()
        .find(|att| att.hash() == target_hash)
        .ok_or_else(|| {
            raise_error!(
                "Attachment not found in Gmail message metadata.".into(),
                ErrorCode::ResourceNotFound
            )
        })?;

    if attachment.size as usize >= MAX_ATTACHMENT_SIZE {
        return Err(raise_error!(
            format!(
                "Attachment size {} bytes exceeds the maximum allowed size of {} bytes",
                attachment.size, MAX_ATTACHMENT_SIZE
            )
            .into(),
            ErrorCode::ExceedsLimitation
        ));
    }

    let body =
        GmailClient::get_attachment(account.id, account.use_proxy, mid, &attachment.id).await?;

    match body {
        PartBody::Body { data, .. } => {
            let decoded = base64_decode_url_safe!(&data).map_err(|e| {
                raise_error!(
                    format!("Failed to decode base64_content: {}", e),
                    ErrorCode::InternalError
                )
            })?;
            DISK_CACHE.put_cache(&cache_key, &decoded, false).await?;
            //Inline attachments directly cache the Base64-encoded content.
            if attachment.inline {
                let inline_cache_key =
                    gmail_inline_attachment_diskcache_key(account.id, mid, attachment_info);
                DISK_CACHE
                    .put_cache(&inline_cache_key, data.as_bytes(), false)
                    .await?;
            }
            DISK_CACHE.get_cache(&cache_key).await?.ok_or_else(|| {
                raise_error!("Unexpected cache miss".into(), ErrorCode::InternalError)
            })
        }
        _ => Err(raise_error!(
            "Expected attachment body part, but received a different part type.".into(),
            ErrorCode::ResourceNotFound
        )),
    }
}

async fn retrieve_graph_attachment(
    account: &AccountModel,
    mid: &str,
    attachment_id: &str,
) -> RustMailerResult<cacache::Reader> {
    let cache_key = graph_attachment_diskcache_key(account.id, mid, attachment_id);
    if let Some(reader) = DISK_CACHE.get_cache(&cache_key).await? {
        return Ok(reader);
    }

    let base_64 =
        OutlookClient::get_attachment(account.id, account.use_proxy, mid, attachment_id).await?;

    let decoded = base64_decode!(base_64.as_bytes());
    DISK_CACHE.put_cache(&cache_key, &decoded, false).await?;
    DISK_CACHE
        .get_cache(&cache_key)
        .await?
        .ok_or_else(|| raise_error!("Unexpected cache miss".into(), ErrorCode::InternalError))
}
