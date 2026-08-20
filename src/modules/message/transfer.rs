// Copyright © 2025-2026 rustmailer.com
// Licensed under RustMailer License Agreement v1.0
// Unauthorized copying, modification, or distribution is prohibited.

use crate::{
    encode_mailbox_name,
    modules::{
        account::{entity::MailerType, migration::AccountModel},
        cache::vendor::{gmail::sync::client::GmailClient, outlook::sync::client::OutlookClient},
        context::executors::RUST_MAIL_CONTEXT,
        envelope::generate_uid_set,
        error::{code::ErrorCode, RustMailerResult},
    },
    raise_error,
};
use poem_openapi::Object;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize, Object)]
pub struct MailboxTransferRequest {
    /// A list of unique message identifiers as strings.
    ///
    /// - For IMAP accounts, each UID is converted to a numeric string (parseable back to `u32`).
    /// - For Gmail API accounts, each element is a message ID (`mid`) returned by the API.
    /// Unifying them as strings simplifies handling across different backends.
    pub ids: Vec<String>,
    /// The name of the mailbox from which the messages will be moved.
    /// For IMAP: the decoded, human-readable name of the mailbox (e.g., "INBOX").
    /// For Gmail API: represents the label name.
    pub current_mailbox: Option<String>,
    /// The name of the mailbox to which the messages will be moved.
    /// For IMAP: the decoded, human-readable name of the mailbox (e.g., "INBOX").
    /// For Gmail API: represents the label name.
    pub target_mailbox: String,
}

#[derive(Clone, Default, Debug)]
pub enum MessageTransfer {
    #[default]
    Move,
    Copy,
}

pub async fn transfer_messages(
    account_id: u64,
    payload: &MailboxTransferRequest,
    transfer: MessageTransfer,
) -> RustMailerResult<()> {
    // Ensure the account exists before proceeding
    let account = AccountModel::get(account_id).await?;
    if !account.enabled {
        return Err(raise_error!(
            format!("Account id='{account_id}' is disabled"),
            ErrorCode::AccountDisabled
        ));
    }
    
    match account.mailer_type {
        MailerType::ImapSmtp => {
            if payload.ids.is_empty() {
                return Err(raise_error!(
                    "`ids` must contain at least one element".into(),
                    ErrorCode::InvalidParameter
                ));
            }

            let uids: Vec<u32> = payload
                .ids
                .iter()
                .map(|id| {
                    id.parse::<u32>().map_err(|_| {
                        raise_error!(
                            format!("Invalid IMAP UID: '{}', must be a numeric string", id),
                            ErrorCode::InvalidParameter
                        )
                    })
                })
                .collect::<Result<_, _>>()?;

            let uid_set = generate_uid_set(uids);
            let executor = RUST_MAIL_CONTEXT.imap(account_id).await?;
            // Encode the mailbox names using UTF-7 encoding
            if payload.current_mailbox.is_none() {
                return Err(
                    raise_error!("IMAP/SMTP accounts must provide a current_mailbox. Please ensure that the account type is properly configured.".into(),
                        ErrorCode::InvalidParameter
                    )
                );
            }

            let current_mailbox = encode_mailbox_name!(payload.current_mailbox.as_deref().unwrap());
            let target_mailbox = encode_mailbox_name!(payload.target_mailbox.as_str());

            match transfer {
                MessageTransfer::Move => {
                    // Move the messages from the current mailbox to the target mailbox
                    executor
                        .uid_move_envelopes(
                            uid_set.as_str(),
                            current_mailbox.as_str(),
                            target_mailbox.as_str(),
                        )
                        .await
                }
                MessageTransfer::Copy => {
                    // Copy the messages from the current mailbox to the target mailbox
                    executor
                        .uid_copy_envelopes(
                            uid_set.as_str(),
                            current_mailbox.as_str(),
                            target_mailbox.as_str(),
                        )
                        .await
                }
            }
        }
        MailerType::GmailApi => {
            let mids = &payload.ids;

            if payload.current_mailbox.is_none() {
                return Err(raise_error!(
                        "When moving emails with a Gmail API account, the current_mailbox parameter must be provided.".into(),
                        ErrorCode::InvalidParameter
                    )
                );
            }

            if mids.is_empty() {
                return Err(raise_error!(
                    "Gmail API copy requires at least one message ID".into(),
                    ErrorCode::InvalidParameter
                ));
            }

            if mids.len() > 500 {
                return Err(raise_error!(
                    format!(
                        "Gmail API batchModify supports at most 500 message IDs, got {}",
                        mids.len()
                    ),
                    ErrorCode::InvalidParameter
                ));
            }

            let labels_map =
                GmailClient::for_lookup_label_id(account_id, account.use_proxy, true).await?;
            match transfer {
                MessageTransfer::Move => {
                    let target_label_id =
                        labels_map.get(&payload.target_mailbox).ok_or_else(|| {
                            raise_error!(
                                format!(
                                    "Target mailbox/label `{}` not found in Gmail labels",
                                    payload.target_mailbox
                                ),
                                ErrorCode::InvalidParameter
                            )
                        })?;

                    let current_label_id = labels_map
                        .get(payload.current_mailbox.as_deref().unwrap())
                        .ok_or_else(|| {
                            raise_error!(
                                format!(
                                    "Current mailbox/label `{}` not found in Gmail labels",
                                    payload.current_mailbox.as_deref().unwrap()
                                ),
                                ErrorCode::InvalidParameter
                            )
                        })?;
                    //modify label
                    GmailClient::batch_modify(
                        account_id,
                        account.use_proxy,
                        mids,
                        vec![target_label_id.into()],
                        vec![current_label_id.into()],
                    )
                    .await
                }
                MessageTransfer::Copy => {
                    let target_label_id =
                        labels_map.get(&payload.target_mailbox).ok_or_else(|| {
                            raise_error!(
                                format!(
                                    "Target mailbox/label `{}` not found in Gmail labels",
                                    payload.target_mailbox
                                ),
                                ErrorCode::InvalidParameter
                            )
                        })?;
                    //add label
                    GmailClient::batch_modify(
                        account_id,
                        account.use_proxy,
                        mids,
                        vec![target_label_id.into()],
                        vec![],
                    )
                    .await
                }
            }
        }
        MailerType::GraphApi => {
            let mids = &payload.ids;
            if mids.is_empty() {
                return Err(raise_error!(
                    "Graph API copy requires at least one message ID".into(),
                    ErrorCode::InvalidParameter
                ));
            }

            if mids.len() > 500 {
                return Err(raise_error!(
                    format!(
                        "Graph API batchModify supports at most 500 message IDs, got {}",
                        mids.len()
                    ),
                    ErrorCode::InvalidParameter
                ));
            }

            let target_folder_id = OutlookClient::list_mailfolders(account_id, account.use_proxy)
                .await?
                .into_iter()
                .find(|f| f.display_name == payload.target_mailbox)
                .map(|f| f.id)
                .ok_or_else(|| {
                    raise_error!(
                        format!(
                            "Target mailbox '{}' not found. Please ensure this folder exists in the Outlook account.",
                            payload.target_mailbox
                        ),
                        ErrorCode::InvalidParameter
                    )
                })?;

            match transfer {
                MessageTransfer::Move => {
                    for mid in mids {
                        OutlookClient::move_message(
                            account_id,
                            account.use_proxy,
                            mid,
                            target_folder_id.as_str(),
                        )
                        .await?;
                    }
                }
                MessageTransfer::Copy => {
                    for mid in mids {
                        OutlookClient::copy_message(
                            account_id,
                            account.use_proxy,
                            mid,
                            target_folder_id.as_str(),
                        )
                        .await?;
                    }
                }
            }
            Ok(())
        }
    }
}
