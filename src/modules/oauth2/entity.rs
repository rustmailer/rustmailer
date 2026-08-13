// Copyright © 2025-2026 rustmailer.com
// Licensed under RustMailer License Agreement v1.0
// Unauthorized copying, modification, or distribution is prohibited.

use crate::{
    encrypt, id,
    modules::{
        database::{
            delete_impl, insert_impl, manager::DB_MANAGER, paginate_query_primary_scan_all_impl,
            secondary_find_impl, update_impl,
        },
        error::{code::ErrorCode, RustMailerResult},
        rest::response::DataPage,
    },
    raise_error, utc_now,
};
use native_db::*;
use native_model::{native_model, Model};
use poem_openapi::{Enum, Object};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub type OAuth2Model = OAuth2V2;

/// The grant type used by this OAuth2 configuration.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Enum)]
pub enum OAuth2GrantType {
    /// Authorization Code Flow with PKCE — requires user interaction.
    #[default]
    AuthorizationCode,
    /// Client Credentials Flow — no user interaction, suitable for service-to-service auth.
    ClientCredentials,
}

/// Represents the OAuth2 configuration for a client, including initialization and runtime values.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Object)]
#[native_model(id = 8, version = 1)]
#[native_db(primary_key(pk -> String))]
pub struct OAuth2 {
    /// A unique identifier for the OAuth2 configuration.
    #[secondary_key(unique)]
    pub id: u64,
    /// A description of what this configuration is used for.
    pub description: Option<String>,
    /// The client ID used for authenticating the application with the OAuth2 provider.
    pub client_id: String,
    /// The client secret used in conjunction with the client ID.
    ///
    /// Users should provide a plaintext secret.
    /// The server will encrypt it using AES-256-GCM and securely store it.
    /// The plaintext secret is never stored, so users must ensure it is valid for OAuth2 authentication.
    pub client_secret: String,
    /// The URL to redirect users to for OAuth2 authorization.
    pub auth_url: String,
    /// The URL to exchange authorization codes for access tokens.
    pub token_url: String,
    /// The URI where the OAuth2 provider will redirect to after authorization.
    pub redirect_uri: String,
    /// The scopes of access that are being requested (e.g., email, profile).
    pub scopes: Option<Vec<String>>,
    /// Any additional parameters to include in the OAuth2 requests (e.g., access_type, prompt).
    pub extra_params: Option<BTreeMap<String, String>>,
    /// Indicates whether this configuration is enabled or disabled.
    pub enabled: bool,
    /// route OAuth through proxy (when direct access is blocked)
    pub use_proxy: Option<u64>,
    /// The timestamp when the configuration was created, in milliseconds since the Unix epoch.
    pub created_at: i64,
    /// The timestamp when the configuration was last updated, in milliseconds since the Unix epoch.
    pub updated_at: i64,
}
impl OAuth2 {
    fn pk(&self) -> String {
        format!("{}_{}", &self.created_at, &self.id)
    }
}

/// Represents the OAuth2 configuration for a client, including initialization and runtime values.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Object)]
#[native_model(id = 8, version = 2, from = OAuth2)]
#[native_db(primary_key(pk -> String))]
pub struct OAuth2V2 {
    /// A unique identifier for the OAuth2 configuration.
    #[secondary_key(unique)]
    pub id: u64,
    /// A description of what this configuration is used for.
    pub description: Option<String>,
    /// The client ID used for authenticating the application with the OAuth2 provider.
    pub client_id: String,
    /// The client secret used in conjunction with the client ID.
    ///
    /// Users should provide a plaintext secret.
    /// The server will encrypt it using AES-256-GCM and securely store it.
    /// The plaintext secret is never stored, so users must ensure it is valid for OAuth2 authentication.
    pub client_secret: String,
    /// The URL to redirect users to for OAuth2 authorization.
    pub auth_url: String,
    /// The URL to exchange authorization codes for access tokens.
    pub token_url: String,
    /// The URI where the OAuth2 provider will redirect to after authorization.
    pub redirect_uri: String,
    /// The scopes of access that are being requested (e.g., email, profile).
    pub scopes: Option<Vec<String>>,
    /// Any additional parameters to include in the OAuth2 requests (e.g., access_type, prompt).
    pub extra_params: Option<BTreeMap<String, String>>,
    /// Indicates whether this configuration is enabled or disabled.
    pub enabled: bool,
    /// route OAuth through proxy (when direct access is blocked)
    pub use_proxy: Option<u64>,
    /// The grant type used for this OAuth2 configuration.
    pub grant_type: OAuth2GrantType,
    /// The timestamp when the configuration was created, in milliseconds since the Unix epoch.
    pub created_at: i64,
    /// The timestamp when the configuration was last updated, in milliseconds since the Unix epoch.
    pub updated_at: i64,
}

impl OAuth2V2 {
    fn pk(&self) -> String {
        format!("{}_{}", &self.created_at, &self.id)
    }

    pub fn new(request: OAuth2CreateRequest) -> RustMailerResult<Self> {
        let request = request.encrypt()?;
        Ok(OAuth2Model {
            id: id!(64),
            description: request.description,
            client_id: request.client_id,
            client_secret: request.client_secret,
            auth_url: request.auth_url,
            token_url: request.token_url,
            redirect_uri: request.redirect_uri,
            scopes: request.scopes,
            extra_params: request.extra_params,
            enabled: request.enabled,
            created_at: utc_now!(),
            updated_at: utc_now!(),
            use_proxy: request.use_proxy,
            grant_type: request.grant_type,
        })
    }

    pub async fn save(self) -> RustMailerResult<()> {
        insert_impl(DB_MANAGER.meta_db(), self).await?;
        Ok(())
    }

    pub async fn paginate_list(
        page: Option<u64>,
        page_size: Option<u64>,
        desc: Option<bool>,
    ) -> RustMailerResult<DataPage<OAuth2Model>> {
        paginate_query_primary_scan_all_impl(DB_MANAGER.meta_db(), page, page_size, desc)
            .await
            .map(DataPage::from)
    }

    pub async fn get(id: u64) -> RustMailerResult<Option<OAuth2Model>> {
        secondary_find_impl(DB_MANAGER.meta_db(), OAuth2V2Key::id, id).await
    }

    pub async fn delete(id: u64) -> RustMailerResult<()> {
        delete_impl(DB_MANAGER.meta_db(), move |rw| {
            rw.get()
                .secondary::<OAuth2Model>(OAuth2V2Key::id, id)
                .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
                .ok_or_else(|| {
                    raise_error!(
                        format!(
                            "The oauth2 entity with id={id} that you want to delete was not found."
                        ),
                        ErrorCode::ResourceNotFound
                    )
                })
        })
        .await
    }

    pub async fn update(id: u64, request: OAuth2UpdateRequest) -> RustMailerResult<()> {
        update_impl(
            DB_MANAGER.meta_db(),
            move |rw| {
                rw.get()
                    .secondary::<OAuth2Model>(OAuth2V2Key::id, id)
                    .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
                    .ok_or_else(|| {
                        raise_error!(
                            format!("The oauth2 entity with id={id} that you want to modify was not found."),
                            ErrorCode::ResourceNotFound
                        )
                    })
            },
            |current| apply_update(current, request),
        )
        .await?;

        Ok(())
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Object)]
pub struct OAuth2CreateRequest {
    /// A description of what this configuration is used for.
    pub description: Option<String>,

    /// The client ID used for authenticating the application with the OAuth2 provider.
    pub client_id: String,

    /// The client secret used in conjunction with the client ID.
    pub client_secret: String,

    /// The URL to redirect users to for OAuth2 authorization.
    pub auth_url: String,

    /// The URL to exchange authorization codes for access tokens.
    pub token_url: String,

    /// The URI where the OAuth2 provider will redirect to after authorization.
    pub redirect_uri: String,

    /// The scopes of access that are being requested (e.g., email, profile).
    pub scopes: Option<Vec<String>>,

    /// Any additional parameters to include in the OAuth2 requests (e.g., access_type, prompt).
    pub extra_params: Option<BTreeMap<String, String>>,

    /// Indicates whether this configuration is enabled or disabled.
    pub enabled: bool,

    /// route OAuth through proxy (when direct access is blocked)
    pub use_proxy: Option<u64>,

    /// The grant type to use for authentication.
    pub grant_type: OAuth2GrantType,
}

impl OAuth2CreateRequest {
    pub fn encrypt(self) -> RustMailerResult<Self> {
        Ok(Self {
            description: self.description,
            client_id: self.client_id,
            client_secret: encrypt!(&self.client_secret)?,
            auth_url: self.auth_url,
            token_url: self.token_url,
            redirect_uri: self.redirect_uri,
            scopes: self.scopes,
            extra_params: self.extra_params,
            enabled: self.enabled,
            use_proxy: self.use_proxy,
            grant_type: self.grant_type,
        })
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Object)]
pub struct OAuth2UpdateRequest {
    /// A description of what this configuration is used for.
    pub description: Option<String>,

    /// The client ID used for authenticating the application with the OAuth2 provider.
    pub client_id: Option<String>,

    /// The client secret used in conjunction with the client ID.
    pub client_secret: Option<String>,

    /// The URL to redirect users to for OAuth2 authorization.
    pub auth_url: Option<String>,

    /// The URL to exchange authorization codes for access tokens.
    pub token_url: Option<String>,

    /// The URI where the OAuth2 provider will redirect to after authorization.
    pub redirect_uri: Option<String>,

    /// The scopes of access that are being requested (e.g., email, profile).
    pub scopes: Option<Vec<String>>,

    /// Any additional parameters to include in the OAuth2 requests (e.g., access_type, prompt).
    pub extra_params: Option<BTreeMap<String, String>>,

    /// Indicates whether this configuration is enabled or disabled.
    pub enabled: Option<bool>,

    /// route OAuth through proxy (when direct access is blocked)
    pub use_proxy: Option<u64>,

    /// The grant type to use for authentication.
    pub grant_type: Option<OAuth2GrantType>,
}

fn apply_update(old: &OAuth2Model, request: OAuth2UpdateRequest) -> RustMailerResult<OAuth2Model> {
    let mut new = old.clone();
    if request.description.is_some() {
        new.description = request.description;
    }
    if let Some(client_id) = request.client_id {
        new.client_id = client_id;
    }
    if let Some(client_secret) = request.client_secret {
        new.client_secret = encrypt!(&client_secret)?;
    }
    if let Some(auth_url) = request.auth_url {
        new.auth_url = auth_url;
    }
    if let Some(token_url) = request.token_url {
        new.token_url = token_url;
    }
    if let Some(redirect_uri) = request.redirect_uri {
        new.redirect_uri = redirect_uri;
    }
    if request.scopes.is_some() {
        new.scopes = request.scopes;
    }
    if request.extra_params.is_some() {
        new.extra_params = request.extra_params;
    }
    if let Some(enabled) = request.enabled {
        new.enabled = enabled;
    }
    if let Some(use_proxy) = request.use_proxy {
        new.use_proxy = Some(use_proxy);
    }
    if let Some(grant_type) = request.grant_type {
        new.grant_type = grant_type;
    }
    new.updated_at = utc_now!();
    Ok(new)
}

impl From<OAuth2> for OAuth2V2 {
    fn from(value: OAuth2) -> Self {
        Self {
            id: value.id,
            description: value.description,
            client_id: value.client_id,
            client_secret: value.client_secret,
            auth_url: value.auth_url,
            token_url: value.token_url,
            redirect_uri: value.redirect_uri,
            scopes: value.scopes,
            extra_params: value.extra_params,
            enabled: value.enabled,
            use_proxy: value.use_proxy,
            grant_type: OAuth2GrantType::AuthorizationCode,
            created_at: value.created_at,
            updated_at: value.updated_at,
        }
    }
}

impl From<OAuth2V2> for OAuth2 {
    fn from(value: OAuth2V2) -> Self {
        Self {
            id: value.id,
            description: value.description,
            client_id: value.client_id,
            client_secret: value.client_secret,
            auth_url: value.auth_url,
            token_url: value.token_url,
            redirect_uri: value.redirect_uri,
            scopes: value.scopes,
            extra_params: value.extra_params,
            enabled: value.enabled,
            use_proxy: value.use_proxy,
            created_at: value.created_at,
            updated_at: value.updated_at,
        }
    }
}
