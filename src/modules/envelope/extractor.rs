// Copyright © 2025-2026 rustmailer.com
// Licensed under RustMailer License Agreement v1.0
// Unauthorized copying, modification, or distribution is prohibited.

use crate::id;
use crate::modules::cache::imap::flags_to_hash;
use crate::modules::cache::imap::mailbox::EnvelopeFlag;
use crate::modules::cache::imap::minimal::MinimalEnvelope;
use crate::modules::cache::imap::migration::EmailEnvelopeV3;
use crate::modules::common::AddrVec;
use crate::modules::envelope::MinimalEnvelopeMeta;
use crate::modules::error::code::ErrorCode;
use crate::modules::error::RustMailerResult;
use crate::modules::imap::section::SectionExtractor;
use crate::modules::utils::mailbox_id;
use crate::raise_error;
use async_imap::types::{Fetch, Flag};
use mail_parser::{Message, MessageParser};

#[inline]
pub fn extract_envelope(
    fetch: &Fetch,
    account_id: u64,
    mailbox_name: &str,
) -> RustMailerResult<EmailEnvelopeV3> {
    let attachments: Option<Vec<crate::modules::imap::section::ImapAttachment>> =
        SectionExtractor::new(fetch.bodystructure().ok_or_else(|| {
            raise_error!(
                "No bodystructure available".into(),
                ErrorCode::InternalError
            )
        })?)
        .get_attachments();

    let body = SectionExtractor::new(fetch.bodystructure().ok_or_else(|| {
        raise_error!(
            "No bodystructure available".into(),
            ErrorCode::InternalError
        )
    })?)
    .get_body_parts();

    let flags: Vec<EnvelopeFlag> = fetch
        .flags()
        .filter(|f| !matches!(f, Flag::Recent))
        .map(Into::into)
        .collect();

    let flags_hash = flags_to_hash(&flags);

    let internal_date = fetch.internal_date().map(|d| d.timestamp_millis());

    let uid = fetch
        .uid
        .ok_or_else(|| raise_error!("No uid available".into(), ErrorCode::InternalError))?;
    let size = fetch
        .size
        .ok_or_else(|| raise_error!("No size available".into(), ErrorCode::InternalError))?;

    let header = fetch
        .header()
        .ok_or_else(|| raise_error!("No header available".into(), ErrorCode::InternalError))?;
    let message = MessageParser::new().parse(header).ok_or_else(|| {
        raise_error!(
            "Email header parse result is not available".into(),
            ErrorCode::InternalError
        )
    })?;

    let mut envelope = EmailEnvelopeV3 {
        account_id,
        mailbox_id: mailbox_id(account_id, mailbox_name),
        mailbox_name: mailbox_name.into(),
        uid,
        internal_date,
        size,
        flags,
        flags_hash,
        bcc: message.bcc().map(|addr| AddrVec::from(addr).0),
        cc: message.cc().map(|addr| AddrVec::from(addr).0),
        date: message.date().map(|d| d.to_timestamp() * 1000),
        from: message
            .from()
            .map(|addr| AddrVec::from(addr).0.first().cloned())
            .flatten(),
        in_reply_to: message.in_reply_to().as_text().map(String::from),
        sender: message
            .sender()
            .map(|addr| AddrVec::from(addr).0.first().cloned())
            .flatten(),
        return_address: message.return_address().map(String::from),
        message_id: message.message_id().map(String::from),
        subject: message.subject().map(String::from),
        mime_version: message.mime_version().as_text().map(String::from),
        thread_id: id!(64),
        thread_name: message.thread_name().map(String::from),
        references: extract_references(&message),
        reply_to: message.reply_to().map(|addr| AddrVec::from(addr).0),
        to: message.to().map(|addr| AddrVec::from(addr).0),
        attachments,
        body_meta: body,
        received: message.received().map(Into::into),
        mid: None,
        labels: vec![],
    };
    envelope.thread_id = envelope.compute_thread_id();

    Ok(envelope)
}

pub fn extract_minimal_envelope_meta(fetch: &Fetch) -> RustMailerResult<MinimalEnvelopeMeta> {
    let attachments = SectionExtractor::new(fetch.bodystructure().ok_or_else(|| {
        raise_error!(
            "No bodystructure available".into(),
            ErrorCode::InternalError
        )
    })?)
    .get_attachments();
    let size = fetch
        .size
        .ok_or_else(|| raise_error!("No size available".into(), ErrorCode::InternalError))?;

    Ok(MinimalEnvelopeMeta { size, attachments })
}

#[inline]
pub fn extract_rich_envelopes(
    fetches: &Vec<Fetch>,
    account_id: u64,
    mailbox_name: &str,
) -> RustMailerResult<Vec<EmailEnvelopeV3>> {
    let mut envelopes = Vec::with_capacity(fetches.len());
    for fetch in fetches {
        let envelope = extract_envelope(fetch, account_id, mailbox_name)?;
        envelopes.push(envelope);
    }
    Ok(envelopes)
}

#[inline]
pub fn extract_minimal_envelopes(
    fetches: &[Fetch],
    account_id: u64,
    mailbox_id: u64,
) -> RustMailerResult<Vec<MinimalEnvelope>> {
    let mut envelopes = Vec::with_capacity(fetches.len());
    for fetch in fetches {
        let uid = fetch
            .uid
            .ok_or_else(|| raise_error!("No uid available".into(), ErrorCode::InternalError))?;
        let flags: Vec<EnvelopeFlag> = fetch
            .flags()
            .filter(|f| !matches!(f, Flag::Recent))
            .map(Into::into)
            .collect();

        let flags_hash = flags_to_hash(&flags);

        envelopes.push(MinimalEnvelope {
            account_id,
            mailbox_id,
            uid,
            flags_hash,
        });
    }
    Ok(envelopes)
}

type Uid = u32;
type FlagsHash = u64;
type EnvelopeFlags = Vec<EnvelopeFlag>;
type UidMetadata = (Uid, (FlagsHash, EnvelopeFlags));
type FetchMetadataList = Vec<UidMetadata>;

pub fn parse_fetch_metadata(
    fetches: Vec<Fetch>,
    uid_only: bool,
) -> RustMailerResult<FetchMetadataList> {
    let mut result = Vec::with_capacity(fetches.len());

    for fetch in fetches.iter() {
        let flags: Vec<EnvelopeFlag> = if uid_only {
            vec![]
        } else {
            fetch
                .flags()
                .filter(|f| !matches!(f, Flag::Recent))
                .map(Into::into)
                .collect()
        };

        let flags_hash = if uid_only { 0 } else { flags_to_hash(&flags) };
        let uid = fetch
            .uid
            .ok_or_else(|| raise_error!("No uid available".into(), ErrorCode::InternalError))?;

        result.push((uid, (flags_hash, flags)));
    }

    Ok(result)
}

fn extract_references(message: &Message<'_>) -> Option<Vec<String>> {
    match message.references() {
        mail_parser::HeaderValue::Text(cow) => Some(vec![cow.to_string()]),
        mail_parser::HeaderValue::TextList(vec) => {
            Some(vec.iter().map(|cow| cow.to_string()).collect())
        }
        _ => None,
    }
}
