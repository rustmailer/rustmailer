// Copyright © 2025-2026 rustmailer.com
// Licensed under RustMailer License Agreement v1.0
// Unauthorized copying, modification, or distribution is prohibited.

use crate::{
    base64_decode_url_safe,
    modules::{
        account::{entity::MailerType, migration::AccountModel},
        cache::{
            disk::DISK_CACHE,
            vendor::{gmail::sync::client::GmailClient, outlook::sync::client::OutlookClient},
        },
        context::executors::RUST_MAIL_CONTEXT,
        error::{code::ErrorCode, RustMailerResult},
    },
    raise_error,
};
// use poem_openapi::Object;
// use serde::{Deserialize, Serialize};

use crate::modules::message::get_minimal_meta;

const MAX_EMAIL_TOTAL_SIZE: u32 = 55 * 1024 * 1024; // 55 MB

// #[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize, Object)]
// pub struct FullMessageRequest {
//     pub mailbox: String,
//     pub filename: String,
//     pub uid: u32,
//     pub max_length: Option<usize>,
// }

fn raw_email_diskcache_key(account_id: u64, mailbox_name: &str, uid: u32) -> String {
    format!("imap_raw_email_{}_{}_{}", account_id, mailbox_name, uid)
}

fn gmail_raw_email_diskcache_key(account_id: u64, mid: &str) -> String {
    format!("gmail_raw_email_{}_{}", account_id, mid)
}

fn outlook_raw_email_diskcache_key(account_id: u64, mid: &str) -> String {
    format!("outlook_raw_email_{}_{}", account_id, mid)
}

pub async fn retrieve_raw_email(
    account_id: u64,
    mailbox: Option<&str>,
    id: &str,
) -> RustMailerResult<cacache::Reader> {
    let account = AccountModel::get(account_id).await?;

    if !account.enabled {
        return Err(raise_error!(
            format!("Account id='{account_id}' is disabled"),
            ErrorCode::AccountDisabled
        ));
    }

    match account.mailer_type {
        MailerType::ImapSmtp => {
            let mailbox = mailbox.ok_or_else(|| {
                raise_error!(
                    "Missing required parameter: `mailbox` for IMAP/SMTP".into(),
                    ErrorCode::InvalidParameter
                )
            })?;
            let uid = id.parse::<u32>().ok().ok_or_else(|| {
                raise_error!(
                    "Invalid IMAP UID: `id` must be a numeric string".into(),
                    ErrorCode::InvalidParameter
                )
            })?;
            retrieve_imap_raw_email(account_id, mailbox, uid).await
        }
        MailerType::GmailApi => retrieve_gmail_raw_email(&account, id).await,
        MailerType::GraphApi => retrieve_outlook_raw_email(&account, id).await,
    }
}

async fn retrieve_imap_raw_email(
    account_id: u64,
    mailbox: &str,
    uid: u32,
) -> RustMailerResult<cacache::Reader> {
    let meta = get_minimal_meta(account_id, mailbox, uid).await?;
    if meta.size > MAX_EMAIL_TOTAL_SIZE {
        return Err(raise_error!(format!(
            "Message size {} bytes exceeds maximum allowed size of {} bytes (UID: {}, Mailbox: '{}')",
            meta.size,
            MAX_EMAIL_TOTAL_SIZE,
            uid,
            mailbox
        ), ErrorCode::ExceedsLimitation));
    }

    let cache_key = raw_email_diskcache_key(account_id, mailbox, uid);
    if let Some(reader) = DISK_CACHE.get_cache(&cache_key).await? {
        return Ok(reader);
    }

    let executor = RUST_MAIL_CONTEXT.imap(account_id).await?;
    let fetch = executor
        .uid_fetch_full_message(uid.to_string().as_str(), mailbox)
        .await?
        .ok_or_else(|| {
            raise_error!(
                format!("No message found for UID {} in mailbox {}", uid, mailbox),
                ErrorCode::ImapUnexpectedResult
            )
        })?;

    let body = fetch.body().ok_or_else(|| {
        raise_error!(
            format!(
                "Message UID {} in mailbox {} is missing a body",
                uid, mailbox
            ),
            ErrorCode::ImapUnexpectedResult
        )
    })?;

    DISK_CACHE.put_cache(&cache_key, body, false).await?;
    DISK_CACHE
        .get_cache(&cache_key)
        .await?
        .ok_or_else(|| raise_error!("Unexpected cache miss".into(), ErrorCode::InternalError))
}

async fn retrieve_gmail_raw_email(
    account: &AccountModel,
    mid: &str,
) -> RustMailerResult<cacache::Reader> {
    let meta = GmailClient::get_message(account.id, account.use_proxy, mid).await?;
    if meta.size_estimate > MAX_EMAIL_TOTAL_SIZE {
        return Err(raise_error!(
            format!(
                "Message size {} bytes exceeds maximum allowed size of {} bytes (mid: {})",
                meta.size_estimate, MAX_EMAIL_TOTAL_SIZE, mid
            ),
            ErrorCode::ExceedsLimitation
        ));
    }

    let cache_key = gmail_raw_email_diskcache_key(account.id, mid);
    if let Some(reader) = DISK_CACHE.get_cache(&cache_key).await? {
        return Ok(reader);
    }

    let data = GmailClient::get_raw_messages(account.id, account.use_proxy, mid).await?;
    let raw = data
        .raw
        .ok_or_else(|| raise_error!("".into(), ErrorCode::InternalError))?;
    let data = base64_decode_url_safe!(raw).map_err(|e| {
        raise_error!(
            format!("Failed to decode base64_content: {}", e),
            ErrorCode::InternalError
        )
    })?;

    DISK_CACHE.put_cache(&cache_key, &data, false).await?;
    DISK_CACHE
        .get_cache(&cache_key)
        .await?
        .ok_or_else(|| raise_error!("Unexpected cache miss".into(), ErrorCode::InternalError))
}

async fn retrieve_outlook_raw_email(
    account: &AccountModel,
    mid: &str,
) -> RustMailerResult<cacache::Reader> {
    let cache_key = outlook_raw_email_diskcache_key(account.id, mid);
    if let Some(reader) = DISK_CACHE.get_cache(&cache_key).await? {
        return Ok(reader);
    }
    let data = OutlookClient::get_raw_message(account.id, account.use_proxy, mid).await?;
    DISK_CACHE.put_cache(&cache_key, &data, false).await?;
    DISK_CACHE
        .get_cache(&cache_key)
        .await?
        .ok_or_else(|| raise_error!("Unexpected cache miss".into(), ErrorCode::InternalError))
}
