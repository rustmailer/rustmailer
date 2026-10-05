// Copyright © 2025-2026 rustmailer.com
// Licensed under RustMailer License Agreement v1.0
// Unauthorized copying, modification, or distribution is prohibited.

use crate::modules::error::code::ErrorCode;
use crate::modules::error::{RustMailerError, RustMailerResult};
use crate::modules::imap::{manager::ImapConnectionManager, session::SessionStream};
use crate::raise_error;
use async_imap::Session;
use bb8::Pool;
use std::time::Duration;
use tracing::{error, warn};

/// Maximum concurrent IMAP connections per account. Also used to bound
/// per-account sync concurrency in `cache::acquire_account_sync_permit` so
/// sync tasks never queue more `pool.get()` calls than the pool has slots.
pub(crate) const MAX_POOL_SIZE: u32 = 8;

/// Timeout for the NOOP health probe run when checking a connection out of the
/// pool. Must stay comfortably above the round-trip latency of a trivial
/// command on a throttling server: Gmail under `[THROTTLED]` was observed
/// answering NOOP in 5.7–6.4s, which the previous 5s probe classified as a
/// dead connection — the pool then discarded every healthy connection it
/// touched and re-dialed in a self-sustaining storm
/// (INCIDENT-6717207556896619).
const CONNECTION_PROBE_TIMEOUT: Duration = Duration::from_secs(15);

impl bb8::ManageConnection for ImapConnectionManager {
    type Connection = MyImapConnection;

    type Error = RustMailerError;

    async fn connect(&self) -> RustMailerResult<Self::Connection> {
        // Pace repeated dial/login attempts with exponential backoff; without
        // this, bb8 retries and min_idle replenishment keep logging in for the
        // whole 30s acquire window, feeding the server-side throttle.
        self.wait_for_backoff().await;
        let session = match self.build().await {
            Ok(session) => {
                self.note_connection_success();
                session
            }
            Err(e) => {
                self.note_connection_failure();
                return Err(e);
            }
        };
        Ok(MyImapConnection {
            session,
            is_bad: false,
        })
    }
    // call this function before using the connection
    async fn is_valid(&self, conn: &mut Self::Connection) -> RustMailerResult<()> {
        if conn.is_bad {
            return Err(raise_error!(
                format!("Connection marked broken"),
                ErrorCode::ImapCommandFailed
            ));
        }
        match tokio::time::timeout(
            CONNECTION_PROBE_TIMEOUT,
            conn.run_command_and_check_ok("NOOP"),
        )
        .await
        {
            Ok(Ok(_)) => Ok(()),
            Ok(Err(e)) => {
                error!("IMAP connection validation failed: {:?}", e);
                conn.is_bad = true;
                Err(raise_error!(
                    format!("{:#?}", e),
                    ErrorCode::ImapCommandFailed
                ))
            }
            Err(_) => {
                warn!(
                    "IMAP NOOP probe timed out after {}s",
                    CONNECTION_PROBE_TIMEOUT.as_secs()
                );
                conn.is_bad = true;
                Err(raise_error!(
                    "NOOP timeout".into(),
                    ErrorCode::ImapCommandFailed
                ))
            }
        }
    }

    fn has_broken(&self, conn: &mut Self::Connection) -> bool {
        conn.is_bad
    }
}

pub async fn build_imap_pool(account_id: u64) -> RustMailerResult<Pool<ImapConnectionManager>> {
    let manager = ImapConnectionManager::new(account_id);
    let pool = Pool::builder()
        .connection_timeout(Duration::from_secs(30))
        .idle_timeout(Duration::from_secs(600))
        .max_lifetime(Duration::from_secs(1800))
        .retry_connection(true)
        .max_size(MAX_POOL_SIZE)
        .min_idle(Some(2))
        .test_on_check_out(true)
        .build(manager)
        .await?;

    Ok(pool)
}

pub struct MyImapConnection {
    pub session: Session<Box<dyn SessionStream>>,
    pub is_bad: bool,
}

impl std::ops::Deref for MyImapConnection {
    type Target = Session<Box<dyn SessionStream>>;
    fn deref(&self) -> &Self::Target {
        &self.session
    }
}

impl std::ops::DerefMut for MyImapConnection {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.session
    }
}
