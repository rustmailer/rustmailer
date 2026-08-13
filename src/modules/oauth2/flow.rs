// Copyright © 2025-2026 rustmailer.com
// Licensed under RustMailer License Agreement v1.0
// Unauthorized copying, modification, or distribution is prohibited.

use crate::modules::error::code::ErrorCode;
use crate::modules::error::RustMailerResult;
use crate::modules::oauth2::entity::{OAuth2GrantType, OAuth2Model};
use crate::modules::oauth2::{pending::OAuth2PendingEntity, token::OAuth2AccessToken};
use crate::modules::settings::proxy::Proxy;
use crate::{decrypt, encrypt, raise_error};
use oauth2::{
    basic::BasicClient, AuthUrl, AuthorizationCode, ClientId, ClientSecret, CsrfToken,
    PkceCodeChallenge, PkceCodeVerifier, RedirectUrl, RefreshToken, Scope, TokenResponse, TokenUrl,
};
use poem_openapi::Object;
use serde::{Deserialize, Serialize};

pub type OAuth2Client = oauth2::Client<
    oauth2::StandardErrorResponse<oauth2::basic::BasicErrorResponseType>,
    oauth2::StandardTokenResponse<oauth2::EmptyExtraTokenFields, oauth2::basic::BasicTokenType>,
    oauth2::StandardTokenIntrospectionResponse<
        oauth2::EmptyExtraTokenFields,
        oauth2::basic::BasicTokenType,
    >,
    oauth2::StandardRevocableToken,
    oauth2::StandardErrorResponse<oauth2::RevocationErrorResponseType>,
    oauth2::EndpointSet,
    oauth2::EndpointNotSet,
    oauth2::EndpointNotSet,
    oauth2::EndpointNotSet,
    oauth2::EndpointSet,
>;

#[derive(Debug, Clone, Default, Eq, PartialEq, Serialize, Deserialize, Object)]
pub struct AuthorizeUrlRequest {
    /// The ID of the account for which the authorization URL is generated.
    pub account_id: u64,
    /// The name of the OAuth2 configuration to use for generating the authorization URL.
    pub oauth2_id: u64,
}

pub struct OAuth2Flow {
    pub oauth2_id: u64,
}

impl OAuth2Flow {
    pub fn new(oauth2_id: u64) -> Self {
        Self { oauth2_id }
    }

    pub async fn authorize_url(&self, account_id: u64) -> RustMailerResult<String> {
        let entity = self.fetch_oauth2_entity().await?;

        if !entity.enabled {
            return Err(raise_error!(
                format!(
                    "OAuth2 authentication is disabled for this client '{}'.",
                    self.oauth2_id
                ),
                ErrorCode::OAuth2ItemDisabled
            ));
        }

        let client = self.build_oauth2_client(&entity)?;
        let (pkce_code_challenge, pkce_code_verifier) = PkceCodeChallenge::new_random_sha256();

        let mut request = client
            .authorize_url(CsrfToken::new_random)
            .set_pkce_challenge(pkce_code_challenge)
            .add_scopes(
                entity
                    .scopes
                    .unwrap_or(Vec::new())
                    .into_iter()
                    .map(Scope::new),
            );

        if let Some(extra_params) = &entity.extra_params {
            for (name, value) in extra_params {
                request = request.add_extra_param(name.clone(), value.clone());
            }
        }

        let (authorize_url, csrf_state) = request.url();
        self.save_pending_oauth2_state(
            account_id,
            csrf_state.secret(),
            pkce_code_verifier.secret(),
        )
        .await?;
        Ok(authorize_url.to_string())
    }

    pub async fn fetch_save_access_token(
        &self,
        account_id: u64,
        code_verifier: &str,
        code: &str,
    ) -> RustMailerResult<()> {
        let entity = self.fetch_oauth2_entity().await?;
        let client = self.build_oauth2_client(&entity)?;
        let http_client = build_http_client(entity.use_proxy).await?;

        let token_response = client
            .exchange_code(AuthorizationCode::new(code.to_owned()))
            .set_pkce_verifier(PkceCodeVerifier::new(code_verifier.to_owned()))
            .request_async(&http_client)
            .await
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::HttpResponseError))?;

        let access_token = token_response.access_token().secret().to_owned();
        let refresh_token = token_response
            .refresh_token()
            .ok_or_else(|| {
                raise_error!(
                    "Missing refresh token in the token response.".into(),
                    ErrorCode::MissingRefreshToken
                )
            })?
            .secret()
            .to_owned();

        self.save_oauth2_entity(account_id, access_token, refresh_token)
            .await?;

        Ok(())
    }

    /// Exchanges client credentials for an access token and stores it.
    /// Suitable for grant_type = ClientCredentials (e.g. Microsoft Graph v2).
    pub async fn exchange_client_credentials(&self, account_id: u64) -> RustMailerResult<()> {
        let entity = self.fetch_oauth2_entity().await?;

        if !matches!(entity.grant_type, OAuth2GrantType::ClientCredentials) {
            return Err(raise_error!(
                format!(
                    "OAuth2 configuration id={} does not use ClientCredentials grant type.",
                    self.oauth2_id
                ),
                ErrorCode::InvalidParameter
            ));
        }

        if !entity.enabled {
            return Err(raise_error!(
                format!(
                    "OAuth2 authentication is disabled for this client '{}'.",
                    self.oauth2_id
                ),
                ErrorCode::OAuth2ItemDisabled
            ));
        }

        let client = self.build_oauth2_client(&entity)?;
        let http_client = build_http_client(entity.use_proxy).await?;

        let scopes: Vec<Scope> = entity
            .scopes
            .clone()
            .unwrap_or_default()
            .into_iter()
            .map(Scope::new)
            .collect();

        let token_response = client
            .exchange_client_credentials()
            .add_scopes(scopes)
            .request_async(&http_client)
            .await
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::HttpResponseError))?;

        let access_token = token_response.access_token().secret().to_owned();

        self.save_oauth2_entity_creds(account_id, access_token)
            .await?;

        Ok(())
    }

    async fn save_oauth2_entity(
        &self,
        account_id: u64,
        access_token: String,
        refresh_token: String,
    ) -> RustMailerResult<()> {
        let token =
            OAuth2AccessToken::create(account_id, self.oauth2_id, access_token, refresh_token)?;
        token.save_or_update().await
    }

    async fn save_oauth2_entity_creds(
        &self,
        account_id: u64,
        access_token: String,
    ) -> RustMailerResult<()> {
        let token = OAuth2AccessToken::create_creds(account_id, self.oauth2_id, access_token)?;
        token.save_or_update().await
    }

    async fn update_oauth2_entity(
        &self,
        account_id: u64,
        access_token: String,
        refresh_token: String,
    ) -> RustMailerResult<()> {
        OAuth2AccessToken::set_access_token(
            account_id,
            encrypt!(&access_token)?,
            encrypt!(&refresh_token)?,
        )
        .await
    }

    pub async fn refresh_access_token(&self, token: &OAuth2AccessToken) -> RustMailerResult<()> {
        let entity = self.fetch_oauth2_entity().await?;
        if !entity.enabled {
            OAuth2AccessToken::delete_by_oauth2_id(token.oauth2_id).await?;
            return Err(raise_error!(
                "OAuth2 authentication is disabled for this client".into(),
                ErrorCode::OAuth2ItemDisabled
            ));
        }

        match &entity.grant_type {
            crate::modules::oauth2::entity::OAuth2GrantType::ClientCredentials => {
                self.exchange_client_credentials(token.account_id).await
            }
            crate::modules::oauth2::entity::OAuth2GrantType::AuthorizationCode => {
                self.refresh_authorization_code_token(&entity, token).await
            }
        }
    }

    async fn refresh_authorization_code_token(
        &self,
        entity: &OAuth2Model,
        token: &OAuth2AccessToken,
    ) -> RustMailerResult<()> {
        let client = self.build_oauth2_client(entity)?;
        let http_client = build_http_client(entity.use_proxy).await?;

        let refresh_token = token.refresh_token.clone().ok_or_else(|| {
            raise_error!(
                "refresh token is null".into(),
                ErrorCode::MissingRefreshToken
            )
        })?;

        let refresh_response = client
            .exchange_refresh_token(&RefreshToken::new(refresh_token.clone()))
            .add_scopes(
                entity
                    .scopes
                    .clone()
                    .unwrap_or_default()
                    .into_iter()
                    .map(Scope::new),
            )
            .request_async(&http_client)
            .await
            .map_err(|e| {
                raise_error!(
                    format!(
                        "Failed to retrieve refresh token response: {}",
                        e.to_string()
                    ),
                    ErrorCode::HttpResponseError
                )
            })?;

        let access_token = refresh_response.access_token().secret().to_owned();
        let new_refresh_token = refresh_response
            .refresh_token()
            .map(|r| r.secret().to_owned())
            .unwrap_or_else(|| refresh_token.clone());
        self.update_oauth2_entity(token.account_id, access_token, new_refresh_token)
            .await?;

        Ok(())
    }

    // Helper function to fetch the OAuth2 entity
    async fn fetch_oauth2_entity(&self) -> RustMailerResult<OAuth2Model> {
        OAuth2Model::get(self.oauth2_id).await?.ok_or_else(|| {
            raise_error!(
                format!("OAuth2 entity with id '{}' not found", self.oauth2_id),
                ErrorCode::ResourceNotFound
            )
        })
    }

    // Helper function to build the OAuth2 client
    fn build_oauth2_client(&self, entity: &OAuth2Model) -> RustMailerResult<OAuth2Client> {
        let auth_url = AuthUrl::new(entity.auth_url.clone())
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InvalidParameter))?;
        let token_url = TokenUrl::new(entity.token_url.clone())
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InvalidParameter))?;

        // Client credentials flow does not use a redirect URI; `oauth2` crate still
        // requires one on the client builder, so use a placeholder that is never sent.
        let redirect_uri_str = if entity.redirect_uri.trim().is_empty() {
            "http://localhost/oauth2/callback"
        } else {
            entity.redirect_uri.as_str()
        };
        let redirect_uri = RedirectUrl::new(redirect_uri_str.to_owned())
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InvalidParameter))?;

        let client = BasicClient::new(ClientId::new(entity.client_id.clone()))
            .set_client_secret(ClientSecret::new(decrypt!(&entity.client_secret)?))
            .set_auth_uri(auth_url)
            .set_token_uri(token_url)
            .set_redirect_uri(redirect_uri);

        Ok(client)
    }

    // Helper function to save the pending OAuth2 state
    async fn save_pending_oauth2_state(
        &self,
        account_id: u64,
        csrf_state: &str,
        pkce_code_verifier: &str,
    ) -> RustMailerResult<()> {
        OAuth2PendingEntity::new(
            self.oauth2_id,
            account_id,
            csrf_state.to_owned(),
            pkce_code_verifier.to_owned(),
        )
        .save()
        .await
    }
}

// Helper function to build the HTTP client
async fn build_http_client(use_proxy: Option<u64>) -> RustMailerResult<reqwest::Client> {
    if let Some(proxy_id) = use_proxy {
        let proxy = Proxy::get(proxy_id).await?;
        return oauth2::reqwest::ClientBuilder::new()
            .redirect(oauth2::reqwest::redirect::Policy::none())
            .proxy(reqwest::Proxy::all(&proxy.url).map_err(|e| {
                raise_error!(
                    format!(
                        "Failed to configure SOCKS5 proxy ({}): {:#?}. Please check",
                        &proxy.url, e
                    ),
                    ErrorCode::InternalError
                )
            })?)
            .build()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError));
    }
    oauth2::reqwest::ClientBuilder::new()
        .redirect(oauth2::reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))
}
