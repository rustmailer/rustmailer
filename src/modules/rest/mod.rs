// Copyright © 2025-2026 rustmailer.com
// Licensed under RustMailer License Agreement v1.0
// Unauthorized copying, modification, or distribution is prohibited.

use crate::modules::common::error::ErrorCapture;
use crate::modules::common::log::Tracing;
use crate::modules::common::tls::rustls_config;
use crate::modules::error::code::ErrorCode;
use crate::modules::error::handler::error_handler;
use crate::modules::error::RustMailerResult;
use crate::modules::metrics::endpoint::PrometheusEndpoint;
use crate::modules::rest::public::login::login;
use crate::modules::rest::public::status::get_status;
use crate::modules::{settings::cli::SETTINGS, utils::shutdown::shutdown_signal};

use super::error::ApiErrorResponse;
use crate::modules::common::auth::ApiGuard;
use crate::modules::common::timeout::{Timeout, TIMEOUT_HEADER};
use crate::raise_error;
use api::create_openapi_service;
use assets::FrontEndAssets;
use http::HeaderValue;
use poem::endpoint::EmbeddedFilesEndpoint;
use poem::listener::{Listener, TcpListener};
use poem::middleware::{CatchPanic, Compression, SetHeader};
use poem::{endpoint::EmbeddedFileEndpoint, middleware::Cors, EndpointExt, Route, Server};
use poem::{get, post};
use poem_openapi::ContactObject;
use public::oauth2::oauth2_callback;
use public::tracking::get_tracking_code;
use std::time::Duration;

pub mod api;
pub mod assets;
pub mod public;
pub mod response;

pub type ApiResult<T, E = ApiErrorResponse> = std::result::Result<T, E>;

const DESCRIPTION: &str = r#"
    RustMailer is a self-hosted IMAP, SMTP, Gmail API, Graph API middleware platform designed for developers and businesses seeking a robust, scalable, and secure email solution.

    - Provides seamless IMAP synchronization and reliable SMTP sending via blazing-fast REST and gRPC APIs.
    - Supports programmable email workflows, customizable filters, and webhook notifications.
    - Offers multi-account synchronization, license-based access control, and a built-in web UI for easy management.

    Whether you're building SaaS platforms, CRM systems, or customer support tools, RustMailer delivers high performance and full control over your email infrastructure.
"#;

pub async fn start_http_server() -> RustMailerResult<()> {
    let listener = TcpListener::bind((
        SETTINGS
            .rustmailer_bind_ip
            .clone()
            .unwrap_or("0.0.0.0".into()),
        SETTINGS.rustmailer_http_port as u16,
    ));

    let listener = if SETTINGS.rustmailer_enable_rest_https {
        listener.rustls(rustls_config()?).boxed()
    } else {
        listener.boxed()
    };

    let api_service = create_openapi_service()
        .description(DESCRIPTION)
        .contact(ContactObject::new().email("rustmailer.git@gmail.com"))
        .license("https://rustmailer.com/license")
        .external_document("https://rustmailer.com/docs")
        .summary("A self-hosted Email Middleware for IMAP, SMTP, Gmail API, Graph API — built for developers");

    let swagger = api_service.swagger_ui();
    let redoc = api_service.redoc();
    let scalar = api_service.scalar();
    let spec_json = api_service.spec_endpoint();
    let spec_yaml = api_service.spec_endpoint_yaml();
    let openapi_explorer = api_service.openapi_explorer();

    let open_api_route = Route::new()
        .nest_no_strip("/api/v1", api_service)
        .with(ApiGuard)
        .with(ErrorCapture)
        .with(Timeout)
        .with(Tracing);

    let cors_origins = SETTINGS.rustmailer_cors_origins.clone();
    let cors_origins: Vec<String> = cors_origins.unwrap_or_default().into_iter().collect();

    let cors = Cors::new()
        .allow_origins_fn(move |origin| {
            tracing::debug!("CORS: Incoming Origin = {:?}", origin);
            tracing::debug!("CORS: Configured origins = {:?}", cors_origins);
            if cors_origins.is_empty() {
                tracing::debug!("CORS: No origins configured, allowing all");
                return true;
            }
            cors_origins.iter().any(|o| o == origin)
        })
        .allow_credentials(true)
        .allow_methods(vec![
            "GET", "POST", "PUT", "DELETE", "OPTIONS", "HEAD", "PATCH",
        ])
        .allow_headers(vec!["Content-Type", "Authorization", TIMEOUT_HEADER])
        .expose_headers(vec!["Accept"])
        .max_age(SETTINGS.rustmailer_cors_max_age);

    let cache_static = || {
        SetHeader::new().overriding(
            http::header::CACHE_CONTROL,
            HeaderValue::from_static("max-age=86400"),
        )
    };
    let route = Route::new()
        .nest("/api-docs/swagger", swagger)
        .nest("/api-docs/redoc", redoc)
        .nest("/api-docs/explorer", openapi_explorer)
        .nest("/api-docs/scalar", scalar)
        .nest("/api-docs/spec.json", spec_json)
        .nest("/api-docs/spec.yaml", spec_yaml)
        .nest("/metrics", PrometheusEndpoint)
        .nest("/oauth2/callback", get(oauth2_callback))
        .at("/email-track/:id", get(get_tracking_code))
        .nest("/api/status", get(get_status))
        .nest("/api/login", post(login))
        .nest_no_strip("/api/v1", open_api_route)
        .nest_no_strip(
            "/assets",
            EmbeddedFilesEndpoint::<FrontEndAssets>::new().with(cache_static()),
        )
        .at(
            "/*",
            EmbeddedFileEndpoint::<FrontEndAssets>::new("index.html"),
        )
        .with(cors)
        .with_if(
            SETTINGS.rustmailer_http_compression_enabled,
            Compression::new(),
        )
        .with(CatchPanic::new());

    let server = Server::new(listener)
        .name("RustMailer API Service")
        .idle_timeout(Duration::from_secs(60))
        .run_with_graceful_shutdown(
            route.catch_all_error(error_handler),
            shutdown_signal(),
            Some(Duration::from_secs(5)),
        );
    println!(
        "RustMailer API Service is now running on port {}.",
        SETTINGS.rustmailer_http_port
    );
    server
        .await
        .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))
}
