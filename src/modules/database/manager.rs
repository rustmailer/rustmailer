// Copyright © 2025-2026 rustmailer.com
// Licensed under RustMailer License Agreement v1.0
// Unauthorized copying, modification, or distribution is prohibited.

use crate::modules::account::migration::AccountModel;
use crate::modules::cache::imap::migration::EmailEnvelopeV3;
use crate::modules::cache::imap::ENVELOPE_MODELS;
use crate::modules::context::Initialize;
use crate::modules::error::{code::ErrorCode, RustMailerError};
use crate::modules::oauth2::entity::OAuth2Model;
use crate::modules::scheduler::nativedb::TaskMetaEntity;
use crate::modules::settings::cli::SETTINGS;
use crate::modules::settings::dir::{DATA_DIR_MANAGER, META_FILE, TASK_FILE};
use crate::modules::{
    database::META_MODELS, error::RustMailerResult, scheduler::nativedb::TASK_MODELS,
};
use crate::raise_error;
use native_db::{Builder, Database};
use std::sync::{Arc, LazyLock};
use tracing::{info, warn};

use crate::modules::{
    account::status::AccountRunningState,
    autoconfig::CachedMailSettings,
    cache::disk::CacheItem,
    database::{batch_insert_impl, list_all_impl},
    hook::entity::EventHooks,
    license::License,
    oauth2::{pending::OAuth2PendingEntity, token::OAuth2AccessToken},
    overview::metrics::DailyMetrics,
    settings::{proxy::Proxy, system::SystemSetting},
    smtp::{mta::entity::Mta, template::entity::EmailTemplate},
    token::AccessToken,
};

pub static DB_MANAGER: LazyLock<DatabaseManager> = LazyLock::new(DatabaseManager::new);

/// Metadata database instance
pub struct DatabaseManager {
    meta_db: Arc<Database<'static>>,
    /// Task scheduling database instance
    tasks_db: Arc<Database<'static>>,
    /// Envelope database instance
    envelope_db: Arc<Database<'static>>,
}

impl DatabaseManager {
    fn new() -> Self {
        let meta_db = Self::init_meta_database().expect("Failed to initialize metadata database");
        let tasks_db =
            Self::init_task_queue_database().expect("Failed to initialize tasks database");
        let envelope_db =
            Self::init_evenlope_database().expect("Failed to initialize evenlope database");
        DatabaseManager {
            meta_db,
            tasks_db,
            envelope_db,
        }
    }

    /// Get a reference to the metadata database
    pub fn meta_db(&self) -> &Arc<Database<'static>> {
        &self.meta_db
    }

    /// Get a reference to the task scheduler database
    pub fn tasks_db(&self) -> &Arc<Database<'static>> {
        &self.tasks_db
    }

    pub fn envelope_db(&self) -> &Arc<Database<'static>> {
        &self.envelope_db
    }
    /// Initialize metadata database with a fixed or configured file path
    fn init_meta_database() -> RustMailerResult<Arc<Database<'static>>> {
        if SETTINGS.rustmailer_metadata_memory_mode_enabled {
            return Ok(Arc::new(
                Builder::new().create_in_memory(&META_MODELS).unwrap(),
            ));
        }
        let mut database = Builder::new()
            .set_cache_size(
                SETTINGS
                    .rustmailer_metadata_cache_size
                    .unwrap_or(134217728)
                    .max(67108864),
            ) //default 128MB
            .create(&META_MODELS, DATA_DIR_MANAGER.meta_db.clone())
            .map_err(Self::handle_database_error)?;

        let rw = database
            .rw_transaction()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        rw.migrate::<AccountModel>()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        rw.migrate::<OAuth2Model>()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        rw.commit()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;

        database
            .compact()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        Ok(Arc::new(database))
    }

    pub async fn load_meta_snapshot(&self) -> RustMailerResult<()> {
        let lastest_snapshot = DATA_DIR_MANAGER.find_latest_snapshot_for(META_FILE);
        let snapshot = match lastest_snapshot {
            Some(snapshot) => {
                info!("Found existing meta snapshot: {:?}", snapshot);
                snapshot
            }
            None => {
                warn!("No meta snapshot found in the data directory");
                info!("Creating new meta snapshot instance");
                return Ok(());
            }
        };

        let database = Arc::new(
            Builder::new()
                .create(&META_MODELS, snapshot)
                .map_err(Self::handle_database_error)?,
        );

        let mut join_set = tokio::task::JoinSet::new();
        macro_rules! spawn_migration_task {
            ($table:ty) => {
                let db = Arc::clone(&database);
                let mem_db = Arc::clone(&self.meta_db);
                join_set.spawn(async move {
                    let data = list_all_impl::<$table>(&db).await?;
                    batch_insert_impl(&mem_db, data).await
                });
            };
        }

        spawn_migration_task!(AccessToken);
        spawn_migration_task!(SystemSetting);
        spawn_migration_task!(License);
        spawn_migration_task!(CachedMailSettings);
        spawn_migration_task!(AccountModel);
        spawn_migration_task!(EmailTemplate);
        spawn_migration_task!(Mta);
        spawn_migration_task!(OAuth2Model);
        spawn_migration_task!(OAuth2PendingEntity);
        spawn_migration_task!(OAuth2AccessToken);
        spawn_migration_task!(EventHooks);
        spawn_migration_task!(CacheItem);
        spawn_migration_task!(AccountRunningState);
        spawn_migration_task!(DailyMetrics);
        spawn_migration_task!(Proxy);

        while let Some(res) = join_set.join_next().await {
            match res {
                Ok(inner_res) => inner_res?,
                Err(join_err) => {
                    return Err(raise_error!(
                        format!("{:#?}", join_err),
                        ErrorCode::InternalError
                    ))
                }
            }
        }

        Ok(())
    }

    fn init_task_queue_database() -> RustMailerResult<Arc<Database<'static>>> {
        if SETTINGS.rustmailer_metadata_memory_mode_enabled {
            return Ok(Arc::new(
                Builder::new().create_in_memory(&TASK_MODELS).unwrap(),
            ));
        }
        let mut database = Builder::new()
            .set_cache_size(
                SETTINGS
                    .rustmailer_task_queue_cache_size
                    .unwrap_or(134217728)
                    .max(67108864),
            ) //default 128MB
            .create(&TASK_MODELS, DATA_DIR_MANAGER.task_db.clone())
            .map_err(Self::handle_database_error)?;
        database
            .compact()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        Ok(Arc::new(database))
    }

    pub async fn load_task_snapshot(&self) -> RustMailerResult<()> {
        let lastest_snapshot = DATA_DIR_MANAGER.find_latest_snapshot_for(TASK_FILE);
        let snapshot = match lastest_snapshot {
            Some(snapshot) => {
                info!("Found existing task snapshot: {:?}", snapshot);
                snapshot
            }
            None => {
                warn!("No task snapshot found in the data directory");
                info!("Creating new task snapshot instance");
                return Ok(());
            }
        };

        let database = Arc::new(
            Builder::new()
                .create(&TASK_MODELS, snapshot)
                .map_err(Self::handle_database_error)?,
        );

        let data = list_all_impl::<TaskMetaEntity>(&database).await?;
        batch_insert_impl(&self.tasks_db, data).await?;

        Ok(())
    }

    fn init_evenlope_database() -> RustMailerResult<Arc<Database<'static>>> {
        info!(
            "Initializing envelope database at: {:?}",
            &DATA_DIR_MANAGER.envelope_db
        );

        let mut database = Builder::new()
            .set_cache_size(
                SETTINGS
                    .rustmailer_envelope_cache_size
                    .unwrap_or(1073741824)
                    .max(67108864),
            ) //default 1GB
            .create(&ENVELOPE_MODELS, DATA_DIR_MANAGER.envelope_db.clone())
            .map_err(Self::handle_database_error)?;

        let rw = database
            .rw_transaction()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        rw.migrate::<EmailEnvelopeV3>()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        rw.commit()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;

        database
            .compact()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;

        Ok(Arc::new(database))
    }

    fn handle_database_error(error: native_db::db_type::Error) -> RustMailerError {
        match error {
            native_db::db_type::Error::RedbDatabaseError(database_error) => match database_error {
                redb::DatabaseError::DatabaseAlreadyOpen => {
                    raise_error!(
                        "Database is already open by another instance".into(),
                        ErrorCode::InternalError
                    )
                }
                other => {
                    raise_error!(
                        format!("Database error: {:?}", other),
                        ErrorCode::InternalError
                    )
                }
            },
            other => {
                raise_error!(
                    format!("Failed to create database: {:?}", other),
                    ErrorCode::InternalError
                )
            }
        }
    }
}

impl Initialize for DatabaseManager {
    async fn initialize() -> RustMailerResult<()> {
        if SETTINGS.rustmailer_metadata_memory_mode_enabled {
            DB_MANAGER.load_meta_snapshot().await?;
            DB_MANAGER.load_task_snapshot().await?;
        }
        Ok(())
    }
}
