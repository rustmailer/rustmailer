// Copyright © 2025-2026 rustmailer.com
// Licensed under RustMailer License Agreement v1.0
// Unauthorized copying, modification, or distribution is prohibited.

use std::{borrow::Cow, collections::HashSet};

use mail_send::{
    mail_builder::{headers::address::Address, MessageBuilder},
    smtp::message::IntoMessage,
};
use poem_openapi::Object;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{
    base64_encode_url_safe,
    modules::{
        account::{entity::MailerType, migration::AccountModel},
        cache::{
            imap::mailbox::AttributeEnum,
            model::Envelope,
            vendor::{
                gmail::sync::{client::GmailClient, envelope::GmailEnvelope},
                outlook::sync::client::OutlookClient,
            },
        },
        common::Addr,
        context::executors::RUST_MAIL_CONTEXT,
        error::{code::ErrorCode, RustMailerResult},
        mailbox::list::request_imap_all_mailbox_list,
        smtp::{
            request::{
                reply::{apply_references, apply_references2},
                EmailHandler,
            },
            util::generate_message_id,
        },
    },
    raise_error,
};

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize, Object)]
pub struct AppendReplyToDraftRequest {
    /// The name of the mailbox or label containing the original message.
    ///
    /// - For IMAP accounts, this is the mailbox name where the source message resides.
    /// - For Gmail API accounts, this refers to the label name associated with the source message.
    /// This is used to locate the message being replied to.
    pub mailbox_name: Option<String>,
    /// The unique ID of the message, either IMAP UID or Gmail API MID.
    ///
    /// - For IMAP accounts, this is the UID converted to a string. It must be a valid numeric string
    ///   that can be parsed back to a `u32`.
    /// - For Gmail API accounts, this is the message ID (`mid`) returned by the API.
    pub id: String,
    /// A preview text for the reply email.
    ///
    /// This optional field provides a short summary or preview of the reply content.
    #[oai(validator(min_length = "1", max_length = "200"))]
    pub preview: Option<String>,
    /// The plain text body of the reply email.
    ///
    /// This field is optional and can be used to provide plain text content.
    #[oai(validator(min_length = "1"))]
    pub text: Option<String>,
    /// The HTML body of the reply email.
    ///
    /// This field is optional and can be used to provide HTML content.
    #[oai(validator(min_length = "1"))]
    pub html: Option<String>,
    /// Indicates whether the reply should be created as “Reply All”.
    ///
    /// - `Some(true)` creates a 'Reply All' message.
    /// - `Some(false)` creates a normal 'Reply' message.
    /// - `None` is treated the same as `false` (normal 'Reply').
    pub reply_all: Option<bool>,
}

impl AppendReplyToDraftRequest {
    fn validate(&self, gmail_or_graph: bool) -> RustMailerResult<()> {
        if !gmail_or_graph {
            // IMAP account: uid and mailbox_name required
            if self.id.parse::<u32>().is_err() {
                return Err(raise_error!(
                    "Invalid IMAP UID: `id` must be a numeric string".into(),
                    ErrorCode::InvalidParameter
                ));
            }

            // IMAP account: mailbox_name must be present
            if self
                .mailbox_name
                .as_ref()
                .map(|s| s.is_empty())
                .unwrap_or(true)
            {
                return Err(raise_error!(
                    "For IMAP accounts, `mailbox_name` is required".into(),
                    ErrorCode::InvalidParameter
                ));
            }
        }
        Ok(())
    }

    pub async fn append_reply_to_draft(&self, account_id: u64) -> RustMailerResult<ReplyDraft> {
        let account = AccountModel::check_account_active(account_id, false).await?;
        self.validate(matches!(
            account.mailer_type,
            MailerType::GmailApi | MailerType::GraphApi
        ))?;

        match account.mailer_type {
            MailerType::ImapSmtp => self.append_reply_to_draft_imap(&account).await,
            MailerType::GmailApi => self.append_reply_to_draft_gmail(&account, account_id).await,
            MailerType::GraphApi => {
                OutlookClient::create_reply(
                    account_id,
                    account.use_proxy,
                    &self.id,
                    self.text.as_deref(),
                    self.html.as_deref(),
                    self.reply_all.unwrap_or_default(),
                )
                .await
            }
        }
    }

    async fn append_reply_to_draft_imap(
        &self,
        account: &AccountModel,
    ) -> RustMailerResult<ReplyDraft> {
        let mailboxes = request_imap_all_mailbox_list(account.id).await?;
        let drafts_mailbox = mailboxes
            .iter()
            .find(|mb| {
                mb.attributes
                    .iter()
                    .any(|attr| matches!(attr.attr, AttributeEnum::Drafts))
            })
            .ok_or_else(|| {
                raise_error!(
                    "Cannot find Drafts mailbox in the account".into(),
                    ErrorCode::InternalError
                )
            })?;

        let envelope: Envelope = EmailHandler::get_envelope(
            account,
            self.mailbox_name.as_deref().unwrap(),
            self.id.parse::<u32>().ok().unwrap(),
        )
        .await?
        .into();

        let from = Address::new_address(
            account.name.as_ref().map(|n| Cow::Owned(n.to_string())),
            Cow::Owned(account.email.clone()),
        );
        //reply
        let mut to = match &envelope.reply_to {
            Some(reply_to) if !reply_to.is_empty() => reply_to.clone(),
            _ => envelope
                .from
                .clone()
                .map(|from| vec![from])
                .ok_or_else(|| {
                    raise_error!(
                        "Invalid email envelope: missing both 'reply_to' and 'from'".into(),
                        ErrorCode::InvalidParameter
                    )
                })?,
        };

        if self.reply_all.unwrap_or_default() {
            if let Some(to_list) = &envelope.to {
                to.extend(to_list.clone());
            }
            if let Some(cc_list) = &envelope.cc {
                to.extend(cc_list.clone());
            }
            Self::dedupe_addrs(&mut to);
            Self::remove_self(&mut to, &account.email);
        }

        let subject = format!("Re: {}", envelope.subject.as_deref().unwrap_or(""));
        let mut builder = MessageBuilder::new()
            .from(from)
            .to(Address::from(to.clone()))
            .subject(subject);
        let message_id = generate_message_id();
        builder = builder.message_id(message_id.clone());
        builder = apply_references(builder, &envelope)?;
        builder = self.apply_content(builder)?;
        let message = builder.into_message().map_err(|e| {
            raise_error!(
                format!("Failed to build message: {}", e),
                ErrorCode::InternalError
            )
        })?;

        let executor = RUST_MAIL_CONTEXT.imap(account.id).await?;
        executor
            .append(
                drafts_mailbox.encoded_name().as_str(),
                None,
                None,
                message.body,
            )
            .await?;
        //Why not use UID SEARCH? Because it’s unreliable—searching by Message-ID may not consistently return results,
        //possibly due to differences in how the IMAP server is implemented.
        let uid = executor
            .get_uid_by_message_id(
                message_id.trim_matches(['<', '>'].as_ref()),
                &drafts_mailbox.encoded_name(),
            )
            .await?;
        Ok(ReplyDraft {
            id: uid.to_string(),
            draft_id: None,
            draft_folder: drafts_mailbox.name.clone(),
        })
    }

    fn dedupe_addrs(addrs: &mut Vec<Addr>) {
        let mut seen = HashSet::new();

        addrs.retain(|addr| {
            if let Some(email) = &addr.address {
                let email_lower = email.to_lowercase();
                if seen.contains(&email_lower) {
                    return false;
                }
                seen.insert(email_lower);
            }
            true
        });
    }

    fn remove_self(addrs: &mut Vec<Addr>, my_email: &str) {
        let my_email_lower = my_email.trim().to_lowercase();

        addrs.retain(|addr| match &addr.address {
            Some(addr_email) => addr_email.trim().to_lowercase() != my_email_lower,
            None => true,
        });
    }

    async fn append_reply_to_draft_gmail(
        &self,
        account: &AccountModel,
        account_id: u64,
    ) -> RustMailerResult<ReplyDraft> {
        let envelope = GmailClient::get_message(account_id, account.use_proxy, &self.id).await?;
        let envelope: GmailEnvelope = envelope.try_into()?;
        let from = Address::new_address(
            account.name.as_ref().map(|n| Cow::Owned(n.to_string())),
            Cow::Owned(account.email.clone()),
        );

        let mut to = match &envelope.reply_to {
            Some(reply_to) if !reply_to.is_empty() => reply_to.clone(),
            _ => envelope
                .from
                .clone()
                .map(|from| vec![from])
                .ok_or_else(|| {
                    raise_error!(
                        "Invalid email envelope: missing both 'reply_to' and 'from'".into(),
                        ErrorCode::InvalidParameter
                    )
                })?,
        };

        if self.reply_all.unwrap_or_default() {
            if let Some(to_list) = &envelope.to {
                to.extend(to_list.clone());
            }
            if let Some(cc_list) = &envelope.cc {
                to.extend(cc_list.clone());
            }
            Self::dedupe_addrs(&mut to);
            Self::remove_self(&mut to, &account.email);
        }

        let subject = format!("Re: {}", envelope.subject.as_deref().unwrap_or(""));
        let mut builder = MessageBuilder::new()
            .from(from)
            .to(Address::from(to.clone()))
            .subject(subject);
        builder = apply_references2(builder, &envelope)?;
        builder = self.apply_content(builder)?;
        let message = builder.into_message().map_err(|e| {
            raise_error!(
                format!("Failed to build message: {}", e),
                ErrorCode::InternalError
            )
        })?;

        let raw_encoded = base64_encode_url_safe!(&message.body);
        let body = json!({
            "message": {
                "threadId": envelope.gmail_thread_id,
                "raw": raw_encoded
            }
        });

        GmailClient::create_draft(account_id, account.use_proxy, body).await
    }

    fn apply_content(
        &self,
        mut builder: MessageBuilder<'static>,
    ) -> RustMailerResult<MessageBuilder<'static>> {
        if let Some(html) = &self.html {
            let html = EmailHandler::insert_preview(&self.preview, html.clone());
            builder = builder.html_body(html);
        }
        if let Some(text) = &self.text {
            builder = builder.text_body(text.clone());
        }
        Ok(builder)
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize, Object)]
pub struct ReplyDraft {
    /// Message identifier:
    /// - For IMAP accounts, this is the UID of the message;
    /// - For Gmail / Graph API, this is the message ID.
    pub id: String,
    /// The provider-specific draft identifier.
    ///
    /// This is only populated for Gmail API accounts, where it holds the Gmail draft ID
    /// (distinct from the message ID stored in `id`). For IMAP and Graph API accounts,
    /// this field is `None` — use `id` directly.
    pub draft_id: Option<String>,
    /// Draft folder name:
    /// - In IMAP, this is the name of the drafts folder;
    /// - In Gmail API, this is the name of the draft label;
    /// - In Graph API, this is the name of the drafts folder;
    /// These names can all be used as `mailbox_name` in RustMailer API.
    pub draft_folder: String,
}
