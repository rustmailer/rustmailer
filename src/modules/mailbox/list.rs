// Copyright © 2025-2026 rustmailer.com
// Licensed under RustMailer License Agreement v1.0
// Unauthorized copying, modification, or distribution is prohibited.

use std::sync::Arc;

use crate::modules::account::entity::MailerType;
use crate::modules::account::migration::AccountModel;
use crate::modules::cache::imap::mailbox::{Attribute, AttributeEnum, MailBox};
use crate::modules::cache::vendor::gmail::model::labels::LabelDetail;
use crate::modules::cache::vendor::gmail::sync::client::GmailClient;
use crate::modules::cache::vendor::gmail::sync::labels::GmailLabels;
use crate::modules::cache::vendor::outlook::sync::client::OutlookClient;
use crate::modules::cache::vendor::outlook::sync::folders::OutlookFolder;
use crate::modules::context::executors::RUST_MAIL_CONTEXT;
use crate::modules::error::code::ErrorCode;
use crate::modules::error::{RustMailerError, RustMailerResult};
use crate::modules::utils::mailbox_id;
use crate::raise_error;
use async_imap::types::Name;

pub async fn get_account_mailboxes(
    account_id: u64,
    remote: bool,
) -> RustMailerResult<Vec<MailBox>> {
    let account = AccountModel::get(account_id).await?;
    let remote = remote || account.minimal_sync();

    if remote && !account.enabled {
        return Err(raise_error!(
            format!("Account id='{account_id}' is disabled"),
            ErrorCode::AccountDisabled
        ));
    }

    match (&account.mailer_type, remote) {
        (MailerType::ImapSmtp, true) => request_imap_all_mailbox_list(account_id).await,
        (MailerType::ImapSmtp, false) => MailBox::list_all(account_id).await,
        (MailerType::GmailApi, true) => request_gmail_label_list(&account).await,
        (MailerType::GmailApi, false) => {
            let labels = GmailLabels::list_all(account_id).await?;
            Ok(labels.into_iter().map(Into::into).collect())
        }
        (MailerType::GraphApi, true) => {
            let folders = OutlookClient::list_mailfolders(account_id, account.use_proxy).await?;
            let mailboxes = folders
                .into_iter()
                .map(|f| {
                    let mut folder: OutlookFolder = f.try_into()?;
                    folder.account_id = account_id;
                    folder.id = mailbox_id(account_id, &folder.folder_id);
                    Ok(MailBox::from(folder))
                })
                .collect::<RustMailerResult<Vec<MailBox>>>()?;
            Ok(mailboxes)
        }
        (MailerType::GraphApi, false) => {
            let folders = OutlookFolder::list_all(account_id).await?;
            Ok(folders.into_iter().map(Into::into).collect())
        }
    }
}

pub async fn list_subscribed_mailboxes(account_id: u64) -> RustMailerResult<Vec<MailBox>> {
    AccountModel::check_account_active(account_id, true).await?;
    request_imap_subscribed_mailbox_list(account_id).await
}

pub async fn request_imap_subscribed_mailbox_list(
    account_id: u64,
) -> RustMailerResult<Vec<MailBox>> {
    let executor = RUST_MAIL_CONTEXT.imap(account_id).await?;
    let names = executor.list_all_subscribed_mailboxes().await?;
    convert_names_to_mailboxes(account_id, names.iter()).await
}

pub async fn request_imap_all_mailbox_list(account_id: u64) -> RustMailerResult<Vec<MailBox>> {
    let executor = RUST_MAIL_CONTEXT.imap(account_id).await?;
    let names = executor.list_all_mailboxes().await?;
    convert_names_to_mailboxes(account_id, names.iter()).await
}

pub async fn request_gmail_label_list(account: &AccountModel) -> RustMailerResult<Vec<MailBox>> {
    let all_labels = GmailClient::list_labels(account.id, account.use_proxy).await?;

    let mut tasks = Vec::new();

    let account = Arc::new(account.clone());
    for label in all_labels.into_iter() {
        let label_id = label.id.clone();
        let account = account.clone();
        let task: tokio::task::JoinHandle<Result<LabelDetail, RustMailerError>> =
            tokio::spawn(async move {
                GmailClient::get_label(account.id, account.use_proxy, label_id.as_str()).await
            });
        tasks.push(task);
    }

    let mut details = Vec::new();

    for task in tasks {
        match task.await {
            Ok(Ok(detail)) => details.push(detail),
            Ok(Err(err)) => return Err(err),
            Err(e) => return Err(raise_error!(format!("{:#?}", e), ErrorCode::InternalError)),
        }
    }

    let mailboxes: Vec<MailBox> = details
        .into_iter()
        .map(|label| {
            let mut label: GmailLabels = label.into();
            label.account_id = account.id;
            label.id = mailbox_id(account.id, &label.label_id);
            label.into()
        })
        .collect();

    Ok(mailboxes)
}

fn contains_no_select(attributes: &[Attribute]) -> bool {
    attributes
        .iter()
        .any(|attr| attr.attr == AttributeEnum::NoSelect)
}

pub async fn convert_names_to_mailboxes(
    account_id: u64,
    names: impl IntoIterator<Item = &Name>,
) -> RustMailerResult<Vec<MailBox>> {
    // Preallocate enough space in the vector to avoid multiple reallocations
    let mut tasks = Vec::new();

    for name in names.into_iter() {
        // Convert the name into a MailBox structure
        let mailbox_name = name.name().to_string();
        let mut mailbox: MailBox = name.into();

        if contains_no_select(&mailbox.attributes) {
            continue;
        }
        mailbox.account_id = account_id;
        mailbox.id = mailbox_id(account_id, &mailbox.name);
        let task: tokio::task::JoinHandle<Result<MailBox, RustMailerError>> =
            tokio::spawn(async move {
                let executor = RUST_MAIL_CONTEXT.imap(account_id).await?;
                let mx = executor.examine_mailbox(mailbox_name.as_str()).await?;
                // Update the mailbox status information
                mailbox.exists = mx.exists; // Number of messages in the mailbox
                mailbox.unseen = mx.unseen; // Number of unseen messages
                mailbox.uid_next = mx.uid_next; // Next unique identifier to be assigned
                mailbox.uid_validity = mx.uid_validity; // Validity of the UIDs
                mailbox.highest_modseq = mx.highest_modseq; // Highest modification sequence number
                                                            // Collect flags and permanent flags using map
                mailbox.flags = mx.flags.into_iter().map(Into::into).collect(); // Convert flags to MailBox format
                mailbox.permanent_flags = mx.permanent_flags.into_iter().map(Into::into).collect(); // Convert permanent flags
                Ok(mailbox)
            });
        tasks.push(task);
    }

    let mut mailboxes = Vec::new();

    for task in tasks {
        match task.await {
            Ok(Ok(mailbox)) => mailboxes.push(mailbox),
            Ok(Err(err)) => return Err(err), // Handle mailbox-level errors
            Err(e) => return Err(raise_error!(format!("{:#?}", e), ErrorCode::InternalError)), // Handle task-level panics or errors
        }
    }

    Ok(mailboxes)
}
