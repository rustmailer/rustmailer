// Copyright © 2025-2026 rustmailer.com
// Licensed under RustMailer License Agreement v1.0
// Unauthorized copying, modification, or distribution is prohibited.

use crate::modules::account::entity::MailerType;
use crate::modules::account::migration::AccountModel;
use crate::modules::cache::vendor::gmail::model::messages::PartBody;
use crate::modules::cache::vendor::gmail::sync::client::GmailClient;
use crate::modules::cache::vendor::outlook::model::{Attachment, Message};
use crate::modules::cache::vendor::outlook::sync::client::OutlookClient;
use crate::modules::error::code::ErrorCode;
use crate::modules::error::RustMailerError;
use crate::modules::imap::section::Encoding;
use crate::modules::message::attachment::inline_attachment_diskcache_key;
use crate::{base64_decode_url_safe, base64_encode, calculate_hash};
use crate::{
    encode_mailbox_name,
    modules::{
        cache::disk::DISK_CACHE,
        context::executors::RUST_MAIL_CONTEXT,
        error::RustMailerResult,
        imap::section::{EmailBodyPart, ImapAttachment, PartType, SegmentPath},
    },
    raise_error,
};

use cacache::Reader;
use mime_guess::from_ext;
use poem_openapi::Object;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncReadExt;

const MAX_BODY_SIZE: usize = 2 * 1024 * 1024;

/// Request for fetching the html/plain content of a specific email message.
#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize, Object)]
pub struct MessageContentRequest {
    /// The name of the mailbox containing the message  
    /// - Required for IMAP/SMTP accounts  
    /// - Not used for Gmail API  
    pub mailbox: Option<String>,
    /// The unique ID of the message, either IMAP UID or Gmail API MID.
    ///
    /// - For IMAP accounts, this is the UID converted to a string. It must be a valid numeric string
    ///   that can be parsed back to a `u32`.
    /// - For Gmail/Graph API accounts, this is the message ID (`mid`) returned by the API.
    pub id: String,
    /// Optional maximum length to retrieve for text parts (useful for large messages)  
    /// - Supported by both IMAP/SMTP and Gmail API  
    pub max_length: Option<usize>,
    /// Specific email body parts to retrieve (e.g., TEXT, HTML)  
    /// - Only used for IMAP/SMTP accounts  
    /// - Comes from `list_messages` results   
    pub sections: Option<Vec<EmailBodyPart>>,
    /// Optional list of attachments to include inline in the response  
    /// - Only used for IMAP/SMTP accounts  
    /// - Comes from `list_messages` results  
    pub inline: Option<Vec<ImapAttachment>>,
}

impl MessageContentRequest {
    pub fn validate(&self, account: &AccountModel) -> RustMailerResult<()> {
        match account.mailer_type {
            MailerType::ImapSmtp => {
                if self.mailbox.is_none() {
                    return Err(raise_error!(
                        "`mailbox` is required for IMAP/SMTP accounts.".into(),
                        ErrorCode::InvalidParameter
                    ));
                }
                if self.id.parse::<u32>().is_err() {
                    return Err(raise_error!(
                        "Invalid IMAP UID: `id` must be a numeric string".into(),
                        ErrorCode::InvalidParameter
                    ));
                }
                if self.sections.is_none() {
                    return Err(raise_error!(
                        "`sections` is required for IMAP/SMTP accounts.".into(),
                        ErrorCode::InvalidParameter
                    ));
                }
            }
            MailerType::GmailApi | MailerType::GraphApi => {
                if self.mailbox.is_some() {
                    return Err(raise_error!(
                        "`mailbox` must not be set for Gmail/Graph API accounts.".into(),
                        ErrorCode::InvalidParameter
                    ));
                }
                if self.sections.is_some() {
                    return Err(raise_error!(
                        "`sections` is only supported for IMAP/SMTP accounts.".into(),
                        ErrorCode::InvalidParameter
                    ));
                }
                if self.inline.is_some() {
                    return Err(raise_error!(
                        "`inline` is only supported for IMAP/SMTP accounts.".into(),
                        ErrorCode::InvalidParameter
                    ));
                }
            }
        }
        Ok(())
    }
}

/// Represents metadata of an attachment in a Gmail message.
///
/// This struct stores information required to identify, download,
/// and render an attachment, including inline images embedded
/// in HTML emails.
#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize, Object)]
pub struct AttachmentInfo {
    /// MIME content type of the attachment (e.g., `image/png`, `application/pdf`).
    pub file_type: String,
    /// Content transfer encoding (usually `"base64"`).
    pub transfer_encoding: Option<String>,
    /// Content-ID, used for inline attachments (referenced in HTML by `cid:` URLs).
    pub content_id: Option<String>,
    /// Whether the attachment is marked as inline (true) or a regular file (false).
    pub inline: bool,
    /// Original filename of the attachment, if provided.
    pub filename: String,
    /// Gmail-specific attachment ID, used to fetch the attachment via Gmail API.
    pub id: String,
    /// Size of the attachment in bytes.
    pub size: u32,
}

impl AttachmentInfo {
    pub fn hash(&self) -> u64 {
        let s = format!(
            "{}|{}|{}|{}|{}|{}",
            self.file_type,
            self.transfer_encoding.as_deref().unwrap_or("n/a"),
            self.content_id.as_deref().unwrap_or("n/a"),
            self.inline,
            self.filename,
            self.size
        );
        calculate_hash!(&s)
    }
}

/// Represents the content of an email message in both plain text and HTML formats.
///
/// This struct contains optional fields for plain text and HTML versions of
/// the email message body. At least one of them may be present.
///
/// # Fields
///
/// - `plain`: The plain text version of the message, if available.
/// - `html`: The HTML version of the message, if available.
#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize, Object)]
pub struct FullMessageContent {
    /// Optional plain text version of the message.
    pub plain: Option<PlainText>,
    /// Optional HTML version of the message.
    pub html: Option<String>,
    /// - **Gmail API accounts**: Always present. If the message has no attachments,
    ///   this will be an empty `Vec`.
    /// - **IMAP accounts**: Always `None`, since attachment metadata is already
    ///   included in the envelope.
    pub attachments: Option<Vec<AttachmentInfo>>,
}

impl FullMessageContent {
    pub fn html(&self) -> Option<&str> {
        self.html.as_deref()
    }

    pub fn plain(&self) -> Option<&str> {
        self.plain.as_ref().map(|plain| plain.content.as_str())
    }
}

/// Represents the plain text content of an email message.
///
/// This struct includes the actual plain text and a flag indicating whether
/// the content has been truncated.
///
/// # Fields
///
/// - `content`: The plain text body of the message.
/// - `truncated`: Indicates whether the content has been truncated,
///   for example, due to length limitations.
#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize, Object)]
pub struct PlainText {
    /// The plain text content of the message.
    pub content: String,

    /// Whether the content has been truncated.
    pub truncated: bool,
}

fn email_content_diskcache_key(
    account_id: u64,
    mailbox_name: &str,
    uid: u32,
    segment_path: SegmentPath,
) -> String {
    format!(
        "email_content_{}_{}_{}_{}",
        account_id, mailbox_name, uid, segment_path
    )
}

fn gmail_content_diskcache_key(account_id: u64, mid: &str) -> String {
    format!("gmail_content_{}_{}", account_id, mid)
}

fn outlook_content_diskcache_key(account_id: u64, mid: &str) -> String {
    format!("outlook_content_{}_{}", account_id, mid)
}

async fn read_string_from_reader(reader: &mut Reader) -> RustMailerResult<Option<String>> {
    let mut buffer = Vec::new();
    if let Err(_) = reader.read_to_end(&mut buffer).await {
        return Ok(None);
    }

    match String::from_utf8(buffer) {
        Ok(s) => Ok(Some(s)),
        Err(_) => Ok(None),
    }
}

async fn read_text_from_reader(
    reader: &mut Reader,
    max_length: Option<usize>,
    actual_size: usize,
) -> RustMailerResult<PlainText> {
    let length_to_read = match max_length {
        Some(max) => max.min(actual_size).min(MAX_BODY_SIZE),
        None => actual_size,
    };
    let mut buffer = vec![0u8; length_to_read];
    let bytes_read = reader
        .read(&mut buffer)
        .await
        .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
    let truncated = bytes_read < actual_size;
    let content = match std::str::from_utf8(&buffer[..bytes_read]) {
        Ok(valid_str) => valid_str.into(),
        Err(_) => "???".into(),
    };
    Ok(PlainText { content, truncated })
}

async fn read_html_from_reader(
    reader: &mut Reader,
    actual_size: usize,
) -> RustMailerResult<String> {
    let mut buffer = vec![0u8; actual_size];
    let bytes_read = reader
        .read(&mut buffer)
        .await
        .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
    let content = match std::str::from_utf8(&buffer[..bytes_read]) {
        Ok(valid_str) => valid_str.into(),
        Err(_) => "???".into(),
    };
    Ok(content)
}

fn to_string(data: &[u8]) -> RustMailerResult<String> {
    let content = match std::str::from_utf8(data) {
        Ok(valid_str) => valid_str.into(),
        Err(_) => "???".into(),
    };
    Ok(content)
}

async fn replace_inline_attachments(
    account_id: u64,
    mailbox: &str,
    uid: u32,
    html_content: &mut String,
    inline_attachments: &[ImapAttachment],
    skip_cache: bool,
) -> RustMailerResult<()> {
    for attachment in inline_attachments {
        // only process inline attachments with content_id and base64 encoding, skip others
        if !attachment.inline
            || attachment.content_id.is_none()
            || attachment.transfer_encoding != Encoding::Base64
        {
            continue;
        }

        let cid = attachment
            .content_id
            .as_deref()
            .unwrap()
            .trim_matches(|c| c == '<' || c == '>');

        if !html_content.contains(cid) {
            continue;
        }

        let attachment_content = if skip_cache {
            // Skip cache and fetch directly from server
            let executor = RUST_MAIL_CONTEXT.imap(account_id).await?;
            let result = executor
                .uid_fetch_single_part(
                    uid.to_string().as_str(),
                    encode_mailbox_name!(mailbox).as_str(),
                    attachment.path.to_string().as_str(),
                )
                .await?;

            let target = result.iter().find(|f| f.uid == Some(uid)).ok_or_else(|| {
                raise_error!(
                    "Failed to fetch attachment from server".into(),
                    ErrorCode::InternalError
                )
            })?;

            let encoded = attachment.encoded(target).ok_or_else(|| {
                raise_error!(
                    "Failed to parse inline attachment content from result".into(),
                    ErrorCode::InternalError
                )
            })?;

            String::from_utf8_lossy(&encoded).into_owned()
        } else {
            // Try cache first, then fall back to server
            let inline_cache_key =
                inline_attachment_diskcache_key(account_id, mailbox, uid, attachment.path.clone());

            match DISK_CACHE.get_cache(&inline_cache_key).await? {
                Some(mut reader) => {
                    let mut str = String::new();
                    reader
                        .read_to_string(&mut str)
                        .await
                        .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
                    reader
                        .check()
                        .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
                    str
                }
                None => {
                    let executor = RUST_MAIL_CONTEXT.imap(account_id).await?;
                    let result = executor
                        .uid_fetch_single_part(
                            uid.to_string().as_str(),
                            encode_mailbox_name!(mailbox).as_str(),
                            attachment.path.to_string().as_str(),
                        )
                        .await?;

                    let target = result.iter().find(|f| f.uid == Some(uid)).ok_or_else(|| {
                        raise_error!(
                            "Failed to fetch attachment from server".into(),
                            ErrorCode::InternalError
                        )
                    })?;

                    let encoded = attachment.encoded(target).ok_or_else(|| {
                        raise_error!(
                            "Failed to parse inline attachment content from result".into(),
                            ErrorCode::InternalError
                        )
                    })?;

                    DISK_CACHE
                        .put_cache(&inline_cache_key, &encoded, false)
                        .await?;
                    String::from_utf8_lossy(&encoded).into_owned()
                }
            }
        };

        let content_type = &attachment.file_type.to_ascii_lowercase();
        let mime_type = from_ext(content_type).first_or_octet_stream();
        let cleaned_content = attachment_content.replace("\n", "").replace("\r", "");
        *html_content = html_content.replace(
            &format!("cid:{}", cid),
            &format!("data:{};base64,{}", mime_type, cleaned_content.trim()),
        );
    }
    Ok(())
}

pub async fn retrieve_email_content(
    account_id: u64,
    request: MessageContentRequest,
    skip_cache: bool,
) -> RustMailerResult<FullMessageContent> {
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
            let sections = request.sections.ok_or_else(|| {
                raise_error!(
                    "`sections` is required when retrieving IMAP/SMTP message content.".into(),
                    ErrorCode::InvalidParameter
                )
            })?;
            let uid = request.id.parse::<u32>().ok().ok_or_else(|| {
                raise_error!(
                    "Invalid IMAP UID: `id` must be a numeric string".into(),
                    ErrorCode::InvalidParameter
                )
            })?;
            let mailbox = request.mailbox.ok_or_else(|| {
                raise_error!(
                    "`mailbox` is required when retrieving IMAP/SMTP message content.".into(),
                    ErrorCode::InvalidParameter
                )
            })?;

            retrieve_imap_message_content(
                account_id,
                sections,
                uid,
                mailbox,
                request.max_length,
                request.inline,
                skip_cache,
            )
            .await
        }
        MailerType::GmailApi => {
            retrieve_gmail_message_content(account_id, request.id, request.max_length, skip_cache)
                .await
        }
        MailerType::GraphApi => {
            retrieve_outlook_message_content(account_id, request.id, request.max_length, skip_cache)
                .await
        }
    }
}

async fn retrieve_imap_message_content(
    account_id: u64,
    sections: Vec<EmailBodyPart>,
    uid: u32,
    mailbox: String,
    max_length: Option<usize>,
    inline: Option<Vec<ImapAttachment>>,
    skip_cache: bool,
) -> RustMailerResult<FullMessageContent> {
    let mut plain: Option<PlainText> = None;
    let mut html: Option<String> = None;

    // Find Plain part
    if let Some(part) = sections.iter().find(|p| p.part_type == PartType::Plain) {
        let content = if skip_cache {
            // Skip cache and fetch directly
            let decoded_content =
                fetch_mail_part_from_imap(account_id, uid, &mailbox, part).await?;
            let mut decoded_content = to_string(&decoded_content)?;

            // Handle max_length truncation
            if matches!(max_length, Some(max) if decoded_content.len() > max) {
                decoded_content.truncate(max_length.unwrap());
                PlainText {
                    content: decoded_content,
                    truncated: true,
                }
            } else {
                PlainText {
                    content: decoded_content,
                    truncated: false,
                }
            }
        } else {
            // Try cache first
            let cache_key =
                email_content_diskcache_key(account_id, &mailbox, uid, part.path.clone());

            if let Some(mut reader) = DISK_CACHE.get_cache(&cache_key).await? {
                read_text_from_reader(&mut reader, max_length, part.size).await?
            } else {
                // Fetch from IMAP if not in cache
                let decoded_content =
                    fetch_mail_part_from_imap(account_id, uid, &mailbox, part).await?;
                // Cache the decoded content
                DISK_CACHE
                    .put_cache(&cache_key, decoded_content.as_slice(), false)
                    .await?;

                let mut decoded_content = to_string(&decoded_content)?;

                // Handle max_length truncation
                if matches!(max_length, Some(max) if decoded_content.len() > max) {
                    decoded_content.truncate(max_length.unwrap());
                    PlainText {
                        content: decoded_content,
                        truncated: true,
                    }
                } else {
                    PlainText {
                        content: decoded_content,
                        truncated: false,
                    }
                }
            }
        };
        plain = Some(content);
    }

    // Find HTML part
    if let Some(part) = sections.iter().find(|p| p.part_type == PartType::Html) {
        let content = if skip_cache {
            // Skip cache and fetch directly
            let decoded_content =
                fetch_mail_part_from_imap(account_id, uid, &mailbox, part).await?;
            let mut decoded_content = to_string(&decoded_content)?;

            // Handle inline attachments
            if let Some(inline) = &inline {
                replace_inline_attachments(
                    account_id,
                    &mailbox,
                    uid,
                    &mut decoded_content,
                    inline,
                    skip_cache,
                )
                .await?;
            }
            decoded_content
        } else {
            // Try cache first
            let cache_key =
                email_content_diskcache_key(account_id, &mailbox, uid, part.path.clone());

            if let Some(mut reader) = DISK_CACHE.get_cache(&cache_key).await? {
                let mut content = read_html_from_reader(&mut reader, part.size).await?;
                if let Some(inline) = &inline {
                    replace_inline_attachments(
                        account_id,
                        &mailbox,
                        uid,
                        &mut content,
                        inline,
                        skip_cache,
                    )
                    .await?;
                }
                content
            } else {
                // Fetch from IMAP if not in cache
                let decoded_content =
                    fetch_mail_part_from_imap(account_id, uid, &mailbox, part).await?;
                // Cache the decoded content
                DISK_CACHE
                    .put_cache(&cache_key, decoded_content.as_slice(), false)
                    .await?;

                let mut decoded_content = to_string(&decoded_content)?;

                // Handle inline attachments
                if let Some(inline) = &inline {
                    replace_inline_attachments(
                        account_id,
                        &mailbox,
                        uid,
                        &mut decoded_content,
                        inline,
                        skip_cache,
                    )
                    .await?;
                }
                decoded_content
            }
        };
        html = Some(content);
    }

    Ok(FullMessageContent {
        plain,
        html,
        attachments: None,
    })
}

async fn gmail_embed_inline_attachments(
    account_id: u64,
    use_proxy: Option<u64>,
    mid: &str,
    message_content: &mut FullMessageContent,
) -> RustMailerResult<()> {
    if let (Some(attachments), Some(html)) =
        (&message_content.attachments, &mut message_content.html)
    {
        for att in attachments {
            if att.inline && att.content_id.is_some() {
                if let PartBody::Body { data, .. } =
                    GmailClient::get_attachment(account_id, use_proxy, mid, &att.id).await?
                {
                    let cid_ref = format!("cid:{}", att.content_id.as_deref().unwrap());
                    let data_uri =
                        format!("data:{};base64,{}", att.file_type, normalize_base64(&data)?);
                    *html = html.replace(&cid_ref, &data_uri);
                }
            }
        }
    }
    Ok(())
}

fn normalize_base64(data: &str) -> RustMailerResult<String> {
    base64_decode_url_safe!(data)
        .map_err(|e| {
            raise_error!(
                format!("Failed to decode base64_content: {}", e),
                ErrorCode::InternalError
            )
        })
        .map(|bytes| base64_encode!(bytes))
}

async fn gmail_fetch_and_cache(
    account_id: u64,
    use_proxy: Option<u64>,
    mid: &str,
    cache_key: &str,
    max_length: Option<usize>,
) -> RustMailerResult<FullMessageContent> {
    let full_message = GmailClient::get_full_messages(account_id, use_proxy, mid).await?;
    let mut message_content: FullMessageContent = full_message.try_into()?;
    if let Some(max_len) = max_length {
        if let Some(plain) = &mut message_content.plain {
            if plain.content.len() > max_len {
                plain.content.truncate(max_len);
                plain.truncated = true;
            } else {
                plain.truncated = false;
            }
        }
    }

    //Check for inline attachments; if present, download and embed them into the HTML, then cache the result. This approach is simplified compared to the IMAP method.
    gmail_embed_inline_attachments(account_id, use_proxy, mid, &mut message_content).await?;

    let json = serde_json::to_string(&message_content).map_err(|e| {
        raise_error!(
            format!(
                "Failed to serialize FullMessageContent into JSON for caching.\nError: {:#?}",
                e
            ),
            ErrorCode::InternalError
        )
    })?;
    DISK_CACHE
        .put_cache(cache_key, json.as_bytes(), false)
        .await?;

    Ok(message_content)
}

async fn retrieve_gmail_message_content(
    account_id: u64,
    mid: String,
    max_length: Option<usize>,
    skip_cache: bool,
) -> RustMailerResult<FullMessageContent> {
    let account = AccountModel::get(account_id).await?;
    let cache_key = gmail_content_diskcache_key(account_id, &mid);
    if skip_cache {
        return gmail_fetch_and_cache(account_id, account.use_proxy, &mid, &cache_key, max_length)
            .await;
    }

    if let Some(mut reader) = DISK_CACHE.get_cache(&cache_key).await? {
        if let Some(json) = read_string_from_reader(&mut reader).await? {
            let mut message: FullMessageContent = serde_json::from_str(&json).map_err(|e| {
                raise_error!(
                    format!(
                        "Failed to deserialize cached JSON into FullMessageContent.\nError: {:#?}",
                        e
                    ),
                    ErrorCode::InternalError
                )
            })?;
            if let Some(max_len) = max_length {
                if let Some(plain) = &mut message.plain {
                    if plain.content.len() > max_len {
                        plain.content.truncate(max_len);
                        plain.truncated = true;
                    } else {
                        plain.truncated = false;
                    }
                }
            }
            return Ok(message);
        }
    }
    gmail_fetch_and_cache(account_id, account.use_proxy, &mid, &cache_key, max_length).await
}

async fn fetch_mail_part_from_imap(
    account_id: u64,
    uid: u32,
    mailbox: &str,
    part: &EmailBodyPart,
) -> RustMailerResult<Vec<u8>> {
    let executor = RUST_MAIL_CONTEXT.imap(account_id).await?;

    let result = executor
        .uid_fetch_single_part(
            uid.to_string().as_str(),
            encode_mailbox_name!(mailbox).as_str(),
            part.path.to_string().as_str(),
        )
        .await?;

    let target = result.iter().find(|f| f.uid == Some(uid)).ok_or_else(|| {
        raise_error!(
            "Failed to fetch the specified content from the IMAP server".into(),
            ErrorCode::InternalError
        )
    })?;

    part.decode(target).ok_or_else(|| {
        raise_error!(
            "Failed to decode the email body content from the result".into(),
            ErrorCode::InternalError
        )
    })
}

async fn retrieve_outlook_message_content(
    account_id: u64,
    mid: String,
    max_length: Option<usize>,
    skip_cache: bool,
) -> RustMailerResult<FullMessageContent> {
    let account = AccountModel::get(account_id).await?;
    let cache_key = outlook_content_diskcache_key(account_id, &mid);
    if skip_cache {
        return outlook_fetch_and_cache(
            account_id,
            account.use_proxy,
            &mid,
            &cache_key,
            max_length,
        )
        .await;
    }

    if let Some(mut reader) = DISK_CACHE.get_cache(&cache_key).await? {
        if let Some(json) = read_string_from_reader(&mut reader).await? {
            let mut message: FullMessageContent = serde_json::from_str(&json).map_err(|e| {
                raise_error!(
                    format!(
                        "Failed to deserialize cached JSON into FullMessageContent.\nError: {:#?}",
                        e
                    ),
                    ErrorCode::InternalError
                )
            })?;
            if let Some(max_len) = max_length {
                if let Some(plain) = &mut message.plain {
                    if plain.content.len() > max_len {
                        plain.content.truncate(max_len);
                        plain.truncated = true;
                    } else {
                        plain.truncated = false;
                    }
                }
            }
            return Ok(message);
        }
    }
    outlook_fetch_and_cache(account_id, account.use_proxy, &mid, &cache_key, max_length).await
}

async fn outlook_fetch_and_cache(
    account_id: u64,
    use_proxy: Option<u64>,
    mid: &str,
    cache_key: &str,
    max_length: Option<usize>,
) -> RustMailerResult<FullMessageContent> {
    let full_message = OutlookClient::get_message(account_id, use_proxy, mid).await?;
    // println!("{:#?}", &full_message.body);
    let mut message_content: FullMessageContent = full_message.try_into()?;
    if let Some(max_len) = max_length {
        if let Some(plain) = &mut message_content.plain {
            if plain.content.len() > max_len {
                plain.content.truncate(max_len);
                plain.truncated = true;
            } else {
                plain.truncated = false;
            }
        }
    }

    //Check for inline attachments; if present, download and embed them into the HTML, then cache the result. This approach is simplified compared to the IMAP method.
    outlook_embed_inline_attachments(account_id, use_proxy, mid, &mut message_content).await?;

    let json = serde_json::to_string(&message_content).map_err(|e| {
        raise_error!(
            format!(
                "Failed to serialize FullMessageContent into JSON for caching.\nError: {:#?}",
                e
            ),
            ErrorCode::InternalError
        )
    })?;
    DISK_CACHE
        .put_cache(cache_key, json.as_bytes(), false)
        .await?;

    Ok(message_content)
}

async fn outlook_embed_inline_attachments(
    account_id: u64,
    use_proxy: Option<u64>,
    mid: &str,
    message_content: &mut FullMessageContent,
) -> RustMailerResult<()> {
    if let (Some(attachments), Some(html)) =
        (&message_content.attachments, &mut message_content.html)
    {
        for att in attachments {
            if att.inline && att.content_id.is_some() {
                let data =
                    OutlookClient::get_attachment(account_id, use_proxy, mid, &att.id).await?;
                let cid_ref = format!("cid:{}", att.content_id.as_deref().unwrap());
                let data_uri = format!("data:{};base64,{}", att.file_type, &data);
                //Inline attachments in Outlook do not need to be decoded and then re-encoded; it seems unrelated to URL-safe encoding.
                *html = html.replace(&cid_ref, &data_uri);
            }
        }
    }
    Ok(())
}

impl TryFrom<Message> for FullMessageContent {
    type Error = RustMailerError;

    fn try_from(value: Message) -> Result<Self, Self::Error> {
        let attachments = value
            .attachments
            .map(|v| v.into_iter().map(Into::into).collect());

        // body maybe None
        let (plain, html) = if let Some(body) = value.body {
            let content_type = body
                .content_type
                .as_ref()
                .map(|s| s.trim().to_ascii_lowercase())
                .unwrap_or_else(|| "text".to_string());

            let content = body.content.unwrap_or_default();

            match content_type.as_str() {
                "text" | "text/plain" => (
                    Some(PlainText {
                        content,
                        truncated: false,
                    }),
                    None,
                ),
                "html" | "text/html" => (None, Some(content)),
                _ => (None, None), // ingore
            }
        } else {
            (None, None)
        };

        Ok(Self {
            plain,
            html,
            attachments,
        })
    }
}

impl From<Attachment> for AttachmentInfo {
    fn from(value: Attachment) -> Self {
        Self {
            file_type: value
                .content_type
                .unwrap_or_else(|| "application/octet-stream".to_string()),
            transfer_encoding: None,
            content_id: value.content_id,
            inline: value.is_inline.unwrap_or(false),
            filename: value.name.unwrap_or_else(|| "unknown".to_string()),
            id: value.id,
            size: value.size.unwrap_or(0),
        }
    }
}

#[cfg(test)]
mod tests {

    use super::*;

    #[test]
    fn test_all_mime_types() {
        assert_eq!(from_ext("png").first_or_octet_stream(), "image/png");
        assert_eq!(from_ext("jpeg").first_or_octet_stream(), "image/jpeg");
        assert_eq!(from_ext("jpg").first_or_octet_stream(), "image/jpeg");
        assert_eq!(from_ext("json").first_or_octet_stream(), "application/json");
        assert_eq!(from_ext("txt").first_or_octet_stream(), "text/plain");
        assert_eq!(
            from_ext("unknown").first_or_octet_stream(),
            "application/octet-stream"
        );
        assert_eq!(from_ext("PNG").first_or_octet_stream(), "image/png");
        assert_eq!(from_ext("JpG").first_or_octet_stream(), "image/jpeg");
        assert_eq!(from_ext("mp4").first_or_octet_stream(), "video/mp4");
        assert_eq!(from_ext("webm").first_or_octet_stream(), "video/webm");
        assert_eq!(from_ext("avi").first_or_octet_stream(), "video/x-msvideo");
    }
}
