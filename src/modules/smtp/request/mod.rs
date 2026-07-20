// Copyright © 2025-2026 rustmailer.com
// Licensed under RustMailer License Agreement v1.0
// Unauthorized copying, modification, or distribution is prohibited.

use crate::base64_decode_url_safe;
use crate::encode_mailbox_name;
use crate::generate_token;
use crate::modules::account::entity::MailerType;
use crate::modules::cache::disk::DISK_CACHE;
use crate::modules::cache::imap::mailbox::EmailFlag;
use crate::modules::cache::imap::mailbox::EnvelopeFlag;
use crate::modules::cache::imap::mailbox::MailBox;
use crate::modules::cache::imap::migration::EmailEnvelopeV3;
use crate::modules::cache::model::Envelope;
use crate::modules::cache::vendor::gmail::sync::client::GmailClient;
use crate::modules::cache::vendor::gmail::sync::envelope::GmailEnvelope;
use crate::modules::cache::vendor::gmail::sync::labels::GmailLabels;
use crate::modules::cache::vendor::outlook::sync::client::OutlookClient;
use crate::modules::cache::vendor::outlook::sync::envelope::OutlookEnvelope;
use crate::modules::cache::vendor::outlook::sync::folders::OutlookFolder;
use crate::modules::common::Addr;
use crate::modules::context::executors::RUST_MAIL_CONTEXT;
use crate::modules::envelope::extractor::extract_envelope;
use crate::modules::error::code::ErrorCode;
use crate::modules::message::content::retrieve_email_content;
use crate::modules::message::content::AttachmentInfo;
use crate::modules::message::content::FullMessageContent;
use crate::modules::message::content::MessageContentRequest;
use crate::modules::smtp::template::preview::EmailPreview;
use crate::modules::tasks::queue::RustMailerTaskQueue;
use crate::modules::utils::envelope_hash_from_id;
use crate::utc_now;
use crate::validate_email;
use crate::{
    modules::{
        account::migration::AccountModel,
        error::RustMailerResult,
        imap::section::ImapAttachment,
        message::attachment::{retrieve_email_attachment, AttachmentRequest},
    },
    raise_error,
};
use imap_proto::NameAttribute;
use mail_send::mail_builder::headers::address::EmailAddress as SmtpEmailAddress;
use mail_send::mail_builder::{headers::address::Address, mime::BodyPart, MessageBuilder};
use mail_send::smtp::message::IntoMessage;
use mail_send::smtp::message::Parameters;
use mime_guess::from_ext;
use mime_guess::{mime, Mime};
use poem_openapi::{Enum, Object};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use task::AnswerEmail;
use task::SmtpTask;
use tokio::io::AsyncReadExt;
use tracing::warn;

pub mod builder;
pub mod forward;
pub mod headers;
pub mod new;
pub mod parser;
pub mod reply;
pub mod task;

/// A structure representing the envelope of an email used for sending.
///
/// The `MailEnvelope` struct encapsulates the sender and recipient information required
/// for email transmission, specifically for defining the SMTP envelope. The SMTP envelope
/// is used by mail servers to route emails and is distinct from the email headers seen by
/// end users. This struct is necessary to specify the "MAIL FROM" and "RCPT TO" fields
/// in the SMTP protocol, which determine the actual sender and recipients of the email,
/// regardless of the "From" or "To" headers in the email content. It is typically used
/// in email clients or servers to prepare emails for delivery via an SMTP server.
///
/// # Purpose
/// - **Routing**: The envelope defines the actual sender and recipient addresses used by
///   SMTP servers for routing the email.
/// - **Delivery**: Ensures emails are sent to the correct recipients, even if the email
///   headers (e.g., "To" or "CC") differ.
/// - **Validation**: Allows email systems to validate sender and recipient addresses
///   before transmission.
/// - **Flexibility**: Supports multiple recipients for sending emails to several
///   addresses in a single transaction.
///
/// # Usage
/// This struct is used when interacting with an SMTP client library to send emails. For
/// example, it can be passed to an SMTP client to set up the sender and recipient details
/// before sending the email content.
#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize, Object)]
pub struct MailEnvelope {
    /// The email address of the sender (used in the SMTP "MAIL FROM" command).
    pub from: String,
    /// A vector of email addresses for the recipients (used in the SMTP "RCPT TO" commands).
    pub recipients: Vec<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize, Object)]
pub struct AttachmentRef {
    /// The name of the IMAP mailbox containing the attachment.
    ///
    /// This field specifies the mailbox (e.g., "INBOX" or "Sent") where the attachment is stored.
    pub mailbox_name: String,

    /// The unique identifier (UID) of the email message containing the attachment.
    ///
    /// This field identifies the specific email message within the mailbox, as per IMAP protocol standards.
    pub uid: u32,

    /// The attachment data retrieved from the IMAP mailbox.
    ///
    /// This field contains the actual attachment content or metadata, encapsulated in an `ImapAttachment` type.
    pub attachment_data: ImapAttachment,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize, Object)]
pub struct AttachmentPayload {
    /// The Base64-encoded content of the attachment.
    ///
    /// This optional field contains the attachment's data encoded in Base64 format. If provided,
    /// it represents the raw content of the attachment (e.g., file bytes).
    pub base64_content: Option<String>,

    /// A reference to an attachment from another message in the same account.
    ///
    /// This optional field refers to an attachment that exists in a different email
    /// within the same mailbox account. It is used when the current message does not
    /// contain the attachment content directly in `base64_content`, but instead links
    /// to an existing attachment (e.g. by message UID and MIME section index).
    ///
    /// ## Notes
    /// - This feature is currently **IMAP-only**.
    /// - Gmail API and Microsoft Graph API do **not** support cross-message attachment
    ///   references at this time.
    /// - For non-IMAP accounts, attachment content must be provided explicitly.
    pub attachment_ref: Option<AttachmentRef>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize, Object)]
pub struct MailAttachment {
    /// The name of the attached file.
    ///
    /// This optional field specifies the file name (e.g., "document.pdf") as it will appear
    /// in the email. If not provided, a default or generated name may be used.
    pub file_name: Option<String>,

    /// The content of the attachment.
    ///
    /// This required field contains the binary or text data of the attachment, encapsulated
    /// in an `AttachmentPayload` type.
    pub payload: AttachmentPayload,

    /// The MIME type of the attachment.
    ///
    /// This required field specifies the content type of the attachment (e.g., "application/pdf"
    /// or "image/png") to inform email clients how to handle the file.
    pub mime_type: String,

    /// Indicates whether the attachment is inline.
    ///
    /// If `true`, the attachment is intended to be displayed within the email body (e.g., an
    /// embedded image). If `false`, it is treated as a regular file attachment.
    pub inline: bool,

    /// The content ID for inline attachments.
    ///
    /// This optional field specifies a unique identifier for inline attachments (e.g., for
    /// referencing in HTML email content using `cid:<content_id>`). It is typically used when
    /// `inline` is `true`.
    pub content_id: Option<String>,
}

impl MailAttachment {
    pub async fn get_content(&self, account: &AccountModel) -> RustMailerResult<BodyPart<'static>> {
        if let Some(content) = &self.payload.base64_content {
            return Self::decode_base64_content(content, &self.mime_type);
        }

        if let Some(attachment_ref) = &self.payload.attachment_ref {
            if matches!(account.mailer_type, MailerType::ImapSmtp) {
                let request = AttachmentRequest {
                    id: attachment_ref.uid.to_string(),
                    mailbox: Some(attachment_ref.mailbox_name.clone()),
                    attachment: Some(attachment_ref.attachment_data.clone()),
                    attachment_info: None,
                    filename: None,
                    attachment_id: None,
                };
                return Self::retrieve_and_decode_attachment(request, &self.mime_type, account)
                    .await;
            } else {
                warn!(
                    "AttachmentRef is only supported for IMAP accounts. \
                    Gmail and Microsoft Graph API do not support this feature yet."
                );
            }
        }

        Err(raise_error!(
            "No content available in attachment payload".into(),
            ErrorCode::InvalidParameter
        ))
    }

    fn decode_base64_content(
        content: &str,
        mime_type: &str,
    ) -> RustMailerResult<BodyPart<'static>> {
        let decoded = base64_decode_url_safe!(content).map_err(|e| {
            raise_error!(
                format!("Failed to decode base64_content: {}", e),
                ErrorCode::InternalError
            )
        })?;

        let mime = mime_type.parse::<Mime>().map_err(|e| {
            raise_error!(
                format!("Invalid MIME type: {}", e),
                ErrorCode::InternalError
            )
        })?;

        if mime.type_() == mime::TEXT {
            let text = String::from_utf8(decoded).map_err(|e| {
                raise_error!(
                    format!("Invalid UTF-8 in text content: {}", e),
                    ErrorCode::InternalError
                )
            })?;
            Ok(BodyPart::Text(Cow::Owned(text)))
        } else {
            Ok(BodyPart::Binary(Cow::Owned(decoded)))
        }
    }

    async fn retrieve_and_decode_attachment(
        request: AttachmentRequest,
        mime_type: &str,
        account: &AccountModel,
    ) -> RustMailerResult<BodyPart<'static>> {
        let (mut reader, _) = retrieve_email_attachment(account.id, request).await?;

        let mut buffer = Vec::new();
        reader.read_to_end(&mut buffer).await.map_err(|e| {
            raise_error!(
                format!("Failed to read attachment content: {}", e),
                ErrorCode::InternalError
            )
        })?;

        let mime = mime_type.parse::<Mime>().map_err(|e| {
            raise_error!(
                format!("Invalid MIME type: {}", e),
                ErrorCode::InternalError
            )
        })?;

        if mime.type_() == mime::TEXT {
            let text = String::from_utf8(buffer).map_err(|e| {
                raise_error!(
                    format!("Invalid UTF-8 in attachment content: {}", e),
                    ErrorCode::InternalError
                )
            })?;
            Ok(BodyPart::Text(Cow::Owned(text)))
        } else {
            Ok(BodyPart::Binary(Cow::Owned(buffer)))
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize, Enum)]
pub enum ReturnContent {
    Full,
    #[default]
    Hdrs,
}

#[derive(Debug, PartialEq, Eq, Clone, Serialize, Deserialize, Object)]
pub struct EmailAddress {
    /// The display name associated with the email address.
    ///
    /// This optional field specifies a human-readable name (e.g., "John Doe") to be displayed
    /// alongside the email address in email clients.
    pub name: Option<String>,

    /// The email address itself.
    ///
    /// This required field contains the actual email address (e.g., "john.doe@example.com") used
    /// for sending or receiving emails.
    #[oai(validator(custom = "crate::modules::common::validator::EmailValidator"))]
    pub address: String,
}

impl From<EmailAddress> for Addr {
    fn from(email_address: EmailAddress) -> Self {
        Addr {
            name: email_address.name,
            address: Some(email_address.address),
        }
    }
}

impl From<EmailAddress> for SmtpEmailAddress<'_> {
    fn from(email_address: EmailAddress) -> Self {
        SmtpEmailAddress {
            name: email_address.name.map(Cow::Owned),
            email: Cow::Owned(email_address.address),
        }
    }
}

impl From<EmailAddress> for Address<'_> {
    fn from(email_address: EmailAddress) -> Self {
        Address::Address(email_address.into())
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize, Enum)]
pub enum NotifyOption {
    /// Request a DSN when the email is successfully delivered.
    Success,
    /// Request a DSN when the email delivery fails (default).
    #[default]
    Failure,
    /// Request a DSN when the email delivery is delayed.
    Delay,
    /// Do not request any DSN, even if supported.
    Never,
}

impl std::fmt::Display for NotifyOption {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            NotifyOption::Success => "SUCCESS",
            NotifyOption::Failure => "FAILURE",
            NotifyOption::Delay => "DELAY",
            NotifyOption::Never => "NEVER",
        };
        write!(f, "{}", s)
    }
}

/// A structure representing the configuration for Delivery Status Notifications (DSN) in email sending.
///
/// The `DSNConfig` struct defines settings for requesting DSNs as part of the SMTP DSN extension
/// (RFC 3461). It is used to configure how and when a mail server should notify the sender about
/// the delivery status of an email sent using a `MailEnvelope`. This is particularly useful for
/// tracking delivery outcomes or debugging email sending issues, especially when combined with
/// retry mechanisms (e.g., `Retry` struct).
///
/// ### Purpose
/// - **Status Tracking**: Allows the sender to receive notifications about the success, failure,
///   or delay of email delivery.
/// - **Debugging**: Helps diagnose issues in email delivery by providing detailed status information.
/// - **Customization**: Enables fine-grained control over DSN behavior, including what content to
///   return and which recipients to track.
/// - **Reliability**: Works with retry mechanisms to ensure robust email delivery workflows.
///
/// ### Usage
/// This struct is typically used with an SMTP client to configure DSN settings for an email defined
/// by a `MailEnvelope`. For example, it can specify that DSNs should be sent for delivery failures
/// and include full email content in the notification.
///
/// ### Example
/// ```
/// use NotifyOption::*;
/// let dsn_config = DSNConfig {
///     ret: ReturnContent::Full, // Assuming ReturnContent::Full is defined
///     envid: Some("unique-id-123".to_string()),
///     notify: vec![Failure, Delay],
///     orcpt: Some("recipient@example.com".to_string()),
/// };
/// // Use dsn_config with an SMTP client to request DSNs for an email.
/// ```
#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize, Object)]
pub struct DSNConfig {
    /// Specifies the amount of email content to include in the DSN (e.g., headers only or full message).
    /// This corresponds to the SMTP DSN `RET` parameter.
    pub ret: ReturnContent,
    /// An optional envelope identifier for tracking the email (SMTP DSN `ENVID` parameter).
    pub envid: Option<String>,
    /// A list of conditions under which DSNs are requested (SMTP DSN `NOTIFY` parameter).
    pub notify: Vec<NotifyOption>,
    /// An optional original recipient address for DSN tracking (SMTP DSN `ORCPT` parameter).
    pub orcpt: Option<String>,
}

/// An enumeration representing the retry strategy for failed email sending attempts.
///
/// This enum defines the possible strategies for scheduling retries when an email fails
/// to send, typically used in conjunction with the `Retry` struct to configure retry behavior
/// in an SMTP client.
///
/// ### Purpose
/// - **Linear**: Retries are scheduled with a fixed delay between attempts.
/// - **Exponential**: Retries are scheduled with an exponentially increasing delay between
///   attempts, useful for reducing server load during repeated failures.
///
/// ### Usage
/// Used to specify how delays between retry attempts are calculated when sending emails
/// fails due to temporary issues (e.g., network errors or server unavailability).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize, Enum)]
pub enum Strategy {
    /// A retry strategy with a fixed delay between attempts.
    Linear,
    /// A retry strategy with an exponentially increasing delay between attempts.
    Exponential,
}

/// A structure representing the configuration for retrying failed email sending attempts.
///
/// The `Retry` struct defines the retry strategy, delay duration, and maximum number of
/// retry attempts for sending an email. It is used in conjunction with an SMTP client and
/// the `MailEnvelope` struct to handle transient failures during email transmission, such
/// as network issues or temporary server errors.
///
/// ### Purpose
/// - **Reliability**: Ensures emails are retried in case of temporary failures, improving
///   delivery success rates.
/// - **Flexibility**: Allows customization of retry behavior (linear or exponential) to
///   balance between prompt retries and server load management.
/// - **Control**: Limits the number of retries to prevent infinite loops in case of
///   persistent failures.
///
/// ### Usage
/// This struct is typically used to configure an SMTP client to retry sending an email
/// (defined by a `MailEnvelope`) when a failure occurs. For example, it can specify that
/// retries should occur with a linear delay of 5 seconds up to 3 times.
///
/// ### Example
/// ```
/// use Strategy::*;
/// let retry_config = Retry {
///     strategy: Linear,
///     seconds: 5,
///     max_retries: 3,
/// };
/// // Use retry_config with an SMTP client to handle email sending retries.
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize, Object)]
pub struct Retry {
    /// The retry strategy to use (Linear or Exponential).
    pub strategy: Strategy,
    /// The base delay (in seconds) between retry attempts.
    /// For `Linear`, this is the fixed delay; for `Exponential`, this is the initial delay.
    pub seconds: u32,
    /// The maximum number of retry attempts (0 means no retries).
    pub max_retries: u32,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize, Object)]
pub struct SendControl {
    /// The email envelope containing sender and recipient addresses (SMTP `MAIL FROM` and `RCPT TO`).
    pub envelope: Option<MailEnvelope>,
    /// Whether to save a copy of the email to the sent folder after successful delivery.
    pub save_to_sent: Option<bool>,
    /// The name of the folder where the email should be saved if `save_to_sent` is true.
    /// If `None` and `save_to_sent` is true, a default folder (e.g., "Sent") may be used.
    pub sent_folder: Option<String>,
    /// Whether to perform a dry run (simulate sending without actual delivery).
    /// Useful for testing email configurations without sending emails.
    pub dry_run: Option<bool>,
    /// An optional Unix timestamp (milliseconds since epoch) specifying when to send the email.
    /// If `None`, the email is sent immediately.
    pub send_at: Option<i64>,
    /// The retry policy for handling transient failures during email sending.
    /// If `None`, no retries are attempted.
    pub retry_policy: Option<Retry>,
    /// The optional name of the Mail Transfer Agent (MTA) to use for sending the email.
    /// If `None`, the SMTP client uses its default MTA.
    pub mta: Option<u64>,
    /// The configuration for Delivery Status Notifications (DSN) to track delivery status.
    /// If `None`, no DSNs are requested.
    pub dsn: Option<DSNConfig>,
    /// Unique identifier for categorizing and tracking email campaigns.
    ///
    /// - Group related emails together for analytics and reporting
    /// - This field is **only used when sending new emails**
    pub campaign_id: Option<String>,

    /// Whether tracking should be inserted into the email.
    ///
    /// Note: This field only takes effect **if system-wide tracking is enabled** (`SETTINGS.rustmailer_email_tracking_enabled == true`).
    /// If system tracking is disabled, this flag has no effect and no tracking will be inserted.
    /// - This field is **only used when sending new emails**
    pub enable_tracking: Option<bool>,
}

impl SendControl {
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();
        if let Some(envelope) = &self.envelope {
            if validate_email!(&envelope.from).is_err() {
                errors.push("Invalid 'send_control.envelope.from' email address".into());
            }
            for recipient in &envelope.recipients {
                if validate_email!(recipient).is_err() {
                    errors.push("Invalid 'send_control.envelope.recipients' email address".into());
                }
            }
        }
        if let Some(send_at) = self.send_at {
            if let Err(error) = EmailHandler::validate_send_at(send_at, utc_now!()) {
                errors.push(error);
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    pub fn build_dsn_params(&self) -> RustMailerResult<(Parameters<'_>, Parameters<'_>)> {
        let mut mail_params = Parameters::new();
        let mut rcpt_params = Parameters::new();

        if let Some(dsn) = &self.dsn {
            match dsn.ret {
                ReturnContent::Full => mail_params.add(("RET", "FULL")),
                ReturnContent::Hdrs => mail_params.add(("RET", "HDRS")),
            };

            if let Some(envid) = &dsn.envid {
                mail_params.add(("ENVID", envid.as_str()));
            }

            if !dsn.notify.is_empty() {
                let notify_str = dsn
                    .notify
                    .iter()
                    .map(|option| option.to_string())
                    .collect::<Vec<String>>()
                    .join(",");
                rcpt_params.add(("NOTIFY".to_string(), notify_str));
            }

            if let Some(orcpt) = &dsn.orcpt {
                rcpt_params.add(("ORCPT", orcpt.as_str()));
            }
        }

        Ok((mail_params, rcpt_params))
    }

    pub async fn save_to_sent_if_needed(
        &self,
        account_id: u64,
        body: &[u8],
    ) -> RustMailerResult<()> {
        if let Some(true) = self.save_to_sent {
            let encoded_sent_folder = self.resolve_sent_mailbox(account_id).await?;
            let executor = RUST_MAIL_CONTEXT.imap(account_id).await?;
            executor
                .append(&encoded_sent_folder, None, None, body)
                .await?;
        }
        Ok(())
    }

    pub async fn resolve_sent_mailbox(&self, account_id: u64) -> RustMailerResult<String> {
        let executor = RUST_MAIL_CONTEXT.imap(account_id).await?;
        let mailboxes = executor.list_all_mailboxes().await?;

        // Helper closure to check if a mailbox is selectable
        let is_selectable = |attributes: &[NameAttribute]| {
            !attributes
                .iter()
                .any(|attr| matches!(attr, NameAttribute::NoSelect))
        };

        // Case 1: Check for a specific sent folder if provided
        if let Some(sent_folder) = &self.sent_folder {
            let encoded_name = encode_mailbox_name!(sent_folder);
            let matching_mailbox = mailboxes
                .iter()
                .find(|n| is_selectable(n.attributes()) && n.name() == encoded_name);

            return match matching_mailbox {
                Some(mailbox) => Ok(mailbox.name().to_string()),
                None => Err(raise_error!(
                    format!(
                        "Sent folder '{}' unavailable: missing or non-selectable",
                        sent_folder
                    ),
                    ErrorCode::ImapUnexpectedResult
                )),
            };
        }

        // Case 2: Fallback to finding a mailbox with the Sent attribute
        let sent_mailbox = mailboxes.iter().find(|n| {
            is_selectable(n.attributes())
                && n.attributes()
                    .iter()
                    .any(|attr| matches!(attr, NameAttribute::Sent))
        });

        match sent_mailbox {
            Some(mailbox) => Ok(mailbox.name().to_string()),
            None => Err(raise_error!(
                "No selectable Sent mailbox found for this account".into(),
                ErrorCode::ImapUnexpectedResult
            )),
        }
    }
}

pub struct EmailHandler;

const TWO_WEEKS_IN_MS: i64 = 14 * 24 * 60 * 60 * 1000;

impl EmailHandler {
    pub fn validate_send_at(send_at: i64, now: i64) -> Result<(), String> {
        if send_at <= now || send_at > (now + TWO_WEEKS_IN_MS) {
            return Err(
                "send_at must be a future timestamp (in milliseconds) within the next 2 weeks."
                    .into(),
            );
        }
        Ok(())
    }

    pub async fn retrieve_message_content(
        account: &AccountModel,
        envelope: &Envelope,
    ) -> RustMailerResult<Option<FullMessageContent>> {
        let body_meta = match &envelope.body_meta {
            Some(meta) => meta,
            None => return Ok(None),
        };
        let inline_attachments = envelope.attachments.as_ref().map(|atts| {
            atts.iter()
                .filter(|att| att.inline)
                .cloned()
                .collect::<Vec<_>>()
        });
        let request = MessageContentRequest {
            mailbox: Some(envelope.mailbox_name.clone()),
            id: envelope.id.clone(),
            max_length: None,
            sections: Some(body_meta.clone()),
            inline: inline_attachments,
        };
        retrieve_email_content(account.id, request, false)
            .await
            .map(Some)
    }

    pub async fn get_outlook_envelope(
        account: &AccountModel,
        mailbox_name: &str,
        mid: &str,
    ) -> RustMailerResult<OutlookEnvelope> {
        if !account.minimal_sync() {
            if let Ok(folder) = OutlookFolder::get_by_name(account.id, mailbox_name).await {
                let envelope =
                    OutlookEnvelope::get(envelope_hash_from_id(account.id, folder.id, mid)).await?;
                if let Some(envelope) = envelope {
                    return Ok(envelope);
                }
            }
        }
        let message = OutlookClient::get_message(account.id, account.use_proxy, mid).await?;
        Ok(message.try_into()?)
    }

    pub async fn get_gmail_envelope(
        account: &AccountModel,
        label_name: &str,
        mid: &str,
    ) -> RustMailerResult<EmailEnvelopeV3> {
        let map = GmailClient::for_get_label_name(account.id, account.use_proxy).await?;
        if let Ok(label) = GmailLabels::get_by_name(account.id, label_name).await {
            if !account.minimal_sync() {
                let envelope = GmailEnvelope::find(account.id, label.id, mid).await?;
                if let Some(envelope) = envelope {
                    return Ok(envelope.into_v3(&map));
                }
            }
        }
        let message = GmailClient::get_message(account.id, account.use_proxy, mid).await?;
        let envelope: GmailEnvelope = message.try_into()?;
        Ok(envelope.into_v3(&map))
    }

    pub async fn get_envelope(
        account: &AccountModel,
        mailbox_name: &str,
        uid: u32,
    ) -> RustMailerResult<EmailEnvelopeV3> {
        if let Ok(mailbox) = MailBox::get(account.id, mailbox_name).await {
            if !account.minimal_sync() {
                let envelope = EmailEnvelopeV3::find(account.id, mailbox.id, uid).await?;
                if let Some(envelope) = envelope {
                    return Ok(envelope);
                }
            }
        }
        let executor = RUST_MAIL_CONTEXT.imap(account.id).await?;
        let fetches = executor
            .uid_fetch_meta(
                &uid.to_string(),
                encode_mailbox_name!(mailbox_name).as_str(),
                false,
            )
            .await?;
        let first = fetches.first().ok_or_else(|| {
            raise_error!(
                "Could not fetch envelope data.".into(),
                ErrorCode::ImapUnexpectedResult
            )
        })?;
        let envelope = extract_envelope(first, account.id, mailbox_name)?;
        Ok(envelope)
    }

    pub async fn mark_message_answered(
        account_id: u64,
        mailbox_name: &str,
        uid: u32,
    ) -> RustMailerResult<()> {
        let executor = RUST_MAIL_CONTEXT.imap(account_id).await?;
        executor
            .uid_set_flags(
                uid.to_string().as_str(),
                &encode_mailbox_name!(mailbox_name),
                Some(vec![EnvelopeFlag::new(EmailFlag::Answered, None)]),
                None,
                None,
            )
            .await?;
        Ok(())
    }

    pub fn to_address(address: &[EmailAddress]) -> RustMailerResult<Address<'static>> {
        if address.is_empty() {
            return Err(raise_error!(
                "Email address list cannot be empty".into(),
                ErrorCode::ImapUnexpectedResult
            ));
        }

        if address.len() == 1 {
            let email = &address[0];
            Ok(match &email.name {
                Some(name) => Address::from((name.clone(), email.address.clone())),
                None => Address::from(email.address.clone()),
            })
        } else {
            let addresses = address
                .iter()
                .map(|email| match &email.name {
                    Some(name) => Address::from((name.clone(), email.address.clone())),
                    None => Address::from(email.address.clone()),
                })
                .collect::<Vec<Address<'static>>>();
            Ok(Address::new_list(addresses))
        }
    }

    async fn add_attachment(
        mut builder: MessageBuilder<'static>,
        imap_attachments: Option<&[ImapAttachment]>,
        attachments: Option<&[AttachmentInfo]>,
        envelope: &Envelope,
        account: &AccountModel,
    ) -> RustMailerResult<MessageBuilder<'static>> {
        match account.mailer_type {
            MailerType::ImapSmtp => {
                if let Some(attachments) = imap_attachments {
                    for attachment in attachments {
                        let uid = envelope.id.parse::<u32>().ok().ok_or_else(|| {
                            raise_error!(
                                "Invalid IMAP UID: `id` must be a numeric string".into(),
                                ErrorCode::InvalidParameter
                            )
                        })?;
                        let attachment_ref = AttachmentRef {
                            mailbox_name: envelope.mailbox_name.clone(),
                            uid,
                            attachment_data: attachment.clone(),
                        };
                        let mime_type = from_ext(&attachment.file_type)
                            .first_or_octet_stream()
                            .to_string();
                        let request = AttachmentRequest {
                            id: attachment_ref.uid.to_string(),
                            mailbox: Some(attachment_ref.mailbox_name.clone()),
                            attachment: Some(attachment_ref.attachment_data.clone()),
                            attachment_info: None,
                            filename: None,
                            attachment_id: None,
                        };
                        let content = MailAttachment::retrieve_and_decode_attachment(
                            request, &mime_type, account,
                        )
                        .await?;

                        if attachment.inline {
                            builder = builder.inline(
                                mime_type,
                                attachment.content_id.clone().ok_or_else(|| {
                                    raise_error!(
                                        "Missing content_id".into(),
                                        ErrorCode::ImapUnexpectedResult
                                    )
                                })?,
                                content,
                            );
                        } else {
                            builder = builder.attachment(
                                mime_type,
                                attachment.filename.clone().ok_or_else(|| {
                                    raise_error!(
                                        "Missing filename".into(),
                                        ErrorCode::ImapUnexpectedResult
                                    )
                                })?,
                                content,
                            );
                        }
                    }
                }
            }
            _ => {
                if let Some(attachments) = attachments {
                    for attachment in attachments {
                        let mime_type = attachment.file_type.clone();
                        let request = AttachmentRequest {
                            id: envelope.id.clone(),
                            mailbox: Some(envelope.mailbox_name.clone()),
                            attachment: None,
                            attachment_info: Some(attachment.clone()),
                            filename: None,
                            attachment_id: Some(attachment.id.clone()),
                        };
                        let content = MailAttachment::retrieve_and_decode_attachment(
                            request, &mime_type, account,
                        )
                        .await?;

                        if attachment.inline {
                            builder = builder.inline(
                                mime_type,
                                attachment.content_id.clone().ok_or_else(|| {
                                    raise_error!(
                                        "Missing content_id".into(),
                                        ErrorCode::ImapUnexpectedResult
                                    )
                                })?,
                                content,
                            );
                        } else {
                            builder =
                                builder.attachment(mime_type, attachment.filename.clone(), content);
                        }
                    }
                }
            }
        }

        Ok(builder)
    }

    pub fn insert_preview(preview: &Option<String>, html: String) -> String {
        if let Some(preview) = preview {
            EmailPreview::insert_preview_into_html(&html, preview)
        } else {
            html
        }
    }

    pub async fn schedule_task(
        account: &AccountModel,
        subject: Option<String>,
        message_id: String,
        cc: Option<Vec<EmailAddress>>,
        bcc: Option<Vec<EmailAddress>>,
        attachment_count: usize,
        builder: MessageBuilder<'_>,
        send_control: Option<SendControl>,
        send_at: Option<i64>,
        answer_email: Option<AnswerEmail>,
    ) -> RustMailerResult<()> {
        let message = builder.into_message().map_err(|e| {
            raise_error!(
                format!("Failed to build message: {}", e),
                ErrorCode::InternalError
            )
        })?;
        // Skip sending if dry_run is enabled; used for testing or simulation.
        if let Some(send_control) = &send_control {
            if let Some(true) = send_control.dry_run {
                return Ok(());
            }
        }

        let cache_key = generate_token!(128);
        DISK_CACHE
            .put_cache(&cache_key, &message.body, true)
            .await?;

        let task = SmtpTask {
            account_id: account.id,
            account_email: account.email.clone(),
            subject,
            message_id,
            cc: Self::extract_address(cc),
            bcc: Self::extract_address(bcc),
            attachment_count,
            control: send_control,
            from: message.mail_from.email.to_string(),
            to: message
                .rcpt_to
                .into_iter()
                .map(|t| t.email.to_string())
                .collect(),
            cache_key,
            answer_email,
            delete_draft_uid: None,
            delete_draft_mailbox: None,
        };

        let delay_seconds = send_at
            .map(|sent_at| {
                let diff_ms = sent_at - utc_now!();
                if diff_ms < 0 {
                    None
                } else {
                    Some((diff_ms / 1000) as u32)
                }
            })
            .unwrap_or(None);

        RustMailerTaskQueue::get()?
            .submit_task(task, delay_seconds)
            .await?;

        Ok(())
    }

    pub fn extract_address(f: Option<Vec<EmailAddress>>) -> Option<Vec<String>> {
        f.map(|vec| vec.into_iter().map(|email| email.address).collect())
    }
}
