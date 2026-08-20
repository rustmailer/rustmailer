// Copyright © 2025-2026 rustmailer.com
// Licensed under RustMailer License Agreement v1.0
// Unauthorized copying, modification, or distribution is prohibited.

use crate::{
    modules::{
        error::{code::ErrorCode, RustMailerError, RustMailerResult},
        license::cache::CachedLicense,
        settings::{cli::SETTINGS, system::SystemSetting},
        token::{root::ROOT_TOKEN, AccessToken, AccessTokenScope, AccountInfo},
        utils::rate_limit::RATE_LIMITER_MANAGER,
    },
    raise_error,
};
use governor::clock::{Clock, QuantaClock};
use http::Method;
use poem::{
    web::{
        headers::{authorization::Bearer, Authorization, HeaderMapExt},
        RealIp,
    },
    Endpoint, FromRequest, Middleware, Request, RequestBody, Result,
};
use serde::Deserialize;
use std::{collections::BTreeSet, net::IpAddr, sync::Arc};

use super::create_api_error_response;

pub struct ApiGuard;

pub struct ApiGuardEndpoint<E> {
    ep: E,
}

impl<E: Endpoint> Middleware<E> for ApiGuard {
    type Output = ApiGuardEndpoint<E>;

    fn transform(&self, ep: E) -> Self::Output {
        ApiGuardEndpoint { ep }
    }
}

#[derive(Deserialize)]
struct Param {
    access_token: String,
}

impl<E: Endpoint> Endpoint for ApiGuardEndpoint<E> {
    type Output = E::Output;

    async fn call(&self, mut req: Request) -> Result<Self::Output> {
        let set_license = matches!(
            (req.method(), req.uri().path()),
            (&Method::POST, "/api/v1/license")
        );
        if !set_license {
            CachedLicense::check_license_validity()
                .await
                .map_err(|error| match error {
                    RustMailerError::Generic {
                        message,
                        location: _,
                        code,
                    } => create_api_error_response(&message, code),
                })?;
        }
        let context = authorize_access(&req, None).await?;
        req.set_data(Arc::new(context));
        self.ep.call(req).await
    }
}

#[derive(Clone, Debug, Default)]
pub struct ClientContext {
    pub ip_addr: Option<IpAddr>,
    pub access_token: Option<AccessToken>,
    pub is_root: bool,
}

impl ClientContext {
    pub fn require_root(&self) -> RustMailerResult<()> {
        if !SETTINGS.rustmailer_enable_access_token || self.is_root {
            Ok(())
        } else {
            Err(raise_error!(
                "Root access required".into(),
                ErrorCode::PermissionDenied
            ))
        }
    }

    pub fn require_authorized(&self) -> RustMailerResult<()> {
        if !SETTINGS.rustmailer_enable_access_token || self.is_root || self.access_token.is_some() {
            Ok(())
        } else {
            Err(raise_error!(
                "Authorization required".into(),
                ErrorCode::PermissionDenied
            ))
        }
    }

    pub fn require_account_access(&self, account_id: u64) -> RustMailerResult<()> {
        if !SETTINGS.rustmailer_enable_access_token || self.is_root {
            return Ok(());
        }

        match &self.access_token {
            Some(token) if token.can_access_account(account_id) => Ok(()),
            _ => Err(raise_error!(format!(
                "You do not have permission to access the requested email account (ID: {}). Please check your access rights or contact the administrator.",
                account_id
            ), ErrorCode::PermissionDenied)),
        }
    }

    pub fn accessible_accounts(&self) -> RustMailerResult<Option<&BTreeSet<AccountInfo>>> {
        if !SETTINGS.rustmailer_enable_access_token || self.is_root {
            Ok(None) // All accounts are accessible
        } else {
            match &self.access_token {
                Some(token) => Ok(Some(&token.accounts)),
                None => Err(raise_error!(
                    "Missing access token".into(),
                    ErrorCode::PermissionDenied
                )),
            }
        }
    }
}

impl<'a> FromRequest<'a> for ClientContext {
    async fn from_request(req: &'a Request, _body: &mut RequestBody) -> Result<Self> {
        extract_client_context(req).await
    }
}

pub async fn extract_client_context(req: &Request) -> Result<ClientContext> {
    if SETTINGS.rustmailer_enable_access_token {
        let ip_addr = RealIp::from_request_without_body(req)
            .await
            .map_err(|_| {
                create_api_error_response(
                    "Failed to parse client IP address",
                    ErrorCode::InvalidParameter,
                )
            })?
            .0
            .ok_or_else(|| {
                create_api_error_response(
                    "Failed to parse client IP address",
                    ErrorCode::InvalidParameter,
                )
            })?;
        // Extract access token from Bearer header or query params
        let bearer = req
            .headers()
            .typed_get::<Authorization<Bearer>>()
            .map(|auth| auth.0.token().to_string())
            .or_else(|| req.params::<Param>().ok().map(|param| param.access_token));

        let token = bearer.ok_or_else(|| {
            create_api_error_response("Valid access token not found", ErrorCode::PermissionDenied)
        })?;

        // Check for root token
        if let Ok(Some(root)) = SystemSetting::get(ROOT_TOKEN).await {
            if root.value == token {
                return Ok(ClientContext {
                    ip_addr: Some(ip_addr),
                    access_token: None,
                    is_root: true,
                });
            }
        }

        // Validate and update access token
        let validated_token = AccessToken::try_update_access_timestamp(&token)
            .await
            .map_err(|_| {
                create_api_error_response("Invalid access token", ErrorCode::PermissionDenied)
            })?;

        return Ok(ClientContext {
            ip_addr: Some(ip_addr),
            access_token: Some(validated_token),
            is_root: false,
        });
    }

    Ok(Default::default())
}

pub async fn authorize_access(
    req: &Request,
    required_scope: Option<AccessTokenScope>,
) -> Result<ClientContext, poem::Error> {
    let context = extract_client_context(&req).await?;
    context.require_authorized().map_err(|error| {
        create_api_error_response(&error.to_string(), ErrorCode::PermissionDenied)
    })?;

    if let Some(access_token) = &context.access_token {
        if let Some(scope) = required_scope {
            if !access_token.access_scopes.contains(&scope) {
                return Err(create_api_error_response(
                    &format!("Token lacks required '{:?}' scope", scope),
                    ErrorCode::PermissionDenied,
                ));
            }
        }

        if let Some(access_control) = &access_token.acl {
            if let Some(ip_addr) = context.ip_addr {
                if let Some(whitelist) = &access_control.ip_whitelist {
                    if !whitelist.contains(&ip_addr.to_string()) {
                        return Err(create_api_error_response(
                            &format!("IP {} not in whitelist", ip_addr),
                            ErrorCode::PermissionDenied,
                        ));
                    }
                }
            }

            if let Some(rate_limit) = &access_control.rate_limit {
                if let Err(not_until) = RATE_LIMITER_MANAGER
                    .check(&access_token.token, rate_limit.clone())
                    .await
                {
                    let wait_duration = not_until.wait_time_from(QuantaClock::default().now());
                    return Err(create_api_error_response(
                        &format!(
                            "Rate limit: {}/{}s. Retry after {}s",
                            rate_limit.quota,
                            rate_limit.interval,
                            wait_duration.as_secs()
                        ),
                        ErrorCode::TooManyRequest,
                    ));
                }
            }
        }
    }

    Ok(context)
}
