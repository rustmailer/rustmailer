// Copyright © 2025-2026 rustmailer.com
// Licensed under RustMailer License Agreement v1.0
// Unauthorized copying, modification, or distribution is prohibited.

use crate::modules::common::auth::ClientContext;
use crate::modules::common::paginated::paginate_vec;
use crate::modules::error::code::ErrorCode;
use crate::modules::grpc::auth::require_account_access;
use crate::modules::message::append::ReplyDraft as RustMailerReplyDraft;
use crate::modules::rest::response::DataPage;
use crate::modules::scheduler::model::TaskStatus;
use crate::modules::smtp::queue::message::SendEmailTask as RustMailerQueuedEmailTask;
use crate::modules::smtp::request::forward::ForwardEmailRequest as RustMailerForwardEmailRequest;
use crate::modules::smtp::request::new::SendDraftRequest as RustMailerSendDraftRequest;
use crate::modules::smtp::request::new::SendEmailRequest as RustMailerSendEmailRequest;
use crate::modules::smtp::request::reply::ReplyEmailRequest as RustMailerReplyEmailRequest;
use crate::modules::tasks::queue::RustMailerTaskQueue;
use crate::modules::{
    grpc::service::rustmailer_grpc::{
        EmailTask, Empty, ForwardMailRequest, GetTaskRequest, ListTasksRequest, PagedEmailTask,
        RemoveTaskRequest, ReplyDraft, ReplyMailRequest, SaveDraftRequest, SendDraftRequest,
        SendMailService, SendNewMailRequest,
    },
    smtp::request::builder::EmailBuilder,
};
use crate::raise_error;
use poem_grpc::{Request, Response, Status};
use std::collections::BTreeSet;
use std::sync::Arc;

pub mod from;

#[derive(Default)]
pub struct RustMailerSendMailService;

impl SendMailService for RustMailerSendMailService {
    async fn send_new_mail(
        &self,
        request: Request<SendNewMailRequest>,
    ) -> Result<Response<Empty>, Status> {
        let req = require_account_access(request, |r| r.account_id)?;
        let email_request: RustMailerSendEmailRequest = req
            .request
            .ok_or_else(|| {
                raise_error!(
                    "'SendEmailRequest' must be set".into(),
                    ErrorCode::InvalidParameter
                )
            })?
            .try_into()
            .map_err(|e: &'static str| raise_error!(e.to_string(), ErrorCode::InvalidParameter))?;

        email_request.build(req.account_id).await?;
        Ok(Response::new(Empty {}))
    }

    async fn reply_mail(
        &self,
        request: Request<ReplyMailRequest>,
    ) -> Result<Response<Empty>, Status> {
        let req = require_account_access(request, |r| r.account_id)?;
        let email_request: RustMailerReplyEmailRequest = req
            .request
            .ok_or_else(|| {
                raise_error!(
                    "'ReplyEmailRequest' must be set".into(),
                    ErrorCode::InvalidParameter
                )
            })?
            .try_into()
            .map_err(|e: &'static str| raise_error!(e.to_string(), ErrorCode::InvalidParameter))?;
        email_request.build(req.account_id).await?;
        Ok(Response::new(Empty::default()))
    }

    async fn forward_mail(
        &self,
        request: Request<ForwardMailRequest>,
    ) -> Result<Response<Empty>, Status> {
        let req = require_account_access(request, |r| r.account_id)?;
        let email_request: RustMailerForwardEmailRequest = req
            .request
            .ok_or_else(|| {
                raise_error!(
                    "'ForwardEmailRequest' must be set".into(),
                    ErrorCode::InvalidParameter
                )
            })?
            .try_into()
            .map_err(|e: &'static str| raise_error!(e.to_string(), ErrorCode::InvalidParameter))?;
        email_request.build(req.account_id).await?;
        Ok(Response::new(Empty::default()))
    }

    async fn save_draft(
        &self,
        request: Request<SaveDraftRequest>,
    ) -> Result<Response<ReplyDraft>, Status> {
        let req = require_account_access(request, |r| r.account_id)?;
        let email_request: RustMailerSendEmailRequest = req
            .request
            .ok_or_else(|| {
                raise_error!(
                    "'SendEmailRequest' must be set".into(),
                    ErrorCode::InvalidParameter
                )
            })?
            .try_into()
            .map_err(|e: &'static str| raise_error!(e.to_string(), ErrorCode::InvalidParameter))?;

        let draft: RustMailerReplyDraft = email_request.save_as_draft(req.account_id).await?;
        Ok(Response::new(draft.into()))
    }

    async fn send_draft(
        &self,
        request: Request<SendDraftRequest>,
    ) -> Result<Response<Empty>, Status> {
        let req = require_account_access(request, |r| r.account_id)?;
        let draft_request = RustMailerSendDraftRequest { id: req.id };
        draft_request.send_draft(req.account_id).await?;
        Ok(Response::new(Empty::default()))
    }

    async fn list_email_tasks(
        &self,
        request: Request<ListTasksRequest>,
    ) -> Result<Response<PagedEmailTask>, Status> {
        let extensions = request.extensions().clone();
        let req = request.into_inner();

        // Get ClientContext from cloned extensions
        let context = extensions.get::<Arc<ClientContext>>().ok_or_else(|| {
            raise_error!("Missing ClientContext".into(), ErrorCode::InternalError)
        })?;

        let accessible_accounts = context.accessible_accounts()?;
        let send_queue = RustMailerTaskQueue::get().unwrap();
        let status = req
            .status
            .map(|i| TaskStatus::try_from(i))
            .transpose()
            .map_err(|e: &'static str| raise_error!(e.to_string(), ErrorCode::InvalidParameter))?;
        if accessible_accounts.is_none() {
            let result = match status {
                Some(status) => {
                    send_queue
                        .list_paginated_email_tasks_by_status(
                            req.page,
                            req.page_size,
                            req.desc,
                            status,
                        )
                        .await?
                }
                None => {
                    send_queue
                        .list_paginated_email_tasks(req.page, req.page_size, req.desc)
                        .await?
                }
            };
            return Ok(Response::new(result.into()));
        }

        let all_tasks = match status {
            Some(status) => send_queue.list_email_tasks_by_status(status).await?,
            None => send_queue.list_all_email_tasks().await?,
        };

        let allowed_ids: BTreeSet<u64> =
            accessible_accounts.unwrap().iter().map(|a| a.id).collect();

        let mut filtered_tasks: Vec<RustMailerQueuedEmailTask> = all_tasks
            .into_iter()
            .filter(|acct| allowed_ids.contains(&acct.account_id))
            .collect();

        let sort_desc = req.desc.unwrap_or(true);
        filtered_tasks.sort_by(|a, b| {
            if sort_desc {
                b.created_at.cmp(&a.created_at)
            } else {
                a.created_at.cmp(&b.created_at)
            }
        });
        let page_data =
            paginate_vec(&filtered_tasks, req.page, req.page_size).map(DataPage::from)?;

        Ok(Response::new(page_data.into()))
    }

    async fn get_email_task(
        &self,
        request: Request<GetTaskRequest>,
    ) -> Result<Response<EmailTask>, Status> {
        let extensions = request.extensions().clone();
        let req = request.into_inner();

        // Get ClientContext from cloned extensions
        let context = extensions.get::<Arc<ClientContext>>().ok_or_else(|| {
            raise_error!("Missing ClientContext".into(), ErrorCode::InternalError)
        })?;
        let send_queue = RustMailerTaskQueue::get().unwrap();
        let result = send_queue
            .get_email_task(req.id)
            .await?
            .ok_or_else(|| raise_error!("task not found".into(), ErrorCode::ResourceNotFound))?;
        // Check account access
        context.require_account_access(result.account_id)?;
        Ok(Response::new(result.into()))
    }

    async fn remove_email_task(
        &self,
        request: Request<RemoveTaskRequest>,
    ) -> Result<Response<Empty>, Status> {
        let extensions = request.extensions().clone();
        let req = request.into_inner();

        // Get ClientContext from cloned extensions
        let context = extensions.get::<Arc<ClientContext>>().ok_or_else(|| {
            raise_error!("Missing ClientContext".into(), ErrorCode::InternalError)
        })?;
        let send_queue = RustMailerTaskQueue::get().unwrap();
        let result = send_queue
            .get_email_task(req.id)
            .await?
            .ok_or_else(|| raise_error!("task not found".into(), ErrorCode::ResourceNotFound))?;

        // Check account access
        context.require_account_access(result.account_id)?;
        send_queue.remove_task(req.id).await?;
        Ok(Response::new(Empty::default()))
    }
}
