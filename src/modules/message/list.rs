// Copyright © 2025-2026 rustmailer.com
// Licensed under RustMailer License Agreement v1.0
// Unauthorized copying, modification, or distribution is prohibited.

use crate::{
    base64_encode_url_safe, encode_mailbox_name,
    modules::{
        account::{entity::MailerType, migration::AccountModel},
        cache::{
            imap::{mailbox::MailBox, migration::EmailEnvelopeV3, thread::EmailThread},
            model::Envelope,
            vendor::{
                gmail::{
                    model::thread::{get_thread_messages_impl, list_threads_impl},
                    sync::{client::GmailClient, envelope::GmailEnvelope, labels::GmailLabels},
                },
                outlook::sync::{
                    client::OutlookClient, envelope::OutlookEnvelope, folders::OutlookFolder,
                },
            },
        },
        common::{decode_page_token, parallel::run_with_limit},
        context::executors::RUST_MAIL_CONTEXT,
        envelope::extractor::extract_envelope,
        error::{code::ErrorCode, RustMailerResult},
        rest::response::{CursorDataPage, DataPage},
        utils::mailbox_id,
    },
    raise_error,
};
use async_imap::types::Fetch;

pub async fn list_messages_in_mailbox(
    account_id: u64,
    mailbox_name: &str,
    next_page_token: Option<&str>,
    page_size: u64,
    remote: bool,
    desc: bool,
) -> RustMailerResult<CursorDataPage<Envelope>> {
    let account = AccountModel::get(account_id).await?;
    if page_size == 0 {
        return Err(raise_error!(
            "page_size must be greater than 0.".into(),
            ErrorCode::InvalidParameter
        ));
    }
    if page_size > 100 {
        return Err(raise_error!(
            "The page_size exceeds the maximum allowed limit of 100.".into(),
            ErrorCode::InvalidParameter
        ));
    }
    let remote = remote || account.minimal_sync();

    if remote && !account.enabled {
        return Err(raise_error!(
            format!("Account id='{account_id}' is disabled"),
            ErrorCode::AccountDisabled
        ));
    }

    if remote {
        fetch_remote_messages(&account, mailbox_name, next_page_token, page_size, desc).await
    } else {
        fetch_local_messages(&account, mailbox_name, next_page_token, page_size, desc).await
    }
}

// fn validate_pagination_params(page: u64, page_size: u64) -> RustMailerResult<()> {
//     if page == 0 || page_size == 0 {
//         return Err(raise_error!(
//             "Both page and page_size must be greater than 0.".into(),
//             ErrorCode::InvalidParameter
//         ));
//     }
//     if page_size > 500 {
//         return Err(raise_error!(
//             "The page_size exceeds the maximum allowed limit of 500.".into(),
//             ErrorCode::InvalidParameter
//         ));
//     }
//     Ok(())
// }

async fn fetch_remote_messages(
    account: &AccountModel,
    mailbox_name: &str,
    next_page_token: Option<&str>,
    page_size: u64,
    desc: bool,
) -> RustMailerResult<CursorDataPage<Envelope>> {
    if page_size > 100 {
        return Err(raise_error!(
            "The page_size exceeds the maximum allowed limit of 100.".into(),
            ErrorCode::InvalidParameter
        ));
    }
    match account.mailer_type {
        MailerType::ImapSmtp => {
            let page = decode_page_token(next_page_token)?;
            let excutor = RUST_MAIL_CONTEXT.imap(account.id).await?;
            let (mut fetches, total_items) = excutor
                .retrieve_metadata_paginated(
                    page,
                    page_size,
                    encode_mailbox_name!(mailbox_name).as_str(),
                    desc,
                    false,
                )
                .await?;
            if total_items == 0 {
                return Ok(CursorDataPage::new(
                    None,
                    Some(page_size),
                    0,
                    Some(0),
                    vec![],
                ));
            }

            if desc {
                fetches.reverse();
            }

            let total_pages = (total_items as f64 / page_size as f64).ceil() as u64;
            let envelopes = process_fetches(fetches, account.id, mailbox_name).await?;

            let next_page_token = if page == total_pages {
                None
            } else {
                Some(base64_encode_url_safe!((page + 1).to_string()))
            };

            Ok(CursorDataPage::new(
                next_page_token,
                Some(page_size),
                total_items,
                Some(total_pages),
                envelopes,
            ))
        }
        MailerType::GmailApi => {
            let label_map =
                GmailClient::for_lookup_label_id(account.id, account.use_proxy, true).await?;
            let label_id = label_map.get(mailbox_name).ok_or_else(|| {
                raise_error!(
                    format!("Label not found for mailbox: {}", mailbox_name),
                    ErrorCode::ResourceNotFound
                )
            })?;
            let message_list = GmailClient::list_messages(
                account.id,
                account.use_proxy,
                Some(label_id),
                next_page_token,
                None,
                page_size as u32,
            )
            .await?;

            let total = message_list.result_size_estimate.ok_or_else(|| {
                raise_error!(
                    "Missing 'resultSizeEstimate' in Gmail API response".into(),
                    ErrorCode::InternalError
                )
            })?;

            let messages = message_list.messages;
            let messages = match messages {
                Some(ref msgs) if !msgs.is_empty() => msgs,
                _ => {
                    return Ok(CursorDataPage {
                        next_page_token: None,
                        page_size: Some(page_size),
                        total_items: 0,
                        items: vec![],
                        total_pages: Some(0),
                    })
                }
            };

            let account_id = account.id;
            let use_proxy = account.use_proxy;
            let next_page_token = message_list.next_page_token;
            let batch_messages =
                run_with_limit(5, messages.iter().cloned(), move |index| async move {
                    GmailClient::get_message(account_id, use_proxy, &index.id).await
                })
                .await?;
            let label_map = GmailClient::for_get_label_name(account.id, account.use_proxy).await?;
            let envelopes: Vec<Envelope> = batch_messages
                .into_iter()
                .map(|m| {
                    let mut envelope: GmailEnvelope = m.try_into()?;
                    envelope.account_id = account_id;
                    envelope.label_name = mailbox_name.into();
                    Ok(envelope.into_envelope(&label_map))
                })
                .collect::<RustMailerResult<Vec<Envelope>>>()?;

            let total_pages = (total as f64 / page_size as f64).ceil() as u64;

            Ok(CursorDataPage {
                next_page_token,
                page_size: Some(page_size),
                total_items: total,
                items: envelopes,
                total_pages: Some(total_pages),
            })
        }
        MailerType::GraphApi => {
            let folders = OutlookClient::list_mailfolders(account.id, account.use_proxy).await?;
            let folder = folders
                .into_iter()
                .find(|f| f.display_name == mailbox_name)
                .ok_or_else(|| {
                    raise_error!(
                        format!("Outlook folder '{}' not found.", mailbox_name),
                        ErrorCode::InvalidParameter
                    )
                })?;
            let total_items = folder.total_item_count.ok_or_else(|| {
                raise_error!(
                    format!(
                        "Outlook folder '{}' does not provide 'total_item_count'.",
                        mailbox_name
                    ),
                    ErrorCode::InvalidParameter
                )
            })?;

            if total_items == 0 {
                return Ok(CursorDataPage::new(
                    None,
                    Some(page_size),
                    0,
                    Some(0),
                    vec![],
                ));
            }

            let total_pages = (total_items as f64 / page_size as f64).ceil() as u64;
            let page = decode_page_token(next_page_token)?;
            let resp = OutlookClient::list_messages(
                account.id,
                account.use_proxy,
                &folder.id,
                page,
                page_size,
                None,
            )
            .await?;

            let envelopes: Vec<Envelope> = resp
                .value
                .into_iter()
                .map(|m| {
                    let mut envelope: OutlookEnvelope = m.try_into()?;
                    envelope.account_id = account.id;
                    envelope.folder_id = mailbox_id(account.id, &folder.id);
                    envelope.folder_name = mailbox_name.to_string();
                    let conversation_id = envelope.conversation_id.clone();
                    let mut e = Envelope::from(envelope);
                    e.thread_id = conversation_id.unwrap_or("unknown".into());
                    Ok(e)
                })
                .collect::<RustMailerResult<Vec<Envelope>>>()?;

            let next_page_token = if page == total_pages {
                None
            } else {
                Some(base64_encode_url_safe!((page + 1).to_string()))
            };

            Ok(CursorDataPage::new(
                next_page_token,
                Some(page_size),
                total_items as u64,
                Some(total_pages),
                envelopes,
            ))
        }
    }
}

async fn process_fetches(
    fetches: Vec<Fetch>,
    account_id: u64,
    mailbox_name: &str,
) -> RustMailerResult<Vec<Envelope>> {
    let mut envelopes = Vec::with_capacity(fetches.len());
    for fetch in fetches {
        let envelope = extract_envelope(&fetch, account_id, mailbox_name)?;
        envelopes.push(envelope.into());
    }
    Ok(envelopes)
}

async fn fetch_local_messages(
    account: &AccountModel,
    mailbox_name: &str,
    next_page_token: Option<&str>,
    page_size: u64,
    desc: bool,
) -> RustMailerResult<CursorDataPage<Envelope>> {
    let page = decode_page_token(next_page_token)?;
    match account.mailer_type {
        MailerType::ImapSmtp => {
            let mailbox = MailBox::get(account.id, mailbox_name).await.map_err(|_| {
                raise_error!(
                    "This mailbox might not be included in the synchronized mailbox list of the account. \
                     To fetch emails from the mailbox, please add the parameter 'remote=true' in the URL."
                        .into(),
                    ErrorCode::MailBoxNotCached
                )
            })?;
            let DataPage {
                current_page: _,
                page_size,
                total_items,
                items,
                total_pages,
            } = EmailEnvelopeV3::list_messages_in_mailbox(mailbox.id, page, page_size, desc)
                .await?;

            if total_items == 0 {
                Ok(CursorDataPage::new(None, page_size, 0, None, vec![]))
            } else {
                let total_pages = total_pages.ok_or_else(|| {
                    raise_error!(
                        "Internal error: total_pages is None (this should never happen)".into(),
                        ErrorCode::InternalError
                    )
                })?;

                let next_page_token = if page == total_pages {
                    None
                } else {
                    Some(base64_encode_url_safe!((page + 1).to_string()))
                };

                Ok(CursorDataPage::new(
                    next_page_token,
                    page_size,
                    total_items,
                    Some(total_pages),
                    items.into_iter().map(Envelope::from).collect(),
                ))
            }
        }
        MailerType::GmailApi => {
            let target_label = GmailLabels::get_by_name(account.id, mailbox_name).await?;
            let DataPage {
                current_page: _,
                page_size,
                total_items,
                items,
                total_pages,
            } = GmailEnvelope::list_messages_in_label(target_label.id, page, page_size, desc)
                .await?;
            let map = GmailClient::for_get_label_name(account.id, account.use_proxy).await?;

            if total_items == 0 {
                Ok(CursorDataPage::new(None, page_size, 0, None, vec![]))
            } else {
                let total_pages = total_pages.ok_or_else(|| {
                    raise_error!(
                        "Internal error: total_pages is None (this should never happen)".into(),
                        ErrorCode::InternalError
                    )
                })?;

                let next_page_token = if page == total_pages {
                    None
                } else {
                    Some(base64_encode_url_safe!((page + 1).to_string()))
                };

                Ok(CursorDataPage::new(
                    next_page_token,
                    page_size,
                    total_items,
                    Some(total_pages),
                    items.into_iter().map(|e| e.into_envelope(&map)).collect(),
                ))
            }
        }
        MailerType::GraphApi => {
            let target_label = OutlookFolder::get_by_name(account.id, mailbox_name).await?;

            let DataPage {
                current_page: _,
                page_size,
                total_items,
                items,
                total_pages,
            } = OutlookEnvelope::list_messages_in_folder(target_label.id, page, page_size, desc)
                .await?;

            if total_items == 0 {
                Ok(CursorDataPage::new(None, page_size, 0, None, vec![]))
            } else {
                let total_pages = total_pages.ok_or_else(|| {
                    raise_error!(
                        "Internal error: total_pages is None (this should never happen)".into(),
                        ErrorCode::InternalError
                    )
                })?;

                let next_page_token = if page == total_pages {
                    None
                } else {
                    Some(base64_encode_url_safe!((page + 1).to_string()))
                };

                Ok(CursorDataPage::new(
                    next_page_token,
                    page_size,
                    total_items,
                    Some(total_pages),
                    items.into_iter().map(|e| e.into()).collect(),
                ))
            }
        }
    }
}

pub async fn list_threads_in_mailbox(
    account_id: u64,
    mailbox_name: &str,
    next_page_token: Option<&str>,
    page_size: u64,
    remote: bool,
    desc: bool,
) -> RustMailerResult<CursorDataPage<Envelope>> {
    let account = AccountModel::get(account_id).await?;

    if remote && !account.enabled {
        return Err(raise_error!(
            format!("Account id='{account_id}' is disabled"),
            ErrorCode::AccountDisabled
        ));
    }

    if page_size == 0 {
        return Err(raise_error!(
            "page_size must be greater than 0.".into(),
            ErrorCode::InvalidParameter
        ));
    }
    if page_size > 100 {
        return Err(raise_error!(
            "The page_size exceeds the maximum allowed limit of 100.".into(),
            ErrorCode::InvalidParameter
        ));
    }

    let not_found_err = || {
        raise_error!(
            format!(
                "Mailbox '{}' not found in the synchronized mailbox list for account {}. \
                 This may happen if the mailbox was not selected during synchronization settings \
                 or has been removed from the email server.",
                mailbox_name, account_id
            ),
            ErrorCode::MailBoxNotCached
        )
    };

    match account.mailer_type {
        MailerType::ImapSmtp => {
            if account.minimal_sync() {
                return Err(raise_error!(
                    format!(
                        "Account {} is configured with minimal_sync. RustMailer does not sync the required \
                        metadata for IMAP accounts in this mode, so thread information cannot be obtained. \
                        IMAP servers do not provide native thread data, therefore you must recreate the \
                        account without minimal_sync to enable thread listing.",
                        account_id
                    ),
                    ErrorCode::Incompatible
                ));
            }

            if remote {
                return Err(raise_error!(
                    format!(
                        "IMAP accounts cannot use the 'remote' parameter because thread lists cannot be \
                        retrieved directly from an IMAP server. The IMAP protocol does not provide any \
                        native thread information, so remote thread listing is unsupported.",
                    ),
                    ErrorCode::Incompatible
                ));
            }

            let page = decode_page_token(next_page_token)?;
            let mailbox = MailBox::get(account.id, mailbox_name)
                .await
                .map_err(|_| not_found_err())?;
            let DataPage {
                current_page: _,
                page_size,
                total_items,
                items,
                total_pages,
            } = EmailThread::list_threads_in_mailbox(mailbox.id, page, page_size, desc).await?;

            if total_items == 0 {
                Ok(CursorDataPage::new(None, page_size, 0, None, vec![]))
            } else {
                let total_pages = total_pages.ok_or_else(|| {
                    raise_error!(
                        "Internal error: total_pages is None (this should never happen)".into(),
                        ErrorCode::InternalError
                    )
                })?;

                let next_page_token = if page == total_pages {
                    None
                } else {
                    Some(base64_encode_url_safe!((page + 1).to_string()))
                };

                Ok(CursorDataPage::new(
                    next_page_token,
                    page_size,
                    total_items,
                    Some(total_pages),
                    items,
                ))
            }
        }
        MailerType::GmailApi => {
            if account.minimal_sync() || remote {
                let label_map =
                    GmailClient::for_lookup_label_id(account.id, account.use_proxy, true).await?;
                let label_id = label_map.get(mailbox_name).ok_or_else(|| {
                    raise_error!(
                        format!("Label not found for mailbox: {}", mailbox_name),
                        ErrorCode::ResourceNotFound
                    )
                })?;

                list_threads_impl(
                    account_id,
                    account.use_proxy,
                    Some(label_id),
                    next_page_token,
                    None,
                    page_size,
                )
                .await
            } else {
                let page = decode_page_token(next_page_token)?;
                let label = GmailLabels::get_by_name(account_id, mailbox_name).await?;
                let DataPage {
                    current_page: _,
                    page_size,
                    total_items,
                    items,
                    total_pages,
                } = EmailThread::list_threads_in_label(account, label.id, page, page_size, desc)
                    .await?;

                if total_items == 0 {
                    Ok(CursorDataPage::new(None, page_size, 0, None, vec![]))
                } else {
                    let total_pages = total_pages.ok_or_else(|| {
                        raise_error!(
                            "Internal error: total_pages is None (this should never happen)".into(),
                            ErrorCode::InternalError
                        )
                    })?;

                    let next_page_token = if page == total_pages {
                        None
                    } else {
                        Some(base64_encode_url_safe!((page + 1).to_string()))
                    };

                    Ok(CursorDataPage::new(
                        next_page_token,
                        page_size,
                        total_items,
                        Some(total_pages),
                        items,
                    ))
                }
            }
        }
        MailerType::GraphApi => {
            let page = decode_page_token(next_page_token)?;
            if account.minimal_sync() || remote {
                let folders =
                    OutlookClient::list_mailfolders(account.id, account.use_proxy).await?;
                let folder = folders
                    .into_iter()
                    .find(|f| f.display_name == mailbox_name)
                    .ok_or_else(|| {
                        raise_error!(
                            format!("Outlook folder '{}' not found.", mailbox_name),
                            ErrorCode::InvalidParameter
                        )
                    })?;
                let total_items = folder.total_item_count.ok_or_else(|| {
                    raise_error!(
                        format!(
                            "Outlook folder '{}' does not provide 'total_item_count'.",
                            mailbox_name
                        ),
                        ErrorCode::InvalidParameter
                    )
                })?;

                if total_items == 0 {
                    return Ok(CursorDataPage::new(
                        None,
                        Some(page_size),
                        0,
                        Some(0),
                        vec![],
                    ));
                }

                let total_pages = (total_items as f64 / page_size as f64).ceil() as u64;
                let resp = OutlookClient::list_messages(
                    account.id,
                    account.use_proxy,
                    &folder.id,
                    page,
                    page_size,
                    None,
                )
                .await?;

                let envelopes: Vec<Envelope> = resp
                    .value
                    .into_iter()
                    .map(|m| {
                        let mut envelope: OutlookEnvelope = m.try_into()?;
                        envelope.account_id = account.id;
                        envelope.folder_id = mailbox_id(account.id, &folder.id);
                        envelope.folder_name = mailbox_name.to_string();
                        let conversation_id = envelope.conversation_id.clone();
                        let mut e = Envelope::from(envelope);
                        e.thread_id = conversation_id.unwrap_or("unknown".into());
                        Ok(e)
                    })
                    .collect::<RustMailerResult<Vec<Envelope>>>()?;

                let next_page_token = if page == total_pages {
                    None
                } else {
                    Some(base64_encode_url_safe!((page + 1).to_string()))
                };

                Ok(CursorDataPage::new(
                    next_page_token,
                    Some(page_size),
                    total_items as u64,
                    Some(total_pages),
                    envelopes,
                ))
            } else {
                let folder = OutlookFolder::get_by_name(account_id, mailbox_name).await?;
                let DataPage {
                    current_page: _,
                    page_size,
                    total_items,
                    items,
                    total_pages,
                } = EmailThread::list_threads_in_folder(folder.id, page, page_size, desc).await?;
                if total_items == 0 {
                    Ok(CursorDataPage::new(None, page_size, 0, None, vec![]))
                } else {
                    let total_pages = total_pages.ok_or_else(|| {
                        raise_error!(
                            "Internal error: total_pages is None (this should never happen)".into(),
                            ErrorCode::InternalError
                        )
                    })?;

                    let next_page_token = if page == total_pages {
                        None
                    } else {
                        Some(base64_encode_url_safe!((page + 1).to_string()))
                    };

                    Ok(CursorDataPage::new(
                        next_page_token,
                        page_size,
                        total_items,
                        Some(total_pages),
                        items,
                    ))
                }
            }
        }
    }
}

pub async fn get_thread_messages(
    account_id: u64,
    thread_id: String,
    remote: bool,
) -> RustMailerResult<Vec<Envelope>> {
    let account = AccountModel::get(account_id).await?;

    if remote && !account.enabled {
        return Err(raise_error!(
            format!("Account id='{account_id}' is disabled"),
            ErrorCode::AccountDisabled
        ));
    }

    match account.mailer_type {
        MailerType::ImapSmtp => {
            if account.minimal_sync() {
                return Err(raise_error!(
                    format!(
                        "Account {} is currently using minimal sync mode. IMAP accounts cannot provide thread \
                        information when minimal sync is enabled because IMAP servers do not supply thread IDs. \
                        Thread data must be computed locally, which requires full metadata caching. \
                        To use thread-related features, please delete and re-add the email account with minimal sync disabled.",
                        account_id
                    ),
                    ErrorCode::Incompatible
                ));
            }

            if remote {
                return Err(raise_error!(
                    format!(
                        "IMAP accounts cannot use the 'remote' parameter because thread messages cannot be \
                        retrieved directly from an IMAP server. The IMAP protocol does not provide any \
                        native thread information, so get remote thread messages is unsupported.",
                    ),
                    ErrorCode::Incompatible
                ));
            }

            let thread_id_num: u64 = thread_id.parse().map_err(|_| {
                raise_error!(
                    format!(
                        "Invalid thread_id: '{}'. IMAP thread_id must be a numeric string that can be parsed into a number.",
                        thread_id
                    ),
                    ErrorCode::InvalidParameter
                )
            })?;

            EmailEnvelopeV3::get_thread(account_id, thread_id_num).await
        }
        MailerType::GmailApi => {
            if account.minimal_sync() || remote {
                get_thread_messages_impl(account_id, account.use_proxy, &thread_id).await
            } else {
                let thread_id_num: u64 = thread_id.parse().map_err(|_| {
                    raise_error!(
                        format!(
                            "Invalid thread_id: '{}'. thread_id must be a numeric string that can be parsed into a number.",
                            thread_id
                        ),
                        ErrorCode::InvalidParameter
                    )
                })?;
                let envelopes = GmailEnvelope::get_thread(account_id, thread_id_num).await?;
                let map = GmailClient::for_get_label_name(account_id, account.use_proxy).await?;
                Ok(envelopes
                    .into_iter()
                    .map(|e| e.into_envelope(&map))
                    .collect())
            }
        }
        MailerType::GraphApi => {
            if account.minimal_sync() || remote {
                let resp =
                    OutlookClient::get_thread_messages(account_id, account.use_proxy, &thread_id)
                        .await?;
                let envelopes: Vec<Envelope> = resp
                    .value
                    .into_iter()
                    .map(|m| {
                        let mut envelope: OutlookEnvelope = m.try_into()?;
                        envelope.account_id = account.id;
                        envelope.folder_id = 0;
                        envelope.folder_name = "".to_string();
                        let conversation_id = envelope.conversation_id.clone();
                        let mut e = Envelope::from(envelope);
                        e.thread_id = conversation_id.unwrap_or("unknown".into());
                        Ok(e)
                    })
                    .collect::<RustMailerResult<Vec<Envelope>>>()?;
                Ok(envelopes)
            } else {
                let thread_id_num: u64 = thread_id.parse().map_err(|_| {
                    raise_error!(
                        format!(
                            "Invalid thread_id: '{}'. thread_id must be a numeric string that can be parsed into a number.",
                            thread_id
                        ),
                        ErrorCode::InvalidParameter
                    )
                })?;
                let envelopes = OutlookEnvelope::get_thread(account_id, thread_id_num).await?;
                Ok(envelopes.into_iter().map(Into::into).collect())
            }
        }
    }
}
