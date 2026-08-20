// Copyright © 2025-2026 rustmailer.com
// Licensed under RustMailer License Agreement v1.0
// Unauthorized copying, modification, or distribution is prohibited.

use mimalloc::MiMalloc;
use modules::{
    common::{rustls::RustMailerTls, watchdog::init_watchdog},
    context::{executors::EmailClientExecutors, Initialize},
    error::{code::ErrorCode, RustMailerResult},
    grpc::server::start_grpc_server,
    license::License,
    logger,
    rest::start_http_server,
    settings::cli::SETTINGS,
    tasks::{queue::RustMailerTaskQueue, PeriodicTasks},
    token::root::ensure_root_token,
};
use tracing::{error, info};

use crate::modules::{
    cache::imap::manager::EnvelopeFlagsManager,
    common::signal::SignalManager,
    database::{manager::DatabaseManager, snapshot::task::DatabaseSnapshotTask},
    metrics::MetricsService,
    settings::dir::DataDirManager,
};

mod modules;

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

static LOGO: &str = r#"
  ____            _   __  __       _ _           
 |  _ \ _   _ ___| |_|  \/  | __ _(_) | ___ _ __ 
 | |_) | | | / __| __| |\/| |/ _` | | |/ _ \ '__|
 |  _ <| |_| \__ \ |_| |  | | (_| | | |  __/ |   
 |_| \_\\__,_|___/\__|_|  |_|\__,_|_|_|\___|_|   
                                                 
"#;
#[tokio::main]
async fn main() -> RustMailerResult<()> {
    logger::initialize_logging();
    info!("{}", LOGO);
    info!("Starting rustmailer-server");
    info!("Version:  {}", rustmailer_version!());
    info!("Git:      [{}]", env!("GIT_HASH"));
    info!("Project:  https://rustmailer.com");
    info!("GitHub:   https://github.com/rustmailer/rustmailer");

    init_watchdog(SETTINGS.rustmailer_watchdog_timeout_secs);

    if let Err(error) = initialize().await {
        eprintln!("{:?}", error);
        return Err(error);
    }

    start_server().await?;
    snapshot_after_shutdown_if_needed().await;
    Ok(())
}

/// Initialize the system by validating settings and starting necessary tasks.
async fn initialize() -> RustMailerResult<()> {
    // SETTINGS.validate()?;
    SignalManager::initialize().await?;
    DataDirManager::initialize().await?;
    MetricsService::initialize().await?;
    DatabaseManager::initialize().await?;
    ensure_root_token().await?;
    License::initialize().await?;
    EnvelopeFlagsManager::initialize().await?;
    RustMailerTls::initialize().await?;
    EmailClientExecutors::initialize().await?;
    RustMailerTaskQueue::initialize().await?;
    PeriodicTasks::start_background_tasks();
    Ok(())
}

async fn start_server() -> RustMailerResult<()> {
    let mut servers = Vec::new();

    // Always start HTTP server
    let http_server = start_http_server();
    servers.push(tokio::spawn({
        let http_server = http_server;
        async move {
            let result = http_server.await;
            if let Err(e) = &result {
                error!("Failed to start REST server: {}", e);
            }
            result
        }
    }));

    // Start gRPC server if enabled
    if SETTINGS.rustmailer_grpc_enabled {
        let grpc_server = start_grpc_server();
        servers.push(tokio::spawn({
            let grpc_server = grpc_server;
            async move {
                let result = grpc_server.await;
                if let Err(e) = &result {
                    error!("Failed to start gRPC server: {}", e);
                }
                result
            }
        }));
    }

    let _ = futures::future::try_join_all(servers)
        .await
        .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
    Ok(())
}

pub async fn snapshot_after_shutdown_if_needed() {
    if SETTINGS.rustmailer_metadata_memory_mode_enabled {
        info!("[metadata] All servers shut down. Starting snapshot...");
        if let Err(e) = DatabaseSnapshotTask::block_snapshot().await {
            error!("[metadata] Snapshot after shutdown failed: {:?}", e);
        } else {
            info!("[metadata] Snapshot after shutdown completed.");
        }
    }
}
