// Copyright © 2025-2026 rustmailer.com
// Licensed under RustMailer License Agreement v1.0
// Unauthorized copying, modification, or distribution is prohibited.

use crate::modules::account::migration::AccountModel as RustMailerAccount;
use crate::modules::account::payload::filter_accessible_accounts;
use crate::modules::account::payload::MinimalAccount;
use crate::modules::account::payload::AccountCreateRequest as RustMailerAccountCreateRequest;
use crate::modules::account::payload::AccountUpdateRequest as RustMailerAccountUpdateRequest;
use crate::modules::account::status::AccountRunningState as RustMailerAccountRunningState;
use crate::modules::common::auth::ClientContext;
use crate::modules::common::paginated::paginate_vec;
use crate::modules::context::controller::SYNC_CONTROLLER;
use crate::modules::error::code::ErrorCode;
use crate::modules::grpc::auth::require_account_access;
use crate::modules::grpc::service::rustmailer_grpc::AccountService;
use crate::modules::grpc::service::rustmailer_grpc::ListMinimalAccountsResponse;
use crate::modules::grpc::service::rustmailer_grpc::{
    Account, AccountCreateRequest, AccountId, AccountRunningState, AccountUpdateRequest, Empty,
    PagedAccount, PaginateRequest,
};
use crate::modules::rest::response::DataPage;
use crate::modules::token::AccessToken;
use crate::modules::token::AccountInfo;
use crate::raise_error;
use poem_grpc::{Request, Response, Status};
use std::collections::BTreeSet;
use std::sync::Arc;

mod from;

#[derive(Default)]
pub struct RustMailerAccountService;

impl AccountService for RustMailerAccountService {
    async fn get_account(&self, request: Request<AccountId>) -> Result<Response<Account>, Status> {
        let req = require_account_access(request, |r| r.account_id)?;
        let account = RustMailerAccount::get(req.account_id).await?;
        Ok(Response::new(account.into()))
    }

    async fn remove_account(&self, request: Request<AccountId>) -> Result<Response<Empty>, Status> {
        let req = require_account_access(request, |r| r.account_id)?;
        RustMailerAccount::delete(req.account_id).await?;
        Ok(Response::new(Empty::default()))
    }

    async fn create_account(
        &self,
        request: Request<AccountCreateRequest>,
    ) -> Result<Response<Account>, Status> {
        let extensions = request.extensions().clone();
        let req = request.into_inner();

        // Get ClientContext from cloned extensions
        let context = extensions.get::<Arc<ClientContext>>().ok_or_else(|| {
            raise_error!("Missing ClientContext".into(), ErrorCode::InternalError)
        })?;

        let request = RustMailerAccountCreateRequest::try_from(req)
            .map_err(|e| raise_error!(e.to_string(), ErrorCode::InvalidParameter))?;
        let entity = request.create_entity()?;
        entity.clone().save().await?;

        if let Some(access_token) = &context.access_token {
            let account_info = AccountInfo {
                id: entity.id.clone(),
                email: entity.email.clone(),
            };

            AccessToken::grant_account_access(&access_token.token, account_info).await?;
        }

        SYNC_CONTROLLER
            .trigger_start(entity.id.clone(), entity.email.clone())
            .await;
        Ok(Response::new(entity.into()))
    }

    async fn update_account(
        &self,
        request: Request<AccountUpdateRequest>,
    ) -> Result<Response<Empty>, Status> {
        let req = require_account_access(request, |r| r.account_id)?;
        let account_id = req.account_id;
        let request = RustMailerAccountUpdateRequest::try_from(req)
            .map_err(|e| raise_error!(e.to_string(), ErrorCode::InvalidParameter))?;
        request.validate_update_request()?;

        RustMailerAccount::update(account_id, request, false).await?;
        Ok(Response::new(Empty::default()))
    }

    async fn list_accounts(
        &self,
        request: Request<PaginateRequest>,
    ) -> Result<Response<PagedAccount>, Status> {
        let extensions = request.extensions().clone();
        let req = request.into_inner();

        // Get ClientContext from cloned extensions
        let context = extensions.get::<Arc<ClientContext>>().ok_or_else(|| {
            raise_error!("Missing ClientContext".into(), ErrorCode::InternalError)
        })?;

        let accessible_accounts = context.accessible_accounts()?;
        if accessible_accounts.is_none() {
            let data = RustMailerAccount::paginate_list(req.page, req.page_size, req.desc).await?;
            return Ok(Response::new(PagedAccount {
                current_page: data.current_page,
                page_size: data.page_size,
                total_items: data.total_items,
                items: data.items.into_iter().map(Into::into).collect(),
                total_pages: data.total_pages,
            }));
        }

        let all_accounts = RustMailerAccount::list_all().await?;

        let allowed_ids: BTreeSet<u64> =
            accessible_accounts.unwrap().iter().map(|a| a.id).collect();

        let mut filtered_accounts: Vec<RustMailerAccount> = all_accounts
            .into_iter()
            .filter(|acct| allowed_ids.contains(&acct.id))
            .collect();

        let sort_desc = req.desc.unwrap_or(true);
        filtered_accounts.sort_by(|a, b| {
            if sort_desc {
                b.created_at.cmp(&a.created_at)
            } else {
                a.created_at.cmp(&b.created_at)
            }
        });
        let page_data =
            paginate_vec(&filtered_accounts, req.page, req.page_size).map(DataPage::from)?;

        Ok(Response::new(PagedAccount {
            current_page: page_data.current_page,
            page_size: page_data.page_size,
            total_items: page_data.total_items,
            items: page_data.items.into_iter().map(Into::into).collect(),
            total_pages: page_data.total_pages,
        }))
    }

    async fn get_account_state(
        &self,
        request: Request<AccountId>,
    ) -> Result<Response<AccountRunningState>, Status> {
        let req = require_account_access(request, |r| r.account_id)?;
        let result = RustMailerAccountRunningState::get(req.account_id).await?;
        let result = result.ok_or_else(|| {
            raise_error!(
                "account running state is not found".into(),
                ErrorCode::ResourceNotFound
            )
        })?;
        Ok(Response::new(result.into()))
    }

    async fn list_minimal_accounts(
        &self,
        request: Request<Empty>,
    ) -> Result<Response<ListMinimalAccountsResponse>, Status> {
        let extensions = request.extensions();
        let context = extensions.get::<Arc<ClientContext>>().ok_or_else(|| {
            raise_error!("Missing ClientContext".into(), ErrorCode::InternalError)
        })?;

        let accessible_accounts = context.accessible_accounts()?;
        let minimal_list: Vec<MinimalAccount> = RustMailerAccount::list_all()
            .await?
            .into_iter()
            .filter(|a| a.enabled)
            .map(|account| MinimalAccount {
                id: account.id,
                email: account.email,
                mailer_type: account.mailer_type,
            })
            .collect();

        let result = match accessible_accounts {
            Some(set) => filter_accessible_accounts(&minimal_list, set),
            None => minimal_list,
        };
        Ok(Response::new(ListMinimalAccountsResponse {
            accounts: result.into_iter().map(Into::into).collect(),
        }))
    }
}
