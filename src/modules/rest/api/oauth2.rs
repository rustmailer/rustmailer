// Copyright © 2025-2026 rustmailer.com
// Licensed under RustMailer License Agreement v1.0
// Unauthorized copying, modification, or distribution is prohibited.

use crate::modules::common::auth::ClientContext;
use crate::modules::error::code::ErrorCode;
use crate::modules::oauth2::entity::{OAuth2CreateRequest, OAuth2Model, OAuth2UpdateRequest};
use crate::modules::oauth2::flow::{AuthorizeUrlRequest, OAuth2Flow};
use crate::modules::oauth2::token::{ExternalOAuth2Request, OAuth2AccessToken};
use crate::modules::rest::api::ApiTags;
use crate::modules::rest::response::DataPage;
use crate::modules::rest::ApiResult;
use crate::raise_error;
use poem::web::Path;
use poem_openapi::{
    param::Query,
    payload::{Json, PlainText},
    Object, OpenApi,
};
use serde::{Deserialize, Serialize};

pub struct OAuth2Api;

#[OpenApi(prefix_path = "/api/v1", tag = "ApiTags::OAuth2")]
impl OAuth2Api {
    /// Retrieves the OAuth2 configuration for a specified name.
    /// Requires root privileges.
    /// This endpoint fetches the OAuth2 configuration identified by the given name.
    #[oai(
        path = "/oauth2/:id",
        method = "get",
        operation_id = "get_oauth2_config"
    )]
    async fn get_oauth2_config(
        &self,
        /// The name of the OAuth2 configuration to retrieve
        id: Path<u64>,
        context: ClientContext,
    ) -> ApiResult<Json<OAuth2Model>> {
        context.require_root()?;
        let id = id.0;
        Ok(Json(OAuth2Model::get(id).await?.ok_or_else(|| {
            raise_error!(
                format!("OAuth2 configuration id='{id}' not found"),
                ErrorCode::ResourceNotFound
            )
        })?))
    }

    /// Deletes an OAuth2 configuration by name.
    /// Requires root privileges.
    /// This endpoint removes the OAuth2 configuration identified by the specified name.
    #[oai(
        path = "/oauth2/:id",
        method = "delete",
        operation_id = "remove_oauth2_config"
    )]
    async fn remove_oauth2_config(
        &self,
        /// The name of the OAuth2 configuration to retrieve
        id: Path<u64>,
        context: ClientContext,
    ) -> ApiResult<()> {
        context.require_root()?;
        Ok(OAuth2Model::delete(id.0).await?)
    }

    /// Creates a new OAuth2 configuration.
    /// Requires root privileges.
    /// This endpoint creates a new OAuth2 configuration based on the provided request data.
    #[oai(
        path = "/oauth2",
        method = "post",
        operation_id = "create_oauth2_config"
    )]
    async fn create_oauth2_config(
        &self,
        /// A JSON payload containing the details for the new OAuth2 configuration
        request: Json<OAuth2CreateRequest>,
        context: ClientContext,
    ) -> ApiResult<()> {
        context.require_root()?;
        let entity = OAuth2Model::new(request.0)?;
        Ok(entity.save().await?)
    }

    /// Updates an existing OAuth2 configuration.
    /// Requires root privileges.
    /// This endpoint updates the OAuth2 configuration identified by the specified name
    #[oai(
        path = "/oauth2/:id",
        method = "post",
        operation_id = "update_oauth2_config"
    )]
    async fn update_oauth2_config(
        &self,
        /// The name of the OAuth2 configuration to update
        id: Path<u64>,
        /// A JSON payload containing the updated configuration details
        payload: Json<OAuth2UpdateRequest>,
        context: ClientContext,
    ) -> ApiResult<()> {
        context.require_root()?;
        Ok(OAuth2Model::update(id.0, payload.0).await?)
    }

    /// Lists OAuth2 configurations with pagination and sorting options.
    /// This endpoint retrieves a paginated list of OAuth2 configurations, allowing for
    /// optional pagination and sorting parameters. It requires root access.
    #[oai(
        path = "/oauth2-list",
        method = "get",
        operation_id = "list_oauth2_config"
    )]
    async fn list_oauth2_config(
        &self,
        /// Optional. The page number to retrieve (starting from 1).
        page: Query<Option<u64>>,
        /// Optional. The number of items per page.
        page_size: Query<Option<u64>>,
        /// Optional. Whether to sort the list in descending order.
        desc: Query<Option<bool>>,
        context: ClientContext,
    ) -> ApiResult<Json<DataPage<OAuth2Model>>> {
        context.require_root()?;
        Ok(Json(
            OAuth2Model::paginate_list(page.0, page_size.0, desc.0).await?,
        ))
    }

    /// Generates an OAuth2 authorization URL for a specific account.
    /// This endpoint creates an authorization URL for the specified OAuth2 configuration
    /// and account ID. It requires root access and returns the URL as plain text.
    #[oai(
        path = "/oauth2-authorize-url",
        method = "post",
        operation_id = "create_oauth2_authorize_url"
    )]
    async fn create_oauth2_authorize_url(
        &self,
        /// A JSON payload containing the OAuth2 configuration name and account ID.
        request: Json<AuthorizeUrlRequest>,
        context: ClientContext,
    ) -> ApiResult<PlainText<String>> {
        context.require_root()?;
        let request = request.0;
        let flow = OAuth2Flow::new(request.oauth2_id);
        Ok(PlainText(flow.authorize_url(request.account_id).await?))
    }

    /// Retrieves OAuth2 access tokens for a specified account.
    /// This endpoint fetches the OAuth2 access tokens associated with the given account ID.
    #[oai(
        path = "/oauth2-tokens/:account_id",
        method = "get",
        operation_id = "get_oauth2_tokens"
    )]
    async fn get_oauth2_tokens(
        &self,
        /// The ID of the account to retrieve access tokens for
        account_id: Path<u64>,
        context: ClientContext,
    ) -> ApiResult<Json<OAuth2AccessToken>> {
        let account = account_id.0;
        context.require_account_access(account)?;
        Ok(Json(OAuth2AccessToken::get(account).await?.ok_or_else(
            || {
                raise_error!(
                    "OAuth2 access tokens not found".into(),
                    ErrorCode::ResourceNotFound
                )
            },
        )?))
    }

    /// Configures an external OAuth2 token for a specified account.
    /// This endpoint allows two usage modes:
    /// 1. If only an `access_token` is provided, RustMailer will store it directly.
    ///    - In this mode, RustMailer **cannot refresh** the token, since it has no
    ///      associated OAuth2 configuration or refresh token.
    ///    - The caller is responsible for periodically updating the access token
    ///      by calling this endpoint again.
    /// 2. If both `oauth2_id` and `refresh_token` are provided, it means the external
    ///    OAuth2 authorization flow has been completed outside RustMailer.
    ///    - Since the OAuth2 configuration (including client_id and client_secret)
    ///      is already stored in RustMailer, the service can use the refresh token
    ///      to obtain new access tokens automatically.
    /// Note: The `oauth2_id` must reference a valid OAuth2 configuration
    /// already created in RustMailer.
    #[oai(
        path = "/store-external-oauth2-token/:account_id",
        method = "post",
        operation_id = "store_external_oauth2_token"
    )]
    async fn store_external_oauth2_token(
        &self,
        account_id: Path<u64>,
        request: Json<ExternalOAuth2Request>,
        context: ClientContext,
    ) -> ApiResult<()> {
        let account = account_id.0;
        // Check account access permissions
        context.require_account_access(account)?;
        OAuth2AccessToken::upsert_external_oauth_token(account, request.0).await?;

        Ok(())
    }

    /// Exchanges client credentials for an access token.
    /// Only applicable for OAuth2 configurations with grant_type = CLIENT_CREDENTIALS.
    #[oai(
        path = "/oauth2-exchange-client-credentials",
        method = "post",
        operation_id = "exchange_client_credentials"
    )]
    async fn exchange_client_credentials(
        &self,
        request: Json<ExchangeClientCredentialsRequest>,
        context: ClientContext,
    ) -> ApiResult<()> {
        context.require_account_access(request.account_id)?;
        let flow = OAuth2Flow::new(request.oauth2_id);
        flow.exchange_client_credentials(request.account_id).await?;
        Ok(())
    }

    /// Refreshes the OAuth2 access token for a specified account.
    /// Uses the refresh token for AuthorizationCode configs, or re-exchanges
    /// client credentials for ClientCredentials configs.
    #[oai(
        path = "/oauth2-tokens/:account_id/refresh",
        method = "post",
        operation_id = "refresh_oauth2_token"
    )]
    async fn refresh_oauth2_token(
        &self,
        account_id: Path<u64>,
        context: ClientContext,
    ) -> ApiResult<()> {
        let account = account_id.0;
        context.require_account_access(account)?;
        let token = OAuth2AccessToken::get(account).await?.ok_or_else(|| {
            raise_error!(
                "OAuth2 access tokens not found".into(),
                ErrorCode::ResourceNotFound
            )
        })?;
        let flow = OAuth2Flow::new(token.oauth2_id);
        flow.refresh_access_token(&token).await?;
        Ok(())
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Object)]
pub struct ExchangeClientCredentialsRequest {
    pub account_id: u64,
    pub oauth2_id: u64,
}
