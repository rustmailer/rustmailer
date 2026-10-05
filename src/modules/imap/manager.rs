// Copyright © 2025-2026 rustmailer.com
// Licensed under RustMailer License Agreement v1.0
// Unauthorized copying, modification, or distribution is prohibited.

use crate::modules::account::dispatcher::STATUS_DISPATCHER;
use crate::modules::account::entity::AuthType;
use crate::modules::account::migration::AccountModel;
use crate::modules::error::code::ErrorCode;
use crate::modules::error::RustMailerResult;
use crate::modules::imap::capabilities::{
    capability_to_string, check_capabilities, fetch_capabilities,
};
use crate::modules::imap::client::Client;
use crate::modules::imap::oauth2::OAuth2;
use crate::modules::imap::session::SessionStream;
use crate::modules::oauth2::token::OAuth2AccessToken;
use crate::{decrypt, raise_error, rustmailer_version};
use async_imap::Session;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing::{error, warn};

/// Initial backoff applied after a failed IMAP connection attempt (dial, login
/// or capability negotiation). Doubles on consecutive failures up to
/// [`CONNECT_BACKOFF_MAX`] and resets to zero after a successful connection.
const CONNECT_BACKOFF_INITIAL: Duration = Duration::from_secs(30);
/// Upper bound of the connection backoff sequence: 30s → 60s → … → 10min.
const CONNECT_BACKOFF_MAX: Duration = Duration::from_secs(600);

#[derive(Debug)]
pub struct ImapConnectionManager {
    pub account_id: u64,
    /// Current connection backoff duration in milliseconds; 0 = no backoff.
    backoff_ms: AtomicU64,
    /// Unix timestamp (ms) before which no new connection attempt may start.
    next_attempt_ms: AtomicU64,
}

fn unix_now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

impl ImapConnectionManager {
    pub fn new(account_id: u64) -> Self {
        Self {
            account_id,
            backoff_ms: AtomicU64::new(0),
            next_attempt_ms: AtomicU64::new(0),
        }
    }

    /// Records a failed connection attempt and extends the backoff window.
    /// This keeps bb8's dial retries and `min_idle` replenishment from
    /// hammering an unreachable or throttling server — every login attempt
    /// feeds a server-side throttle (INCIDENT-6717207556896619).
    pub(crate) fn note_connection_failure(&self) {
        let current = self.backoff_ms.load(Ordering::Relaxed);
        let next = if current == 0 {
            CONNECT_BACKOFF_INITIAL.as_millis() as u64
        } else {
            (current * 2).min(CONNECT_BACKOFF_MAX.as_millis() as u64)
        };
        self.backoff_ms.store(next, Ordering::Relaxed);
        self.next_attempt_ms
            .store(unix_now_ms().saturating_add(next), Ordering::Relaxed);
        warn!(
            account_id = self.account_id,
            backoff_secs = next / 1000,
            "IMAP connection attempt failed, backing off before next attempt"
        );
    }

    /// Records a successful connection and clears the backoff window.
    pub(crate) fn note_connection_success(&self) {
        self.backoff_ms.store(0, Ordering::Relaxed);
        self.next_attempt_ms.store(0, Ordering::Relaxed);
    }

    /// Sleeps until the backoff window (if any) has elapsed. Concurrent
    /// callers all park until the window opens, pacing connection storms.
    pub(crate) async fn wait_for_backoff(&self) {
        loop {
            let now = unix_now_ms();
            let until = self.next_attempt_ms.load(Ordering::Relaxed);
            if now >= until {
                return;
            }
            tokio::time::sleep(Duration::from_millis(until - now)).await;
        }
    }

    pub async fn fetch_account(&self) -> RustMailerResult<AccountModel> {
        // Fetch the account entity in non-test environment
        AccountModel::get(self.account_id).await
    }

    async fn create_client(&self, account: &AccountModel) -> RustMailerResult<Client> {
        let imap = account
            .imap
            .clone()
            .expect("BUG: account.imap is None, but it should always be present");
        Client::connection(imap.host, imap.encryption, imap.port, imap.use_proxy).await
    }

    async fn authenticate(
        &self,
        client: Client,
        account: &AccountModel,
    ) -> RustMailerResult<Session<Box<dyn SessionStream>>> {
        let imap = account
            .imap
            .clone()
            .expect("BUG: account.imap is None, but it should always be present");

        match &imap.auth.auth_type {
            AuthType::Password => {
                let password = imap.auth.password.clone().ok_or_else(|| {
                    raise_error!(
                        "Imap auth type is Passwd, but password not set".into(),
                        ErrorCode::MissingConfiguration
                    )
                })?;

                let password = decrypt!(&password)?;
                client.login(&account.email, &password).await
            }
            AuthType::OAuth2 => {
                let record = OAuth2AccessToken::get(self.account_id).await?;
                let access_token = record.and_then(|r| r.access_token).ok_or_else(|| {
                    raise_error!(
                        "Imap auth type is OAuth2, but OAuth2 authorization is not yet complete."
                            .into(),
                        ErrorCode::MissingConfiguration
                    )
                })?;
                client
                    .authenticate(OAuth2::new(account.email.clone(), access_token))
                    .await
            }
        }
    }

    pub async fn build(&self) -> RustMailerResult<Session<Box<dyn SessionStream>>> {
        let account = self.fetch_account().await?;

        let client = match self.create_client(&account).await {
            Ok(client) => client,
            Err(error) => {
                error!(
                    "Failed to create IMAP {}'s client: {:#?}",
                    &account.email, error
                );
                STATUS_DISPATCHER
                    .append_error(
                        self.account_id,
                        format!("imap client connect error: {:#?}", error),
                    )
                    .await;
                return Err(error);
            }
        };

        let mut session = match self.authenticate(client, &account).await {
            Ok(session) => session,
            Err(error) => {
                error!("Failed to authenticate IMAP session: {:#?}", error);

                STATUS_DISPATCHER
                    .append_error(
                        self.account_id,
                        format!("imap client authenticate error: {:#?}", error),
                    )
                    .await;
                return Err(error);
            }
        };

        match fetch_capabilities(&mut session).await {
            Ok(capabilities) => {
                let to_save: Vec<String> = capabilities.iter().map(capability_to_string).collect();
                AccountModel::update_capabilities(self.account_id, to_save).await?;
                if let Err(error) = check_capabilities(&capabilities) {
                    error!("Failed to check IMAP capabilities: {:#?}", error);
                    STATUS_DISPATCHER
                        .append_error(
                            self.account_id,
                            format!("imap client check capabilities error: {:#?}", error),
                        )
                        .await;
                    return Err(error);
                }
                if capabilities.has_str("ID") || capabilities.has_str("id") {
                    session
                        .id([
                            ("name", Some("rustmailer")),
                            ("version", Some(rustmailer_version!())),
                            ("vendor", Some("rustmailer")),
                        ])
                        .await
                        .map_err(|e| {
                            raise_error!(format!("{:#?}", e), ErrorCode::ImapCommandFailed)
                        })?;
                }
            }
            Err(error) => {
                error!("Failed to fetch IMAP capabilities: {:#?}", error);
                STATUS_DISPATCHER
                    .append_error(
                        self.account_id,
                        format!("imap client fetch capabilities error: {:#?}", error),
                    )
                    .await;
                return Err(error);
            }
        }

        Ok(session)
    }
}
