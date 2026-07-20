// Copyright © 2025-2026 rustmailer.com
// Licensed under RustMailer License Agreement v1.0
// Unauthorized copying, modification, or distribution is prohibited.

use crate::modules::{
    common::Addr,
    grpc::service::rustmailer_grpc,
    message::append::ReplyDraft as RustMailerReplyDraft,
    rest::response::DataPage,
    scheduler::model::TaskStatus,
    smtp::{
        queue::message::SendEmailTask,
        request::{
            forward::ForwardEmailRequest,
            headers::{HeaderValue, Raw, Text, Url},
            new::{Recipient, SendEmailRequest},
            reply::ReplyEmailRequest,
            AttachmentPayload, AttachmentRef, DSNConfig, EmailAddress, MailAttachment,
            MailEnvelope, NotifyOption, Retry, ReturnContent, SendControl, Strategy,
        },
    },
    utils::prost_value_to_json_value,
};
use std::collections::HashMap;

impl TryFrom<rustmailer_grpc::SendEmailRequest> for SendEmailRequest {
    type Error = &'static str;

    fn try_from(value: rustmailer_grpc::SendEmailRequest) -> Result<Self, Self::Error> {
        Ok(Self {
            from: value.from.map(Into::into),
            recipients: value.recipients.into_iter().map(Into::into).collect(),
            subject: value.subject,
            text: value.text,
            html: value.html,
            preview: value.preview,
            eml: value.eml,
            template_id: value.template_id,
            attachments: value
                .attachments
                .into_iter()
                .map(MailAttachment::try_from)
                .collect::<Result<Vec<_>, _>>()
                .ok()
                .filter(|v| !v.is_empty()),
            headers: {
                if value.headers.is_empty() {
                    None
                } else {
                    let mut new_headers = HashMap::new();
                    for (key, value) in value.headers.into_iter() {
                        let converted_value = HeaderValue::try_from(value)?;
                        new_headers.insert(key, converted_value);
                    }
                    Some(new_headers)
                }
            },
            send_control: value.send_control.map(|c| c.try_into()).transpose()?,
        })
    }
}

impl TryFrom<rustmailer_grpc::ReplyEmailRequest> for ReplyEmailRequest {
    type Error = &'static str;

    fn try_from(value: rustmailer_grpc::ReplyEmailRequest) -> Result<Self, Self::Error> {
        Ok(Self {
            mailbox_name: value.mailbox_name,
            id: value.id,
            text: value.text,
            html: value.html,
            preview: value.preview,
            headers: {
                if value.headers.is_empty() {
                    None
                } else {
                    let mut new_headers = HashMap::new();
                    for (key, value) in value.headers.into_iter() {
                        let converted_value = HeaderValue::try_from(value)?;
                        new_headers.insert(key, converted_value);
                    }
                    Some(new_headers)
                }
            },
            reply_all: value.reply_all,
            attachments: (!value.attachments.is_empty())
                .then(|| {
                    value
                        .attachments
                        .into_iter()
                        .map(MailAttachment::try_from)
                        .collect::<Result<Vec<_>, _>>()
                })
                .transpose()?,

            cc: (!value.cc.is_empty()).then(|| value.cc.into_iter().map(Into::into).collect()),
            bcc: (!value.bcc.is_empty()).then(|| value.bcc.into_iter().map(Into::into).collect()),
            timezone: value.timezone,
            include_original: value.include_original,
            include_all_attachments: value.include_all_attachments,
            send_control: { value.send_control.map(|c| c.try_into()).transpose()? },
        })
    }
}

impl TryFrom<rustmailer_grpc::ForwardEmailRequest> for ForwardEmailRequest {
    type Error = &'static str;

    fn try_from(value: rustmailer_grpc::ForwardEmailRequest) -> Result<Self, Self::Error> {
        Ok(Self {
            mailbox_name: value.mailbox_name,
            id: value.id,
            to: value.to.into_iter().map(Into::into).collect(),
            cc: (!value.cc.is_empty()).then(|| value.cc.into_iter().map(Into::into).collect()),
            bcc: (!value.bcc.is_empty()).then(|| value.bcc.into_iter().map(Into::into).collect()),
            text: value.text,
            html: value.html,
            preview: value.preview,
            headers: {
                if value.headers.is_empty() {
                    None
                } else {
                    let mut new_headers = HashMap::new();
                    for (key, value) in value.headers.into_iter() {
                        let converted_value = HeaderValue::try_from(value)?;
                        new_headers.insert(key, converted_value);
                    }
                    Some(new_headers)
                }
            },
            timezone: value.timezone,
            attachments: (!value.attachments.is_empty())
                .then(|| {
                    value
                        .attachments
                        .into_iter()
                        .map(MailAttachment::try_from)
                        .collect::<Result<Vec<_>, _>>()
                })
                .transpose()?,
            include_original: value.include_original,
            include_all_attachments: value.include_all_attachments,
            send_control: { value.send_control.map(|c| c.try_into()).transpose()? },
        })
    }
}

impl TryFrom<rustmailer_grpc::SendControl> for SendControl {
    type Error = &'static str;

    fn try_from(value: rustmailer_grpc::SendControl) -> Result<Self, Self::Error> {
        Ok(Self {
            envelope: value.envelope.map(Into::into),
            save_to_sent: value.save_to_sent,
            sent_folder: value.sent_folder,
            dry_run: value.dry_run,
            send_at: value.send_at,
            retry_policy: value.retry_policy.map(Retry::try_from).transpose()?,
            mta: value.mta,
            dsn: value.dsn.map(DSNConfig::try_from).transpose()?,
            campaign_id: value.campaign_id,
            enable_tracking: value.enable_tracking,
        })
    }
}

impl TryFrom<rustmailer_grpc::DsnConfig> for DSNConfig {
    type Error = &'static str;

    fn try_from(value: rustmailer_grpc::DsnConfig) -> Result<Self, Self::Error> {
        Ok(Self {
            ret: value.ret.try_into()?,
            envid: value.envid,
            notify: value
                .notify
                .into_iter()
                .map(NotifyOption::try_from)
                .collect::<Result<Vec<NotifyOption>, _>>()?,
            orcpt: value.orcpt,
        })
    }
}

impl From<DSNConfig> for rustmailer_grpc::DsnConfig {
    fn from(value: DSNConfig) -> Self {
        Self {
            ret: value.ret.into(),
            envid: value.envid,
            notify: value.notify.into_iter().map(Into::into).collect(),
            orcpt: value.orcpt,
        }
    }
}

impl TryFrom<i32> for NotifyOption {
    type Error = &'static str;

    fn try_from(value: i32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Success),
            1 => Ok(Self::Failure),
            2 => Ok(Self::Delay),
            3 => Ok(Self::Never),
            _ => Err("Invalid value for NotifyOption"),
        }
    }
}

impl From<NotifyOption> for i32 {
    fn from(value: NotifyOption) -> Self {
        match value {
            NotifyOption::Success => 0,
            NotifyOption::Failure => 1,
            NotifyOption::Delay => 2,
            NotifyOption::Never => 3,
        }
    }
}

impl TryFrom<i32> for ReturnContent {
    type Error = &'static str;

    fn try_from(value: i32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Full),
            1 => Ok(Self::Hdrs),
            _ => Err("Invalid value for ReturnContent"),
        }
    }
}

impl From<ReturnContent> for i32 {
    fn from(value: ReturnContent) -> Self {
        match value {
            ReturnContent::Full => 0,
            ReturnContent::Hdrs => 1,
        }
    }
}

impl From<rustmailer_grpc::MailEnvelope> for MailEnvelope {
    fn from(value: rustmailer_grpc::MailEnvelope) -> Self {
        Self {
            from: value.from,
            recipients: value.recipients,
        }
    }
}

impl From<MailEnvelope> for rustmailer_grpc::MailEnvelope {
    fn from(value: MailEnvelope) -> Self {
        Self {
            from: value.from,
            recipients: value.recipients,
        }
    }
}

impl TryFrom<rustmailer_grpc::Retry> for Retry {
    type Error = &'static str;

    fn try_from(value: rustmailer_grpc::Retry) -> Result<Self, Self::Error> {
        Ok(Self {
            strategy: value.strategy.try_into()?,
            seconds: value.seconds,
            max_retries: value.max_retries,
        })
    }
}

impl TryFrom<i32> for Strategy {
    type Error = &'static str;

    fn try_from(value: i32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Linear),
            1 => Ok(Self::Exponential),
            _ => Err("Invalid value for Strategy"),
        }
    }
}

impl From<rustmailer_grpc::EmailAddress> for EmailAddress {
    fn from(value: rustmailer_grpc::EmailAddress) -> Self {
        Self {
            name: value.name,
            address: value.address,
        }
    }
}

impl From<rustmailer_grpc::Recipient> for Recipient {
    fn from(value: rustmailer_grpc::Recipient) -> Self {
        Self {
            to: value.to.into_iter().map(Into::into).collect(),
            cc: (!value.cc.is_empty()).then(|| value.cc.into_iter().map(Into::into).collect()),
            bcc: (!value.bcc.is_empty()).then(|| value.bcc.into_iter().map(Into::into).collect()),
            reply_to: (!value.reply_to.is_empty())
                .then(|| value.reply_to.into_iter().map(Into::into).collect()),
            template_params: value.template_params.map(prost_value_to_json_value),
            send_at: value.send_at,
        }
    }
}

impl TryFrom<rustmailer_grpc::AttachmentRef> for AttachmentRef {
    type Error = &'static str;
    fn try_from(value: rustmailer_grpc::AttachmentRef) -> Result<Self, Self::Error> {
        Ok(Self {
            mailbox_name: value.mailbox_name,
            uid: value.uid,
            attachment_data: {
                value
                    .attachment_data
                    .ok_or("field 'attachment_data' missing")?
                    .try_into()?
            },
        })
    }
}

impl TryFrom<rustmailer_grpc::AttachmentPayload> for AttachmentPayload {
    type Error = &'static str;

    fn try_from(value: rustmailer_grpc::AttachmentPayload) -> Result<Self, Self::Error> {
        match value.payload_type {
            Some(payload_type) => match payload_type {
                rustmailer_grpc::attachment_payload::PayloadType::Base64Content(v) => Ok(Self {
                    base64_content: Some(v),
                    attachment_ref: None,
                }),
                rustmailer_grpc::attachment_payload::PayloadType::AttachmentRef(attachment_ref) => {
                    Ok(Self {
                        base64_content: None,
                        attachment_ref: Some(attachment_ref.try_into()?),
                    })
                }
            },
            None => Ok(Self {
                base64_content: None,
                attachment_ref: None,
            }),
        }
    }
}

impl TryFrom<rustmailer_grpc::MailAttachment> for MailAttachment {
    type Error = &'static str;

    fn try_from(value: rustmailer_grpc::MailAttachment) -> Result<Self, Self::Error> {
        Ok(Self {
            file_name: value.file_name,
            payload: { value.payload.ok_or("field 'payload' missing")?.try_into()? },
            mime_type: value.mime_type,
            inline: value.inline,
            content_id: value.content_id,
        })
    }
}

impl TryFrom<rustmailer_grpc::HeaderValue> for HeaderValue {
    type Error = &'static str;
    fn try_from(value: rustmailer_grpc::HeaderValue) -> Result<Self, Self::Error> {
        match value.value {
            Some(value) => match value {
                rustmailer_grpc::header_value::Value::Raw(raw) => Ok(HeaderValue::Raw(raw.into())),
                rustmailer_grpc::header_value::Value::Text(text) => {
                    Ok(HeaderValue::Text(text.into()))
                }
                rustmailer_grpc::header_value::Value::Url(url) => Ok(HeaderValue::Url(url.into())),
            },
            None => Err("HeaderValue is missing"),
        }
    }
}

impl From<rustmailer_grpc::Addr> for Addr {
    fn from(value: rustmailer_grpc::Addr) -> Addr {
        Self {
            name: value.name,
            address: value.address,
        }
    }
}

impl From<rustmailer_grpc::Raw> for Raw {
    fn from(value: rustmailer_grpc::Raw) -> Self {
        Self { raw: value.raw }
    }
}

impl From<rustmailer_grpc::Text> for Text {
    fn from(value: rustmailer_grpc::Text) -> Self {
        Self { text: value.text }
    }
}

impl From<rustmailer_grpc::Url> for Url {
    fn from(value: rustmailer_grpc::Url) -> Self {
        Self { url: value.url }
    }
}

impl From<SendEmailTask> for rustmailer_grpc::EmailTask {
    fn from(value: SendEmailTask) -> Self {
        Self {
            id: value.id,
            created_at: value.created_at,
            status: value.status.into(),
            stopped_reason: value.stopped_reason,
            error: value.error,
            last_duration_ms: value.last_duration_ms.map(|s| s as u64),
            retry_count: value.retry_count.map(|s| s as u32),
            scheduled_at: value.scheduled_at,
            account_id: value.account_id,
            account_email: value.account_email,
            subject: value.subject,
            message_id: value.message_id,
            from: value.from,
            to: value.to,
            cc: value.cc.unwrap_or_default(),
            bcc: value.bcc.unwrap_or_default(),
            attachment_count: value.attachment_count as u32,
            cache_key: value.cache_key,
            envelope: value.envelope.map(Into::into),
            save_to_sent: value.save_to_sent,
            sent_folder: value.sent_folder,
            send_at: value.send_at,
            mta: value.mta,
            dsn: value.dsn.map(Into::into),
            reply: value.reply,
            mailbox: value.mailbox,
            uid: value.uid,
        }
    }
}

impl From<TaskStatus> for i32 {
    fn from(value: TaskStatus) -> Self {
        match value {
            TaskStatus::Scheduled => 0,
            TaskStatus::Running => 1,
            TaskStatus::Success => 2,
            TaskStatus::Failed => 3,
            TaskStatus::Removed => 4,
            TaskStatus::Stopped => 5,
        }
    }
}

impl TryFrom<i32> for TaskStatus {
    type Error = &'static str;

    fn try_from(value: i32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(TaskStatus::Scheduled),
            1 => Ok(TaskStatus::Running),
            2 => Ok(TaskStatus::Success),
            3 => Ok(TaskStatus::Failed),
            4 => Ok(TaskStatus::Removed),
            5 => Ok(TaskStatus::Stopped),
            _ => Err("Invalid value for TaskStatus"),
        }
    }
}

impl From<DataPage<SendEmailTask>> for rustmailer_grpc::PagedEmailTask {
    fn from(value: DataPage<SendEmailTask>) -> Self {
        Self {
            current_page: value.current_page,
            page_size: value.page_size,
            total_items: value.total_items,
            items: value.items.into_iter().map(Into::into).collect(),
            total_pages: value.total_pages,
        }
    }
}

impl From<RustMailerReplyDraft> for rustmailer_grpc::ReplyDraft {
    fn from(value: RustMailerReplyDraft) -> Self {
        Self {
            id: value.id,
            draft_id: value.draft_id,
            draft_folder: value.draft_folder,
        }
    }
}
