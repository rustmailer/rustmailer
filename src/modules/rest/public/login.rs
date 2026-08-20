// Copyright © 2025-2026 rustmailer.com
// Licensed under RustMailer License Agreement v1.0
// Unauthorized copying, modification, or distribution is prohibited.

use crate::modules::token::root::check_root_password;
use poem::{handler, IntoResponse, Response};
use tracing::error;

/// Login endpoint for Root user
///
/// Accepts a plain text password and returns the `root_token`
/// on successful authentication.
#[handler]
pub async fn login(password: String) -> Response {
    match check_root_password(&password).await {
        Ok(root_token) => Response::builder()
            .status(http::StatusCode::OK)
            .content_type("text/plain")
            .body(root_token)
            .into_response(),
        Err(e) => {
            error!("Root login failed: {:?}", e);
            Response::builder()
                .status(http::StatusCode::UNAUTHORIZED)
                .content_type("text/plain")
                .body(e.to_string())
                .into_response()
        }
    }
}
