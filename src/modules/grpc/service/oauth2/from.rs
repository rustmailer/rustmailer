// Copyright © 2025-2026 rustmailer.com
// Licensed under RustMailer License Agreement v1.0
// Unauthorized copying, modification, or distribution is prohibited.

use crate::modules::{
    grpc::service::rustmailer_grpc::{
        self, ClientCredentialsRequest, OAuth2GrantType as GrpcOAuth2GrantType, PagedOAuth2,
    },
    oauth2::{
        entity::{OAuth2CreateRequest, OAuth2GrantType, OAuth2Model, OAuth2UpdateRequest},
        token::{ExternalOAuth2Request, OAuth2AccessToken},
    },
    rest::response::DataPage,
};

fn grpc_grant_type_to_domain(v: i32) -> OAuth2GrantType {
    match GrpcOAuth2GrantType::try_from(v) {
        Ok(GrpcOAuth2GrantType::ClientCredentials) => OAuth2GrantType::ClientCredentials,
        _ => OAuth2GrantType::AuthorizationCode,
    }
}

fn grpc_grant_type_to_optional(v: Option<i32>) -> Option<OAuth2GrantType> {
    match v {
        Some(code) => Some(grpc_grant_type_to_domain(code)),
        None => None,
    }
}

impl From<rustmailer_grpc::OAuth2CreateRequest> for OAuth2CreateRequest {
    fn from(value: rustmailer_grpc::OAuth2CreateRequest) -> Self {
        Self {
            description: value.description,
            client_id: value.client_id,
            client_secret: value.client_secret,
            auth_url: value.auth_url,
            token_url: value.token_url,
            redirect_uri: value.redirect_uri,
            scopes: (!value.scopes.is_empty()).then_some(value.scopes),
            extra_params: (!value.extra_params.is_empty())
                .then(|| value.extra_params.into_iter().collect()),
            enabled: value.enabled,
            use_proxy: value.use_proxy,
            grant_type: grpc_grant_type_to_domain(value.grant_type),
        }
    }
}

impl From<rustmailer_grpc::UpdateOAuth2Request> for OAuth2UpdateRequest {
    fn from(value: rustmailer_grpc::UpdateOAuth2Request) -> Self {
        Self {
            description: value.description,
            client_id: value.client_id,
            client_secret: value.client_secret,
            auth_url: value.auth_url,
            token_url: value.token_url,
            redirect_uri: value.redirect_uri,
            scopes: (!value.scopes.is_empty()).then_some(value.scopes),
            extra_params: (!value.extra_params.is_empty())
                .then(|| value.extra_params.into_iter().collect()),
            enabled: value.enabled,
            use_proxy: value.use_proxy,
            grant_type: grpc_grant_type_to_optional(value.grant_type),
        }
    }
}

impl From<OAuth2Model> for rustmailer_grpc::OAuth2 {
    fn from(value: OAuth2Model) -> Self {
        Self {
            id: value.id,
            description: value.description,
            client_id: value.client_id,
            client_secret: value.client_secret,
            auth_url: value.auth_url,
            token_url: value.token_url,
            redirect_uri: value.redirect_uri,
            scopes: value.scopes.unwrap_or_default(),
            extra_params: value
                .extra_params
                .map(|p| p.into_iter().collect())
                .unwrap_or_default(),
            enabled: value.enabled,
            use_proxy: value.use_proxy,
            created_at: value.created_at,
            updated_at: value.updated_at,
            grant_type: match value.grant_type {
                OAuth2GrantType::ClientCredentials => GrpcOAuth2GrantType::ClientCredentials as i32,
                _ => GrpcOAuth2GrantType::AuthorizationCode as i32,
            },
        }
    }
}

impl From<DataPage<OAuth2Model>> for PagedOAuth2 {
    fn from(value: DataPage<OAuth2Model>) -> Self {
        Self {
            current_page: value.current_page,
            page_size: value.page_size,
            total_items: value.total_items,
            items: value.items.into_iter().map(Into::into).collect(),
            total_pages: value.total_pages,
        }
    }
}

impl From<OAuth2AccessToken> for rustmailer_grpc::OAuth2AccessToken {
    fn from(value: OAuth2AccessToken) -> Self {
        Self {
            account_id: value.account_id,
            oauth2_id: value.oauth2_id,
            access_token: value.access_token,
            refresh_token: value.refresh_token,
            created_at: value.created_at,
            updated_at: value.updated_at,
        }
    }
}

impl From<rustmailer_grpc::ExternalOAuth2Request> for ExternalOAuth2Request {
    fn from(value: rustmailer_grpc::ExternalOAuth2Request) -> Self {
        Self {
            oauth2_id: value.oauth2_id,
            access_token: value.access_token,
            refresh_token: value.refresh_token,
        }
    }
}

impl From<ClientCredentialsRequest> for (u64, u64) {
    fn from(value: ClientCredentialsRequest) -> Self {
        (value.account_id, value.oauth2_id)
    }
}
