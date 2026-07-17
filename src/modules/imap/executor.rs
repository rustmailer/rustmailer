// Copyright © 2025-2026 rustmailer.com
// Licensed under RustMailer License Agreement v1.0
// Unauthorized copying, modification, or distribution is prohibited.

use crate::modules::cache::imap::mailbox::EnvelopeFlag;
use crate::modules::error::code::ErrorCode;
use crate::modules::{error::RustMailerResult, imap::manager::ImapConnectionManager};
use crate::{encode_mailbox_name, raise_error};
use async_imap::types::{Fetch, Mailbox, Name};
use bb8::Pool;
use futures::{StreamExt, TryStreamExt};
use mail_parser::MessageParser;
use std::collections::HashSet;
use tracing::{debug, error, info, warn};

/// The IMAP query to fetch email metadata including headers and body structure.
const RICH_METADATA_QUERY: &str = "(UID BODYSTRUCTURE RFC822.SIZE INTERNALDATE FLAGS BODY.PEEK[HEADER.FIELDS (BCC CC Date From In-Reply-To Sender Return-Path Message-ID Subject MIME-Version References Reply-To To Received)])";

const MINIMAL_METADATA_QUERY: &str = "(UID FLAGS)";

const UID_FLAGS: &str = "(UID FLAGS)";

const BODYSTRUCTURE: &str = "(UID BODYSTRUCTURE RFC822.SIZE)";
// const SUBSCRIBE_INFO_QUERY: &str =
//     "(UID BODY.PEEK[HEADER.FIELDS (List-Unsubscribe List-Subscribe List-ID)])";

const BODY_FETCH_COMMAND: &str = "(BODY.PEEK[])";

const HEADER_MESSAGE_ID_QUERY: &str = "(UID BODY.PEEK[HEADER.FIELDS (Message-ID)])";

pub struct ImapExecutor {
    pool: Pool<ImapConnectionManager>,
}

impl ImapExecutor {
    pub fn new(pool: Pool<ImapConnectionManager>) -> Self {
        Self { pool }
    }

    pub async fn list_all_mailboxes(&self) -> RustMailerResult<Vec<Name>> {
        let mut session = self.pool.get().await?;

        let result = async {
            let list = session.list(Some(""), Some("*")).await?;
            list.try_collect::<Vec<Name>>().await
        }
        .await
        .map_err(|e| {
            if is_connection_error(&e) {
                session.is_bad = true;
            }
            raise_error!(format!("{:#?}", e), ErrorCode::ImapCommandFailed)
        })?;

        if result.is_empty() {
            session.is_bad = true;
            warn!("IMAP list returned empty, marking connection as bad.");
        }

        Ok(result)
    }

    pub async fn list_all_subscribed_mailboxes(&self) -> RustMailerResult<Vec<Name>> {
        let mut session = self.pool.get().await?;

        let result = async {
            let list_stream = session.lsub(Some(""), Some("*")).await?;
            list_stream.try_collect::<Vec<Name>>().await
        }
        .await
        .map_err(|e| {
            if is_connection_error(&e) {
                session.is_bad = true;
            }
            raise_error!(format!("{:#?}", e), ErrorCode::ImapCommandFailed)
        })?;

        if result.is_empty() {
            if let Err(_) = session.noop().await {
                session.is_bad = true;
                warn!("LSUB returned empty and NOOP failed, marking connection as bad.");
            }
        }

        Ok(result)
    }

    pub async fn create_mailbox(&self, mailbox_name: &str) -> RustMailerResult<()> {
        let mut session = self.pool.get().await?;
        session.create(mailbox_name).await.map_err(|e| {
            if is_connection_error(&e) {
                session.is_bad = true;
            }
            raise_error!(format!("{:#?}", e), ErrorCode::ImapCommandFailed)
        })?;
        Ok(())
    }

    pub async fn examine_mailbox(&self, mailbox_name: &str) -> RustMailerResult<Mailbox> {
        let mut session = self.pool.get().await?;
        session.examine(mailbox_name).await.map_err(|e| {
            if is_connection_error(&e) {
                session.is_bad = true;
            }
            raise_error!(format!("{:#?}", e), ErrorCode::ImapCommandFailed)
        })
    }

    async fn do_expunge(
        session: &mut bb8::PooledConnection<'_, ImapConnectionManager>,
        mailbox_name: &str,
    ) -> Result<(), async_imap::error::Error> {
        session.select(mailbox_name).await?;
        let _ = session.expunge().await?;
        Ok(())
    }

    pub async fn expunge_mailbox(&self, mailbox_name: &str) -> RustMailerResult<()> {
        let mut session: bb8::PooledConnection<'_, ImapConnectionManager> = self.pool.get().await?;
        let result = Self::do_expunge(&mut session, mailbox_name).await;
        if let Err(e) = result {
            if is_connection_error(&e) {
                session.is_bad = true;
            }
            return Err(raise_error!(
                format!("{:#?}", e),
                ErrorCode::ImapCommandFailed
            ));
        }
        Ok(())
    }

    pub async fn delete_mailbox(&self, mailbox_name: &str) -> RustMailerResult<()> {
        let mut session = self.pool.get().await?;
        session.delete(mailbox_name).await.map_err(|e| {
            if is_connection_error(&e) {
                session.is_bad = true;
            }
            raise_error!(format!("{:#?}", e), ErrorCode::ImapCommandFailed)
        })?;
        Ok(())
    }

    pub async fn rename_mailbox(&self, from: &str, to: &str) -> RustMailerResult<()> {
        let mut session = self.pool.get().await?;
        session.rename(from, to).await.map_err(|e| {
            if is_connection_error(&e) {
                session.is_bad = true;
            }
            raise_error!(format!("{:#?}", e), ErrorCode::ImapCommandFailed)
        })?;
        Ok(())
    }

    pub async fn subscribe_mailbox(&self, mailbox_name: &str) -> RustMailerResult<()> {
        let mut session = self.pool.get().await?;
        session.subscribe(mailbox_name).await.map_err(|e| {
            if is_connection_error(&e) {
                session.is_bad = true;
            }
            raise_error!(format!("{:#?}", e), ErrorCode::ImapCommandFailed)
        })?;
        Ok(())
    }

    pub async fn unsubscribe_mailbox(&self, mailbox_name: &str) -> RustMailerResult<()> {
        let mut session = self.pool.get().await?;
        session.unsubscribe(mailbox_name).await.map_err(|e| {
            if is_connection_error(&e) {
                session.is_bad = true;
            }
            raise_error!(format!("{:#?}", e), ErrorCode::ImapCommandFailed)
        })?;
        Ok(())
    }

    pub async fn fetch_uid_list(
        &self,
        start_uid: u32,
        mailbox_name: &str,
        minimal: bool,
    ) -> RustMailerResult<Vec<Fetch>> {
        assert!(start_uid > 0, "start_uid must be greater than 0");

        let uid_set = format!("{}:*", start_uid);
        let mut session = self.pool.get().await?;

        let result: Result<Vec<Fetch>, _> = async {
            session.examine(mailbox_name).await?;

            let mut stream = session
                .uid_fetch(
                    uid_set.as_str(),
                    if minimal {
                        MINIMAL_METADATA_QUERY
                    } else {
                        "(UID)"
                    },
                )
                .await?;

            let mut results = Vec::new();

            while let Some(item) = stream.next().await {
                match item {
                    Ok(fetch) => results.push(fetch),
                    Err(e) => {
                        error!("Failed to parse FETCH response, skipping: {:?}", e);
                        debug!("Error details: {:#?}", e);
                        continue;
                    }
                }
            }
            Ok(results)
        }
        .await;

        match result {
            Ok(v) => Ok(v),
            Err(e) => {
                if is_connection_error(&e) {
                    session.is_bad = true;
                }
                Err(raise_error!(
                    format!("{:#?}", e),
                    ErrorCode::ImapCommandFailed
                ))
            }
        }
    }

    /// Get the UID of a message by its Message-ID in the specified mailbox.
    ///
    /// ⚠️ Note:
    /// This function is intended for use in the **Drafts mailbox**, where the number of
    /// messages is typically small. It performs a linear scan of all messages in the mailbox,
    /// so it should **not be used for large mailboxes** as it may be inefficient.
    pub async fn get_uid_by_message_id(
        &self,
        target_message_id: &str,
        mailbox_name: &str,
    ) -> RustMailerResult<u32> {
        let mut session = self.pool.get().await?;

        let result: Result<u32, async_imap::error::Error> = async {
            session.examine(mailbox_name).await?;

            let mut stream = session.fetch("1:*", HEADER_MESSAGE_ID_QUERY).await?;

            while let Some(fetch_res) = stream.next().await {
                let fetch = fetch_res?;

                let uid = fetch
                    .uid
                    .ok_or_else(|| async_imap::error::Error::Bad("Missing UID".into()))?;

                let header = fetch
                    .header()
                    .ok_or_else(|| async_imap::error::Error::Bad("Missing header".into()))?;

                let headers = MessageParser::default()
                    .parse_headers(&header)
                    .ok_or_else(|| async_imap::error::Error::Bad("Header parse failed".into()))?;

                if let Some(message_id) = headers.message_id() {
                    if message_id == target_message_id {
                        return Ok(uid);
                    }
                }
            }

            Err(async_imap::error::Error::Bad("Message not found".into()))
        }
        .await;

        match result {
            Ok(uid) => Ok(uid),
            Err(e) => {
                if is_connection_error(&e) {
                    session.is_bad = true;
                }

                Err(raise_error!(
                    format!("{:#?}", e),
                    ErrorCode::ImapCommandFailed
                ))
            }
        }
    }

    pub async fn retrieve_metadata_paginated(
        &self,
        page: u64,
        page_size: u64,
        mailbox_name: &str,
        desc: bool,
        minimal: bool,
    ) -> RustMailerResult<(Vec<Fetch>, u64)> {
        assert!(page > 0, "Page number must be greater than 0");
        assert!(page_size > 0, "Page size must be greater than 0");

        let mut session = self.pool.get().await?;

        let result: Result<(Vec<Fetch>, u64), async_imap::error::Error> = async {
            let total = session.examine(mailbox_name).await?.exists as u64;

            if total == 0 {
                return Ok((Vec::new(), 0));
            }

            let (start, end) = if desc {
                let end = total.saturating_sub((page - 1) * page_size);
                if end == 0 {
                    return Ok((Vec::new(), total));
                }
                let start = end.saturating_sub(page_size - 1).max(1);
                (start, end)
            } else {
                let start = (page - 1) * page_size + 1;
                if start > total {
                    return Ok((Vec::new(), total));
                }
                let end = (start + page_size - 1).min(total);
                (start, end)
            };

            let sequence_set = format!("{}:{}", start, end);

            info!(
                "Fetching mailbox '{}' messages: sequence {} (page {}, page_size {}, desc={})",
                mailbox_name, sequence_set, page, page_size, desc
            );

            let query = if minimal {
                MINIMAL_METADATA_QUERY
            } else {
                RICH_METADATA_QUERY
            };

            let mut stream = session.fetch(sequence_set.as_str(), query).await?;
            let mut results = Vec::new();

            while let Some(item) = stream.next().await {
                match item {
                    Ok(fetch) => results.push(fetch),
                    Err(e) => {
                        error!("Failed to parse FETCH response, skipping: {:?}", e);
                        debug!("Error details: {:#?}", e);
                        continue;
                    }
                }
            }

            Ok((results, total))
        }
        .await;

        match result {
            Ok(v) => Ok(v),
            Err(e) => {
                if is_connection_error(&e) {
                    session.is_bad = true;
                }

                Err(raise_error!(
                    format!("{:#?}", e),
                    ErrorCode::ImapCommandFailed
                ))
            }
        }
    }

    pub async fn retrieve_paginated_uid_and_flags(
        &self,
        page: u32,
        page_size: u32,
        mailbox_name: &str,
        desc: bool,
    ) -> RustMailerResult<Vec<Fetch>> {
        assert!(page > 0, "Page number must be greater than 0");
        assert!(page_size > 0, "Page size must be greater than 0");

        let mut session = self.pool.get().await?;

        let result: Result<Vec<Fetch>, async_imap::error::Error> = async {
            let total = session.examine(mailbox_name).await?.exists;

            if total == 0 {
                return Ok(Vec::new());
            }

            let (start, end) = if desc {
                let end = total.saturating_sub((page - 1) * page_size);
                if end == 0 {
                    return Ok(Vec::new());
                }
                let start = end.saturating_sub(page_size - 1).max(1);
                (start, end)
            } else {
                let start = (page - 1) * page_size + 1;
                if start > total {
                    return Ok(Vec::new());
                }
                let end = (start + page_size - 1).min(total);
                (start, end)
            };

            if start > end {
                return Ok(Vec::new());
            }

            let sequence_set = format!("{}:{}", start, end);

            info!(
                "Fetching mailbox '{}' messages: sequence {} (page {}, page_size {}, desc={})",
                mailbox_name, sequence_set, page, page_size, desc
            );

            let mut stream = session.fetch(sequence_set.as_str(), UID_FLAGS).await?;

            let mut results = Vec::new();

            while let Some(item) = stream.next().await {
                match item {
                    Ok(fetch) => results.push(fetch),
                    Err(e) => {
                        error!("Failed to parse FETCH response, skipping: {:?}", e);
                        debug!("Error details: {:#?}", e);
                        continue;
                    }
                }
            }
            Ok(results)
        }
        .await;

        match result {
            Ok(v) => Ok(v),
            Err(e) => {
                if is_connection_error(&e) {
                    session.is_bad = true;
                }

                Err(raise_error!(
                    format!("{:#?}", e),
                    ErrorCode::ImapCommandFailed
                ))
            }
        }
    }

    pub async fn uid_fetch_uid_and_flags(
        &self,
        uid_set: &str,
        mailbox_name: &str,
    ) -> RustMailerResult<Vec<Fetch>> {
        debug!("Fetching UID batch: '{}'", uid_set);

        let mut session = self.pool.get().await?;

        let result: Result<Vec<Fetch>, async_imap::error::Error> = async {
            session.examine(mailbox_name).await?;

            let mut stream = session.uid_fetch(uid_set, UID_FLAGS).await?;
            let mut results = Vec::new();

            while let Some(item) = stream.next().await {
                match item {
                    Ok(fetch) => results.push(fetch),
                    Err(e) => {
                        error!("Failed to parse FETCH response, skipping: {:?}", e);
                        debug!("Error details: {:#?}", e);
                        continue;
                    }
                }
            }
            Ok(results)
        }
        .await;

        match result {
            Ok(v) => Ok(v),
            Err(e) => {
                if is_connection_error(&e) {
                    session.is_bad = true;
                }

                Err(raise_error!(
                    format!("{:#?}", e),
                    ErrorCode::ImapCommandFailed
                ))
            }
        }
    }

    pub async fn uid_fetch_body_structure(
        &self,
        uid_set: &str,
        mailbox_name: &str,
    ) -> RustMailerResult<Vec<Fetch>> {
        let mut session = self.pool.get().await?;

        let result: Result<Vec<Fetch>, async_imap::error::Error> = async {
            session.examine(mailbox_name).await?;

            let mut stream = session.uid_fetch(uid_set, BODYSTRUCTURE).await?;
            let mut results = Vec::new();

            while let Some(item) = stream.next().await {
                match item {
                    Ok(fetch) => {
                        results.push(fetch);
                    }
                    Err(e) => {
                        error!("Failed to parse FETCH response, skipping: {:?}", e);
                        debug!("Error details: {:#?}", e);
                        continue;
                    }
                }
            }

            Ok(results)
        }
        .await;

        match result {
            Ok(v) => Ok(v),
            Err(e) => {
                if is_connection_error(&e) {
                    session.is_bad = true;
                }

                Err(raise_error!(
                    format!("{:#?}", e),
                    ErrorCode::ImapCommandFailed
                ))
            }
        }
    }

    pub async fn uid_fetch_meta(
        &self,
        uid_set: &str,
        mailbox_name: &str,
        minimal: bool,
    ) -> RustMailerResult<Vec<Fetch>> {
        let mut session = self.pool.get().await?;

        let result: Result<Vec<Fetch>, async_imap::error::Error> = async {
            session.examine(mailbox_name).await?;

            let query = if minimal {
                MINIMAL_METADATA_QUERY
            } else {
                RICH_METADATA_QUERY
            };

            let mut stream = session.uid_fetch(uid_set, query).await?;
            let mut results = Vec::new();
            while let Some(item) = stream.next().await {
                match item {
                    Ok(fetch) => results.push(fetch),
                    Err(e) => {
                        error!("Failed to parse FETCH response, skipping: {:?}", e);
                        debug!("Error details: {:#?}", e);
                        continue;
                    }
                }
            }

            Ok(results)
        }
        .await;

        match result {
            Ok(v) => Ok(v),
            Err(e) => {
                if is_connection_error(&e) {
                    session.is_bad = true;
                }

                Err(raise_error!(
                    format!("{:#?}", e),
                    ErrorCode::ImapCommandFailed
                ))
            }
        }
    }

    pub async fn append(
        &self,
        mailbox_name: impl AsRef<str>,
        flags: Option<&str>,
        internaldate: Option<&str>,
        content: impl AsRef<[u8]>,
    ) -> RustMailerResult<()> {
        let mut session = self.pool.get().await?;
        session
            .append(mailbox_name, flags, internaldate, content)
            .await
            .map_err(|e| {
                if is_connection_error(&e) {
                    session.is_bad = true;
                }
                raise_error!(format!("{:#?}", e), ErrorCode::ImapCommandFailed)
            })
    }

    pub async fn uid_fetch_full_message(
        &self,
        uid: &str,
        mailbox_name: &str,
    ) -> RustMailerResult<Option<Fetch>> {
        let mut session = self.pool.get().await?;

        let result: Result<Option<Fetch>, async_imap::error::Error> = async {
            session.examine(mailbox_name).await?;

            let mut stream = session.uid_fetch(uid, BODY_FETCH_COMMAND).await?;
            let fetch = stream.try_next().await?;

            Ok(fetch)
        }
        .await;

        match result {
            Ok(v) => Ok(v),
            Err(e) => {
                if is_connection_error(&e) {
                    session.is_bad = true;
                }

                Err(raise_error!(
                    format!("{:#?}", e),
                    ErrorCode::ImapCommandFailed
                ))
            }
        }
    }

    pub async fn uid_fetch_single_part(
        &self,
        uid: &str,
        mailbox_name: &str,
        path: &str,
    ) -> RustMailerResult<Vec<Fetch>> {
        let mut session = self.pool.get().await?;

        let result: Result<Vec<Fetch>, async_imap::error::Error> = async {
            session.examine(mailbox_name).await?;

            let query = format!("(UID BODY.PEEK[{}])", path);
            let mut stream = session.uid_fetch(uid, &query).await?;
            let mut results = Vec::new();

            while let Some(item) = stream.next().await {
                match item {
                    Ok(fetch) => results.push(fetch),
                    Err(e) => {
                        error!("Failed to parse FETCH response, skipping: {:?}", e);
                        debug!("Error details: {:#?}", e);
                        continue;
                    }
                }
            }
            Ok(results)
        }
        .await;

        match result {
            Ok(v) => Ok(v),
            Err(e) => {
                if is_connection_error(&e) {
                    session.is_bad = true;
                }

                Err(raise_error!(
                    format!("{:#?}", e),
                    ErrorCode::ImapCommandFailed
                ))
            }
        }
    }

    pub async fn uid_move_envelopes(
        &self,
        uid_set: &str,
        from: &str,
        to: &str,
    ) -> RustMailerResult<()> {
        let mut session = self.pool.get().await?;

        let result: Result<(), async_imap::error::Error> = async {
            session.select(from).await?;
            session.uid_mv(uid_set, to).await?;
            Ok(())
        }
        .await;

        match result {
            Ok(v) => Ok(v),
            Err(e) => {
                if is_connection_error(&e) {
                    session.is_bad = true;
                }

                Err(raise_error!(
                    format!("{:#?}", e),
                    ErrorCode::ImapCommandFailed
                ))
            }
        }
    }

    pub async fn uid_copy_envelopes(
        &self,
        uid_set: &str,
        from: &str,
        to: &str,
    ) -> RustMailerResult<()> {
        let mut session = self.pool.get().await?;

        let result: Result<(), async_imap::error::Error> = async {
            session.select(from).await?;
            session.uid_copy(uid_set, to).await?;
            Ok(())
        }
        .await;

        match result {
            Ok(v) => Ok(v),
            Err(e) => {
                if is_connection_error(&e) {
                    session.is_bad = true;
                }

                Err(raise_error!(
                    format!("{:#?}", e),
                    ErrorCode::ImapCommandFailed
                ))
            }
        }
    }

    async fn uid_flag_store(
        &self,
        uid_set: &str,
        mailbox_name: &str,
        query: &str,
    ) -> RustMailerResult<Vec<Fetch>> {
        let mut session = self.pool.get().await?;

        let result: Result<Vec<Fetch>, async_imap::error::Error> = async {
            session.select(mailbox_name).await?;
            let mut stream = session.uid_store(uid_set, query).await?;
            let mut results = Vec::new();

            while let Some(item) = stream.next().await {
                match item {
                    Ok(fetch) => results.push(fetch),
                    Err(e) => {
                        error!("Failed to parse FETCH response, skipping: {:?}", e);
                        debug!("Error details: {:#?}", e);
                        continue;
                    }
                }
            }
            Ok(results)
        }
        .await;

        match result {
            Ok(v) => Ok(v),
            Err(e) => {
                if is_connection_error(&e) {
                    session.is_bad = true;
                }

                Err(raise_error!(
                    format!("{:#?}", e),
                    ErrorCode::ImapCommandFailed
                ))
            }
        }
    }

    pub async fn uid_set_flags(
        &self,
        uid_set: &str,
        mailbox_name: &str,
        add_flags: Option<Vec<EnvelopeFlag>>,
        remove_flags: Option<Vec<EnvelopeFlag>>,
        overwrite_flags: Option<Vec<EnvelopeFlag>>,
    ) -> RustMailerResult<Vec<Fetch>> {
        // Validate inputs
        if uid_set.is_empty() {
            return Err(raise_error!(
                "UID set cannot be empty".into(),
                ErrorCode::InternalError
            ));
        }
        if mailbox_name.is_empty() {
            return Err(raise_error!(
                "Mailbox name cannot be empty".into(),
                ErrorCode::InternalError
            ));
        }

        let mailbox_name = &encode_mailbox_name!(mailbox_name);

        let mut result = Vec::new();
        // Helper to convert flags to IMAP string
        let flags_to_string = |flags: &[EnvelopeFlag]| -> RustMailerResult<String> {
            if flags.is_empty() {
                return Err(raise_error!(
                    "Flag list cannot be empty".into(),
                    ErrorCode::InternalError
                ));
            }
            let flag_strings: Result<Vec<String>, _> =
                flags.iter().map(|f| f.to_imap_string()).collect();
            Ok(flag_strings?.join(" "))
        };

        // Process overwrite flags
        if let Some(ref flags) = overwrite_flags {
            let flags_str = flags_to_string(flags)?;
            let res = self
                .uid_flag_store(uid_set, mailbox_name, &format!("FLAGS ({})", flags_str))
                .await?;
            return Ok(res); // Early return for overwrite, as it’s exclusive
        }

        // Process add flags
        if let Some(ref flags) = add_flags {
            let flags_str = flags_to_string(flags)?;
            let res = self
                .uid_flag_store(uid_set, mailbox_name, &format!("+FLAGS ({})", flags_str))
                .await?;
            result.extend(res);
        }

        // Process remove flags
        if let Some(ref flags) = remove_flags {
            let flags_str = flags_to_string(flags)?;
            let res = self
                .uid_flag_store(uid_set, mailbox_name, &format!("-FLAGS ({})", flags_str))
                .await?;
            result.extend(res);
        }

        Ok(result)
    }

    pub async fn uid_delete_envelopes(
        &self,
        uid_set: &str,
        mailbox_name: &str,
    ) -> RustMailerResult<()> {
        self.uid_flag_store(uid_set, mailbox_name, "+FLAGS (\\Deleted)")
            .await?;
        self.expunge_mailbox(mailbox_name).await
    }

    pub async fn touch_connection(&self) -> RustMailerResult<()> {
        let state = self.pool.state();
        let idle_count = state.idle_connections;

        if idle_count == 0 {
            let _ = self.pool.get().await;
            return Ok(());
        }

        let touch_limit = std::cmp::min(idle_count as usize, 5);
        let mut active_conns = Vec::with_capacity(touch_limit);

        for _ in 0..touch_limit {
            if let Ok(Ok(conn)) =
                tokio::time::timeout(std::time::Duration::from_secs(10), self.pool.get()).await
            {
                active_conns.push(conn);
            }
        }

        debug!(
            idle_total = idle_count,
            touched = active_conns.len(),
            "IMAP connections refreshed."
        );
        Ok(())
    }

    pub async fn uid_search(
        &self,
        mailbox_name: &str,
        query: &str,
    ) -> RustMailerResult<HashSet<u32>> {
        let mut session = self.pool.get().await?;

        let result: Result<HashSet<u32>, async_imap::error::Error> = async {
            session.examine(mailbox_name).await?;
            let result = session.uid_search(query).await?;
            Ok(result)
        }
        .await;

        match result {
            Ok(v) => Ok(v),
            Err(e) => {
                if is_connection_error(&e) {
                    session.is_bad = true;
                }

                Err(raise_error!(
                    format!("{:#?}", e),
                    ErrorCode::ImapCommandFailed
                ))
            }
        }
    }
}

fn is_connection_error(e: &async_imap::error::Error) -> bool {
    match e {
        async_imap::error::Error::Io(_) => true,
        async_imap::error::Error::Bad(_) => false,
        async_imap::error::Error::No(_) => false,
        async_imap::error::Error::ConnectionLost => true,
        async_imap::error::Error::Parse(_) => true,
        async_imap::error::Error::Validate(_) => true,
        async_imap::error::Error::Append => false,
        _ => true,
    }
}
