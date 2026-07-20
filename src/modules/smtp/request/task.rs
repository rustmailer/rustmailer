// Copyright © 2025-2026 rustmailer.com
// Licensed under RustMailer License Agreement v1.0
// Unauthorized copying, modification, or distribution is prohibited.

use std::sync::Arc;
use std::time::Instant;

use crate::modules::account::entity::MailerType;
use crate::modules::cache::disk::DISK_CACHE;
use crate::modules::cache::vendor::gmail::sync::client::GmailClient;
use crate::modules::cache::vendor::outlook::sync::client::OutlookClient;
use crate::modules::error::code::ErrorCode;
use crate::modules::error::RustMailerResult;
use crate::modules::hook::channel::{Event, EVENT_CHANNEL};
use crate::modules::hook::events::{
    payload::EmailSentSuccess, EventPayload, EventType, RustMailerEvent,
};
use crate::modules::hook::task::EventHookTask;
use crate::modules::metrics::{
    FAILURE, RUSTMAILER_EMAIL_SEND_DURATION_SECONDS, RUSTMAILER_EMAIL_SENT_BYTES,
    RUSTMAILER_EMAIL_SENT_TOTAL, SUCCESS,
};
use crate::modules::smtp::executor::SmtpExecutor;
use crate::{base64_encode, base64_encode_url_safe, raise_error};

use crate::modules::scheduler::{
    retry::{RetryPolicy, RetryStrategy},
    task::{Task, TaskFuture},
};

use crate::modules::smtp::{
    mta::entity::Mta,
    request::{EmailHandler, MailEnvelope, SendControl, Strategy},
};

use crate::modules::{account::migration::AccountModel, context::executors::RUST_MAIL_CONTEXT};

use mail_send::smtp::message::{Address, Message, Parameters};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncReadExt;

pub const EXT_DSN: u32 = 1 << 10;
pub const OUTBOX_QUEUE: &str = "send_email";

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct SmtpTask {
    pub account_id: u64,
    pub account_email: String,
    pub subject: Option<String>,
    pub message_id: String,
    pub from: String,
    pub to: Vec<String>,
    pub cc: Option<Vec<String>>,
    pub bcc: Option<Vec<String>>,
    pub attachment_count: usize,
    pub control: Option<SendControl>,
    pub cache_key: String,
    pub answer_email: Option<AnswerEmail>,
    /// If set, the draft with this UID will be deleted from the drafts mailbox
    /// after the email is successfully sent.
    #[serde(default)]
    pub delete_draft_uid: Option<String>,
    /// The encoded name of the drafts mailbox containing the draft to delete.
    #[serde(default)]
    pub delete_draft_mailbox: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct AnswerEmail {
    pub reply: bool,
    pub mailbox: String,
    pub uid: u32,
}

impl SmtpTask {
    async fn build_message<'a>(
        envelope: Option<&'a MailEnvelope>,
        body: &'a [u8],
        from: String,
        recipients: &[String],
        dsn_params: Option<(&'a Parameters<'a>, &'a Parameters<'a>)>,
    ) -> Message<'a> {
        let mut message = Message::empty();
        message = message.body(body);
        if let Some(envelope) = envelope {
            message = message.from(if let Some((mail_params, _)) = dsn_params {
                Address::new(envelope.from.clone(), mail_params.clone())
            } else {
                Address::from(envelope.from.clone())
            });

            for recip in &envelope.recipients {
                message = message.to(if let Some((_, rcpt_params)) = dsn_params {
                    Address::new(recip.clone(), rcpt_params.clone())
                } else {
                    Address::from(recip.clone())
                });
            }
        } else {
            message = message.from(from);
            for recip in recipients {
                message = message.to(recip.clone());
            }
        }

        message
    }

    async fn load_email_body(&self) -> RustMailerResult<Vec<u8>> {
        let mut reader = DISK_CACHE
            .get_cache(&self.cache_key)
            .await?
            .ok_or_else(|| {
                raise_error!(
                    "failed to get cache reader to load email body.".into(),
                    ErrorCode::InternalError
                )
            })?;
        let mut body = Vec::new();
        reader.read_to_end(&mut body).await.map_err(|e| {
            raise_error!(
                format!("failed to load email body from disk cache. {:#?}", e),
                ErrorCode::InternalError
            )
        })?;
        Ok(body)
    }

    fn record_send_failure_metrics(start: Instant) {
        let elapsed = start.elapsed();
        RUSTMAILER_EMAIL_SEND_DURATION_SECONDS
            .with_label_values(&[FAILURE])
            .observe(elapsed.as_secs_f64());
        RUSTMAILER_EMAIL_SENT_TOTAL
            .with_label_values(&[FAILURE])
            .inc();
    }

    async fn handle_email_send_success(
        &self,
        start: Instant,
        body_len: usize,
    ) -> RustMailerResult<()> {
        let elapsed = start.elapsed();
        RUSTMAILER_EMAIL_SEND_DURATION_SECONDS
            .with_label_values(&[SUCCESS])
            .observe(elapsed.as_secs_f64());
        RUSTMAILER_EMAIL_SENT_TOTAL
            .with_label_values(&[SUCCESS])
            .inc();
        RUSTMAILER_EMAIL_SENT_BYTES.inc_by(body_len as u64);
        if EventHookTask::is_watching_email_sent_success(self.account_id).await? {
            EVENT_CHANNEL
                .queue(Event::new(
                    self.account_id,
                    &self.account_email,
                    RustMailerEvent::new(
                        EventType::EmailSentSuccess,
                        EventPayload::EmailSentSuccess(EmailSentSuccess {
                            account_id: self.account_id,
                            account_email: self.account_email.clone(),
                            from: self.from.clone(),
                            to: self.to.clone(),
                            subject: self.subject.clone(),
                            message_id: self.message_id.clone(),
                        }),
                    ),
                ))
                .await;
        }
        Ok(())
    }

    async fn finalize_sent_email(&self, body: &[u8]) -> RustMailerResult<()> {
        if let Some(answer_email) = &self.answer_email {
            EmailHandler::mark_message_answered(
                self.account_id,
                &answer_email.mailbox,
                answer_email.uid,
            )
            .await?;
        }

        if let Some(send_control) = &self.control {
            send_control
                .save_to_sent_if_needed(self.account_id, body)
                .await?;
        }

        if let (Some(uid), Some(mailbox)) = (&self.delete_draft_uid, &self.delete_draft_mailbox) {
            let executor = RUST_MAIL_CONTEXT.imap(self.account_id).await?;
            executor.uid_delete_envelopes(uid, mailbox).await?;
        }

        Ok(())
    }

    async fn build_message_with_optional_params<'a>(
        &'a self,
        body: &'a [u8],
        params: &'a Option<(Parameters<'a>, Parameters<'a>)>,
    ) -> Message<'a> {
        let envelope_opt = self.control.as_ref().and_then(|c| c.envelope.as_ref());
        if let Some((mail_params, rcpt_params)) = params {
            Self::build_message(
                envelope_opt,
                body,
                self.from.clone(),
                &self.to,
                Some((mail_params, rcpt_params)),
            )
            .await
        } else {
            Self::build_message(envelope_opt, body, self.from.clone(), &self.to, None).await
        }
    }
}

impl Task for SmtpTask {
    const TASK_KEY: &'static str = "send_email";
    const TASK_QUEUE: &'static str = OUTBOX_QUEUE;

    //default delay seconds
    fn delay_seconds(&self) -> u32 {
        0
    }

    fn retry_policy(&self) -> RetryPolicy {
        if let Some(control) = &self.control {
            if let Some(rp) = &control.retry_policy {
                let strategy = match rp.strategy {
                    Strategy::Linear => RetryStrategy::Linear {
                        interval: rp.seconds,
                    },
                    Strategy::Exponential => RetryStrategy::Exponential { base: rp.seconds },
                };
                return RetryPolicy {
                    strategy,
                    max_retries: Some(rp.max_retries),
                };
            }
        }

        RetryPolicy {
            strategy: RetryStrategy::Linear { interval: 2 },
            max_retries: Some(10),
        }
    }

    fn run(self, _task_id: u64) -> TaskFuture {
        Box::pin(async move {
            let account = AccountModel::get(self.account_id).await?;
            let start = Instant::now();
            let body = self.load_email_body().await?;

            if let Some(control) = &self.control {
                if let Some(mta) = control.mta {
                    let mta = Mta::get(mta).await?.ok_or_else(|| {
                        raise_error!("MTA not found.".into(), ErrorCode::ResourceNotFound)
                    })?;
                    let executor = RUST_MAIL_CONTEXT.mta(mta.id).await?;
                    let params = if mta.dsn_capable {
                        let params = control.build_dsn_params()?;
                        Some(params)
                    } else {
                        None
                    };

                    let message = self
                        .build_message_with_optional_params(&body, &params)
                        .await;
                    match send_email(executor, message).await {
                        Ok(()) => {
                            self.handle_email_send_success(start, body.len()).await?;
                            if matches!(account.mailer_type, MailerType::ImapSmtp) {
                                self.finalize_sent_email(&body).await?;
                            }

                            return Ok(());
                        }
                        Err(e) => {
                            Self::record_send_failure_metrics(start);
                            return Err(e);
                        }
                    }
                }
            }

            match account.mailer_type {
                MailerType::ImapSmtp => {
                    let executor = RUST_MAIL_CONTEXT.smtp(account.id).await?;

                    let dsn_capable = if let Some(dsn_capable) = &account.dsn_capable {
                        *dsn_capable
                    } else {
                        let capabilities = executor.capabilities(&account.smtp.as_ref().expect("BUG: account.smtp is None, but it should always be present at this point").host).await?;
                        let dsn_capable = capabilities & EXT_DSN != 0;
                        AccountModel::update_dsn_capable(account.id, dsn_capable).await?;
                        dsn_capable
                    };

                    let params = if dsn_capable {
                        self.control
                            .as_ref()
                            .map(|c| c.build_dsn_params())
                            .transpose()?
                    } else {
                        None
                    };

                    let message = self
                        .build_message_with_optional_params(&body, &params)
                        .await;
                    match send_email(executor, message).await {
                        Ok(()) => {
                            self.handle_email_send_success(start, body.len()).await?;
                            self.finalize_sent_email(&body).await
                        }
                        Err(e) => {
                            Self::record_send_failure_metrics(start);
                            Err(e)
                        }
                    }
                }
                MailerType::GmailApi => {
                    let envelope_opt = self.control.as_ref().and_then(|c| c.envelope.as_ref());
                    let message =
                        Self::build_message(envelope_opt, &body, self.from.clone(), &self.to, None)
                            .await;
                    let raw_encoded = base64_encode_url_safe!(&message.body);
                    match gmail_send_email(self.account_id, account.use_proxy, raw_encoded).await {
                        Ok(_) => self.handle_email_send_success(start, body.len()).await,
                        Err(e) => {
                            Self::record_send_failure_metrics(start);
                            Err(e)
                        }
                    }
                }
                MailerType::GraphApi => {
                    let envelope_opt = self.control.as_ref().and_then(|c| c.envelope.as_ref());
                    let message =
                        Self::build_message(envelope_opt, &body, self.from.clone(), &self.to, None)
                            .await;
                    let raw_encoded = base64_encode!(&message.body);
                    match outlook_send_email(self.account_id, account.use_proxy, raw_encoded).await
                    {
                        Ok(_) => self.handle_email_send_success(start, body.len()).await,
                        Err(e) => {
                            Self::record_send_failure_metrics(start);
                            Err(e)
                        }
                    }
                }
            }
        })
    }
}

async fn send_email(executor: Arc<SmtpExecutor>, message: Message<'_>) -> RustMailerResult<()> {
    executor.send_email(message).await
}

async fn gmail_send_email(
    account_id: u64,
    use_proxy: Option<u64>,
    raw_encoded: String,
) -> RustMailerResult<()> {
    GmailClient::send_email(account_id, use_proxy, raw_encoded).await?;
    Ok(())
}

async fn outlook_send_email(
    account_id: u64,
    use_proxy: Option<u64>,
    raw_encoded: String,
) -> RustMailerResult<()> {
    OutlookClient::send_email(account_id, use_proxy, raw_encoded).await
}
