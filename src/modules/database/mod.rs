// Copyright © 2025-2026 rustmailer.com
// Licensed under RustMailer License Agreement v1.0
// Unauthorized copying, modification, or distribution is prohibited.

use crate::modules::account::migration::{AccountV2, AccountV3};
use crate::modules::account::status::AccountRunningState;
use crate::modules::autoconfig::CachedMailSettings;
use crate::modules::cache::disk::CacheItem;
use crate::modules::error::RustMailerResult;
use crate::modules::hook::entity::EventHooks;
use crate::modules::license::License;
use crate::modules::oauth2::entity::{OAuth2, OAuth2V2};
use crate::modules::oauth2::pending::OAuth2PendingEntity;
use crate::modules::oauth2::token::OAuth2AccessToken;
use crate::modules::settings::proxy::Proxy;
use crate::modules::settings::system::SystemSetting;
use crate::modules::smtp::mta::entity::Mta;
use crate::modules::smtp::template::entity::EmailTemplate;
use crate::modules::token::AccessToken;
use crate::modules::{account::entity::Account, overview::metrics::DailyMetrics};
use crate::raise_error;
use db_type::{KeyOptions, ToKeyDefinition};
use itertools::Itertools;
use native_db::*;
use serde::Serialize;
use std::sync::{Arc, LazyLock};
use transaction::RwTransaction;

use super::error::code::ErrorCode;
pub mod backup;
pub mod manager;
pub mod snapshot;
#[cfg(test)]
mod tests;

pub static META_MODELS: LazyLock<Models> = LazyLock::new(|| {
    let mut adapter = ModelsAdapter::new();
    adapter.register_metadata_models();
    adapter.models
});

pub struct ModelsAdapter {
    pub models: Models,
}

impl ModelsAdapter {
    pub fn new() -> Self {
        ModelsAdapter {
            models: Models::new(),
        }
    }

    pub fn register_model<T: ToInput>(&mut self) {
        self.models.define::<T>().expect("failed to define model ");
    }

    pub fn register_metadata_models(&mut self) {
        self.register_model::<AccessToken>();
        self.register_model::<SystemSetting>();
        self.register_model::<License>();
        self.register_model::<CachedMailSettings>();
        self.register_model::<Account>();
        self.register_model::<AccountV2>();
        self.register_model::<AccountV3>();
        self.register_model::<EmailTemplate>();
        self.register_model::<Mta>();
        self.register_model::<OAuth2>();
        self.register_model::<OAuth2V2>();
        self.register_model::<OAuth2PendingEntity>();
        self.register_model::<OAuth2AccessToken>();
        self.register_model::<EventHooks>();
        self.register_model::<CacheItem>();
        self.register_model::<AccountRunningState>();
        self.register_model::<DailyMetrics>();
        self.register_model::<Proxy>();
    }
}

pub async fn insert_impl<T: ToInput + Clone + Send + 'static>(
    database: &Arc<Database<'static>>,
    item: T,
) -> RustMailerResult<()> {
    let db = database.clone();
    tokio::task::spawn_blocking(move || {
        let rw_transaction = db
            .rw_transaction()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        rw_transaction
            .insert(item)
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        rw_transaction
            .commit()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        Ok(())
    })
    .await
    .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
}

pub async fn batch_insert_impl<T: ToInput + Clone + Send + 'static>(
    database: &Arc<Database<'static>>,
    batch: Vec<T>,
) -> RustMailerResult<()> {
    let db = database.clone();
    tokio::task::spawn_blocking(move || {
        let rw_transaction = db
            .rw_transaction()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        for item in batch {
            rw_transaction
                .insert(item)
                .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        }
        rw_transaction
            .commit()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        Ok(())
    })
    .await
    .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
}

pub async fn batch_upsert_impl<T: ToInput + Clone + Send + 'static>(
    database: &Arc<Database<'static>>,
    batch: Vec<T>,
) -> RustMailerResult<()> {
    let db = database.clone();
    tokio::task::spawn_blocking(move || {
        let rw_transaction = db
            .rw_transaction()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        for item in batch {
            rw_transaction
                .upsert(item)
                .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        }
        rw_transaction
            .commit()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        Ok(())
    })
    .await
    .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
}

pub async fn with_transaction(
    database: &Arc<Database<'static>>,
    f: impl FnOnce(&RwTransaction) -> RustMailerResult<()> + Send + 'static,
) -> RustMailerResult<()> {
    let db = database.clone();
    tokio::task::spawn_blocking(move || {
        let rw_transaction = db
            .rw_transaction()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        f(&rw_transaction)?;
        rw_transaction
            .commit()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        Ok(())
    })
    .await
    .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
}

pub async fn upsert_impl<T: ToInput + Clone + Send + 'static>(
    database: &Arc<Database<'static>>,
    item: T,
) -> RustMailerResult<()> {
    let db = database.clone();
    tokio::task::spawn_blocking(move || {
        let rw_transaction = db
            .rw_transaction()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        rw_transaction
            .upsert(item)
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        rw_transaction
            .commit()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        Ok(())
    })
    .await
    .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
}

pub async fn update_impl<T: ToInput + Clone + std::fmt::Debug + Send + 'static>(
    database: &Arc<Database<'static>>,
    current: impl FnOnce(&RwTransaction) -> RustMailerResult<T> + Send + 'static,
    updated: impl FnOnce(&T) -> RustMailerResult<T> + Send + 'static,
) -> RustMailerResult<T> {
    let db = database.clone();
    tokio::task::spawn_blocking(move || {
        let rw = db
            .rw_transaction()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        let current_item = current(&rw)?;
        let updated_item = updated(&current_item)?;
        rw.update(current_item.clone(), updated_item)
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        rw.commit()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        Ok(current_item)
    })
    .await
    .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
}

pub async fn batch_update_impl<T: ToInput + Clone + std::fmt::Debug + Send + 'static>(
    database: &Arc<Database<'static>>,
    filter: impl FnOnce(&RwTransaction) -> RustMailerResult<Vec<T>> + Send + 'static,
    updated: impl FnOnce(&Vec<T>) -> RustMailerResult<Vec<(T, T)>> + Send + 'static,
) -> RustMailerResult<Vec<T>> {
    let db = database.clone();
    tokio::task::spawn_blocking(move || {
        let rw = db
            .rw_transaction()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        let targets = filter(&rw)?;
        let tuples = updated(&targets)?;
        for (old, updated) in tuples {
            rw.update(old, updated)
                .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        }
        rw.commit()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        Ok(targets)
    })
    .await
    .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
}

pub async fn async_find_impl<T: ToInput + Clone + Send + 'static>(
    database: &Arc<Database<'static>>,
    key: impl ToKey + Send + 'static,
) -> RustMailerResult<Option<T>> {
    let db = database.clone();
    tokio::task::spawn_blocking(move || {
        let r_transaction = db
            .r_transaction()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        let entity: Option<T> = r_transaction
            .get()
            .primary(key)
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        Ok(entity)
    })
    .await
    .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
}

pub async fn delete_impl<T: ToInput + Clone + Send + 'static>(
    database: &Arc<Database<'static>>,
    delete: impl FnOnce(&RwTransaction) -> RustMailerResult<T> + Send + 'static,
) -> RustMailerResult<()> {
    let db = database.clone();
    tokio::task::spawn_blocking(move || {
        let rw_transaction = db
            .rw_transaction()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        let to_delete = delete(&rw_transaction)?;
        rw_transaction
            .remove::<T>(to_delete)
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        rw_transaction
            .commit()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        Ok(())
    })
    .await
    .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
}

pub async fn batch_delete_impl<T: ToInput + Clone + Send + 'static>(
    database: &Arc<Database<'static>>,
    delete: impl FnOnce(&RwTransaction) -> RustMailerResult<Vec<T>> + Send + 'static,
) -> RustMailerResult<usize> {
    let db = database.clone();
    tokio::task::spawn_blocking(move || {
        let rw_transaction = db
            .rw_transaction()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        let to_delete = delete(&rw_transaction)?;
        let delete_count = to_delete.len();
        for item in to_delete {
            rw_transaction
                .remove(item)
                .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        }
        rw_transaction
            .commit()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        Ok(delete_count)
    })
    .await
    .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
}

pub async fn list_all_impl<T: ToInput + Clone + Send + 'static>(
    database: &Arc<Database<'static>>,
) -> RustMailerResult<Vec<T>> {
    let db = database.clone();
    tokio::task::spawn_blocking(move || {
        let r_transaction = db
            .r_transaction()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        let entities: Vec<T> = r_transaction
            .scan()
            .primary()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
            .all()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
            .try_collect()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        Ok(entities)
    })
    .await
    .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
}

// For tables with a creation timestamp, place the creation time at the front of the primary key.
// This allows sorting by time, as the data is stored in dictionary order based on the primary key.
// If reverse sorting by time is needed, the iterator can be reversed.
pub async fn paginate_query_primary_scan_all_impl<
    T: ToInput + Serialize + std::fmt::Debug + std::marker::Unpin + Send + Sync + 'static,
>(
    database: &Arc<Database<'static>>,
    page: Option<u64>,
    page_size: Option<u64>,
    desc: Option<bool>,
) -> RustMailerResult<Paginated<T>> {
    let db = database.clone();

    tokio::task::spawn_blocking(move || {
        let r_transaction = db
            .r_transaction()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        let total_items = r_transaction
            .len()
            .primary::<T>()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;

        // Validate page and page_size
        let (offset, total_pages) = if let (Some(p), Some(s)) = (page, page_size) {
            if p == 0 || s == 0 {
                return Err(raise_error!(
                    "'page' and 'page_size' must be greater than 0.".into(),
                    ErrorCode::InvalidParameter
                ));
            }
            let offset = (p - 1) * s;
            let total_pages = if total_items > 0 {
                (total_items as f64 / s as f64).ceil() as u64
            } else {
                0
            };
            (Some(offset), Some(total_pages))
        } else {
            (None, None)
        };

        // Handle empty result early
        if let Some(offset) = offset {
            if offset >= total_items {
                return Ok(Paginated::new(
                    page,
                    page_size,
                    total_items,
                    total_pages,
                    vec![],
                ));
            }
        }

        let scan = r_transaction
            .scan()
            .primary()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        let iter = scan
            .all()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;

        // Collect items based on the reverse flag and pagination
        let items: Vec<T> = match desc {
            Some(true) => iter
                .rev()
                .skip(offset.unwrap_or(0) as usize)
                .take(page_size.unwrap_or(total_items) as usize)
                .try_collect()
                .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?,
            _ => iter
                .skip(offset.unwrap_or(0) as usize)
                .take(page_size.unwrap_or(total_items) as usize)
                .try_collect()
                .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?,
        };

        Ok(Paginated::new(
            page,
            page_size,
            total_items,
            total_pages,
            items,
        ))
    })
    .await
    .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
}

pub async fn filter_by_secondary_key_impl<T: ToInput + Clone + Send + 'static>(
    database: &Arc<Database<'static>>,
    key_def: impl ToKeyDefinition<KeyOptions> + Send + 'static,
    start_with: impl ToKey + Send + 'static,
) -> RustMailerResult<Vec<T>> {
    let db = database.clone();
    tokio::task::spawn_blocking(move || {
        let r_transaction = db
            .r_transaction()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        let entities: Vec<T> = r_transaction
            .scan()
            .secondary(key_def)
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
            .start_with(start_with)
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
            .try_collect()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        Ok(entities)
    })
    .await
    .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
}

pub async fn count_by_unique_secondary_key_impl<T: ToInput + Clone + Send + 'static>(
    database: &Arc<Database<'static>>,
    key_def: impl ToKeyDefinition<KeyOptions> + Send + 'static,
) -> RustMailerResult<usize> {
    let db = database.clone();
    tokio::task::spawn_blocking(move || {
        let r_transaction = db
            .r_transaction()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        let count = r_transaction
            .scan()
            .secondary::<T>(key_def)
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
            .all()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
            .count();
        Ok(count)
    })
    .await
    .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
}

pub async fn secondary_find_impl<T: ToInput + Clone + Send + 'static>(
    database: &Arc<Database<'static>>,
    key_def: impl ToKeyDefinition<KeyOptions> + Send + 'static,
    key: impl ToKey + Send + 'static,
) -> RustMailerResult<Option<T>> {
    let db = database.clone();
    tokio::task::spawn_blocking(move || {
        let r_transaction = db
            .r_transaction()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;

        let entities: Option<T> = r_transaction
            .get()
            .secondary(key_def, key)
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;

        Ok(entities)
    })
    .await
    .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
}

pub async fn paginate_secondary_scan_impl<
    T: ToInput + Serialize + std::fmt::Debug + std::marker::Unpin + Send + Sync + 'static,
>(
    database: &Arc<Database<'static>>,
    page: Option<u64>,
    page_size: Option<u64>,
    desc: Option<bool>,
    key_def: impl ToKeyDefinition<KeyOptions> + Send + 'static,
    start_with: impl ToKey + Send + Clone + 'static,
) -> RustMailerResult<Paginated<T>> {
    let db = database.clone();
    tokio::task::spawn_blocking(move || {
        let r_transaction = db
            .r_transaction()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        let scan = r_transaction
            .scan()
            .secondary::<T>(key_def)
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        let iter = scan
            .start_with(start_with.clone())
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        let total_items = iter.count() as u64;

        // Validate page and page_size
        let (offset, total_pages) = if let (Some(p), Some(s)) = (page, page_size) {
            if p == 0 || s == 0 {
                return Err(raise_error!(
                    "'page' and 'page_size' must be greater than 0.".to_string(),
                    ErrorCode::InvalidParameter
                ));
            }
            let offset = (p - 1) * s;
            let total_pages = if total_items > 0 {
                (total_items as f64 / s as f64).ceil() as u64
            } else {
                0
            };
            (Some(offset), Some(total_pages))
        } else {
            (None, None)
        };

        // Handle empty result early
        if let Some(offset) = offset {
            if offset >= total_items {
                return Ok(Paginated::new(
                    page,
                    page_size,
                    total_items,
                    total_pages,
                    vec![],
                ));
            }
        }

        let iter = scan
            .start_with(start_with)
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;

        // Collect items based on the reverse flag and pagination
        let items: Vec<T> = match desc {
            Some(true) => iter
                .rev()
                .skip(offset.unwrap_or(0) as usize)
                .take(page_size.unwrap_or(total_items) as usize)
                .try_collect()
                .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?,
            _ => iter
                .skip(offset.unwrap_or(0) as usize)
                .take(page_size.unwrap_or(total_items) as usize)
                .try_collect()
                .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?,
        };

        Ok(Paginated::new(
            page,
            page_size,
            total_items,
            total_pages,
            items,
        ))
    })
    .await
    .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
}

#[derive(Debug)]
pub struct Paginated<T> {
    pub page: Option<u64>,
    pub page_size: Option<u64>,
    pub total_items: u64,
    pub total_pages: Option<u64>,
    pub items: Vec<T>,
}

impl<T> Paginated<T> {
    pub fn new(
        page: Option<u64>,
        page_size: Option<u64>,
        total_items: u64,
        total_pages: Option<u64>,
        items: Vec<T>,
    ) -> Self {
        Paginated {
            page,
            page_size,
            total_items,
            total_pages,
            items,
        }
    }
}
