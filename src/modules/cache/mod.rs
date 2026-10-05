// Copyright © 2025-2026 rustmailer.com
// Licensed under RustMailer License Agreement v1.0
// Unauthorized copying, modification, or distribution is prohibited.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock};

use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore};

use crate::modules::error::code::ErrorCode;
use crate::modules::error::RustMailerResult;
use crate::modules::imap::pool::MAX_POOL_SIZE;
use crate::modules::settings::cli::SETTINGS;
use crate::raise_error;

pub mod disk;
pub mod imap;
pub mod model;
pub mod sync_type;
pub mod vendor;

pub static SEMAPHORE: LazyLock<Arc<Semaphore>> = LazyLock::new(|| {
    Arc::new(Semaphore::new(
        SETTINGS
            .rustmailer_sync_concurrency
            .map(|c| c as usize)
            .unwrap_or(num_cpus::get() * 2),
    ))
});

/// Per-account sync task registry. Entries are never removed: the number of
/// accounts is license-bounded, so the map stays small even if accounts are
/// deleted and re-created.
static ACCOUNT_SYNC_SEMAPHORES: LazyLock<Mutex<HashMap<u64, Arc<Semaphore>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Acquires a per-account sync slot, bounding each account to at most
/// `MAX_POOL_SIZE` concurrent IMAP sync tasks. Without this bound, a single
/// large mailbox sync can queue more `pool.get()` calls than the account's
/// pool has connections, and the queued tasks hit the 30s pool acquire
/// timeout (INCIDENT-6717207556896619). Combine with the global
/// [`SEMAPHORE`]: acquire the global permit first, then this one.
pub async fn acquire_account_sync_permit(
    account_id: u64,
) -> RustMailerResult<OwnedSemaphorePermit> {
    let semaphore = {
        let mut registry = ACCOUNT_SYNC_SEMAPHORES.lock().await;
        registry
            .entry(account_id)
            .or_insert_with(|| Arc::new(Semaphore::new(MAX_POOL_SIZE as usize)))
            .clone()
    };
    semaphore
        .acquire_owned()
        .await
        .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))
}
