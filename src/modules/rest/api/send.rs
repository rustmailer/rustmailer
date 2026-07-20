// Copyright © 2025-2026 rustmailer.com
// Licensed under RustMailer License Agreement v1.0
// Unauthorized copying, modification, or distribution is prohibited.

use crate::modules::common::auth::ClientContext;
use crate::modules::common::paginated::paginate_vec;
use crate::modules::error::code::ErrorCode;
use crate::modules::message::append::ReplyDraft;
use crate::modules::rest::api::ApiTags;
use crate::modules::rest::response::DataPage;
use crate::modules::rest::ApiResult;
use crate::modules::scheduler::model::TaskStatus;
use crate::modules::smtp::queue::message::SendEmailTask;
use crate::modules::smtp::request::builder::EmailBuilder;
use crate::modules::smtp::request::forward::ForwardEmailRequest;
use crate::modules::smtp::request::new::{SendDraftRequest, SendEmailRequest};
use crate::modules::smtp::request::reply::ReplyEmailRequest;
use crate::modules::tasks::queue::RustMailerTaskQueue;
use crate::raise_error;
use poem::web::Path;
use std::collections::BTreeSet;

use poem_openapi::param::Query;
use poem_openapi::payload::Json;
use poem_openapi::OpenApi;
pub struct SendMailApi;

#[OpenApi(prefix_path = "/api/v1", tag = "ApiTags::SendMail")]
impl SendMailApi {
    /// Sends a new email for a specified account.
    /// This endpoint constructs and sends a new email based on the provided request data.
    #[oai(
        path = "/send-mail/:account_id",
        method = "post",
        operation_id = "send_new_mail"
    )]
    async fn send_new_mail(
        &self,
        /// The ID of the account sending the email
        account_id: Path<u64>,
        /// A JSON payload containing the details of the email to be sent
        request: Json<SendEmailRequest>,
        context: ClientContext,
    ) -> ApiResult<()> {
        let account_id = account_id.0;
        context.require_account_access(account_id)?;
        let request = request.0;
        Ok(request.build(account_id).await?)
    }

    /// Creates a new draft email without sending it.
    /// This endpoint constructs and saves a draft email to the account's drafts folder.
    #[oai(
        path = "/save-draft/:account_id",
        method = "post",
        operation_id = "save_draft"
    )]
    async fn save_draft(
        &self,
        /// The ID of the email account for which the draft is created.
        account_id: Path<u64>,
        /// Request body containing the draft email content.
        payload: Json<SendEmailRequest>,
        /// Request context (authentication, authorization).
        context: ClientContext,
    ) -> ApiResult<Json<ReplyDraft>> {
        let account_id = account_id.0;
        context.require_account_access(account_id)?;
        Ok(Json(payload.0.save_as_draft(account_id).await?))
    }

    /// Sends an existing draft email.
    ///
    /// The draft must have been previously created via `save-draft` or `append-reply-to-draft`.
    /// For Gmail API accounts, pass the `draft_id` from the draft response.
    /// For Graph API and IMAP accounts, pass the `id` from the draft response.
    #[oai(
        path = "/send-draft/:account_id",
        method = "post",
        operation_id = "send_draft"
    )]
    async fn send_draft(
        &self,
        /// The ID of the email account that owns the draft.
        account_id: Path<u64>,
        /// Request body containing the draft identifier.
        payload: Json<SendDraftRequest>,
        /// Request context (authentication, authorization).
        context: ClientContext,
    ) -> ApiResult<()> {
        let account_id = account_id.0;
        context.require_account_access(account_id)?;
        Ok(payload.0.send_draft(account_id).await?)
    }

    /// Sends a reply to an existing email for a specified account.
    /// This endpoint constructs and sends a reply to an email based on the provided request data.
    #[oai(
        path = "/reply-mail/:account_id",
        method = "post",
        operation_id = "reply_mail"
    )]
    async fn reply_mail(
        &self,
        /// The ID of the account sending the email
        account_id: Path<u64>,
        /// A JSON payload containing the details of the email reply
        request: Json<ReplyEmailRequest>,
        context: ClientContext,
    ) -> ApiResult<()> {
        let account_id = account_id.0;
        context.require_account_access(account_id)?;
        let request = request.0;
        Ok(request.build(account_id).await?)
    }

    /// Forwards an existing email for a specified account.
    /// This endpoint constructs and sends a forwarded email based on the provided request data.
    #[oai(
        path = "/forward-mail/:account_id",
        method = "post",
        operation_id = "forward_mail"
    )]
    async fn forward_mail(
        &self,
        /// The ID of the account forwarding the email
        account_id: Path<u64>,
        /// A JSON payload containing the details of the email to be forwarded.
        request: Json<ForwardEmailRequest>,
        context: ClientContext,
    ) -> ApiResult<()> {
        let account_id = account_id.0;
        context.require_account_access(account_id)?;
        let request = request.0;
        Ok(request.build(account_id).await?)
    }

    /// Lists email tasks with pagination, sorting, and optional status filtering.
    /// This endpoint retrieves a paginated list of email tasks, filtered by accessible accounts
    /// and optionally by task status. It supports sorting in ascending or descending order by creation time.
    #[oai(
        path = "/send-email-tasks",
        method = "get",
        operation_id = "list_send_email_tasks"
    )]
    async fn list_send_email_tasks(
        &self,
        /// Optional. The page number to retrieve (starting from 1).
        page: Query<Option<u64>>,
        /// Optional. The number of items per page.
        page_size: Query<Option<u64>>,
        /// Optional. Whether to sort the list in descending order.
        desc: Query<Option<bool>>,
        // Optional task status to filter the list.
        status: Query<Option<TaskStatus>>,
        context: ClientContext,
    ) -> ApiResult<Json<DataPage<SendEmailTask>>> {
        let send_queue = RustMailerTaskQueue::get().unwrap();
        let status = status.0;
        let sort_desc = desc.0.unwrap_or(true);

        if context.accessible_accounts()?.is_none() {
            let tasks = match status {
                Some(status) => {
                    send_queue
                        .list_paginated_email_tasks_by_status(
                            page.0,
                            page_size.0,
                            Some(sort_desc),
                            status,
                        )
                        .await?
                }
                None => {
                    send_queue
                        .list_paginated_email_tasks(page.0, page_size.0, Some(sort_desc))
                        .await?
                }
            };
            return Ok(Json(tasks));
        }

        let accessible_accounts = context.accessible_accounts()?.unwrap();
        let allowed_ids: BTreeSet<u64> = accessible_accounts.iter().map(|a| a.id).collect();

        // Fetch all relevant tasks in one go
        let all_tasks = match status {
            Some(status) => send_queue.list_email_tasks_by_status(status).await?,
            None => send_queue.list_all_email_tasks().await?,
        };

        // Filter and sort
        let mut filtered_tasks: Vec<SendEmailTask> = all_tasks
            .into_iter()
            .filter(|task| allowed_ids.contains(&task.account_id))
            .collect();

        filtered_tasks.sort_by(|a, b| {
            if sort_desc {
                b.created_at.cmp(&a.created_at)
            } else {
                a.created_at.cmp(&b.created_at)
            }
        });

        Ok(Json(
            paginate_vec(&filtered_tasks, page.0, page_size.0).map(DataPage::from)?,
        ))
    }

    /// Retrieves a specific email task by its ID.
    /// This endpoint fetches the details of an email task identified by the provided ID.
    #[oai(
        path = "/send-email-task/:id",
        method = "get",
        operation_id = "get_email_task"
    )]
    async fn get_email_task(
        &self,
        /// The ID of the email task to retrieve
        id: Path<u64>,
        context: ClientContext,
    ) -> ApiResult<Json<SendEmailTask>> {
        let send_queue = RustMailerTaskQueue::get().unwrap();
        let task = send_queue
            .get_email_task(id.0)
            .await?
            .ok_or_else(|| raise_error!("Task not found".into(), ErrorCode::ResourceNotFound))?;
        context.require_account_access(task.account_id)?;
        Ok(Json(task))
    }

    /// Mark a email task for deletion from queue
    /// Initiates asynchronous removal of an email task by marking it for deletion.
    /// The task will be:
    /// 1. Immediately marked as "cancelled" in the system
    /// 2. Prevented from any further execution attempts
    /// 3. Physically removed by background cleanup processes
    #[oai(
        path = "/send-email-task/:id",
        method = "delete",
        operation_id = "remove_email_task"
    )]
    async fn remove_email_task(
        &self,
        /// The ID of the email task to remove
        id: Path<u64>,
        context: ClientContext,
    ) -> ApiResult<()> {
        let send_queue = RustMailerTaskQueue::get().unwrap();
        let id = id.0;
        let task = send_queue
            .get_email_task(id)
            .await?
            .ok_or_else(|| raise_error!("Task not found".into(), ErrorCode::ResourceNotFound))?;

        context.require_account_access(task.account_id)?;
        Ok(send_queue.remove_task(id).await?)
    }
}
