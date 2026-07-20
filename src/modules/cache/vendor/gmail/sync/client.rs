// Copyright © 2025-2026 rustmailer.com
// Licensed under RustMailer License Agreement v1.0
// Unauthorized copying, modification, or distribution is prohibited.

use std::sync::Arc;

use ahash::AHashMap;
use serde_json::json;
use url::Url;

use crate::{
    modules::{
        cache::vendor::gmail::{
            cache::GMAIL_LABELS_CACHE,
            model::{
                history::HistoryList,
                labels::{Label, LabelDetail, LabelList},
                messages::{FullMessage, MessageList, MessageMeta, PartBody},
                thread::{ThreadList, ThreadMessages},
            },
        },
        common::http::HttpClient,
        error::{code::ErrorCode, RustMailerResult},
        mailbox::{create::CreateMailboxRequest, rename::MailboxUpdateRequest},
        message::append::ReplyDraft,
        oauth2::token::OAuth2AccessToken,
    },
    raise_error,
};
pub struct GmailClient;

impl GmailClient {
    async fn get_access_token(account_id: u64) -> RustMailerResult<String> {
        let record = OAuth2AccessToken::get(account_id).await?;
        record.and_then(|r| r.access_token).ok_or_else(|| {
            raise_error!(
                "Gmail API requires an OAuth2 access token, but authorization is incomplete."
                    .into(),
                ErrorCode::MissingConfiguration
            )
        })
    }

    pub async fn list_labels(
        account_id: u64,
        use_proxy: Option<u64>,
    ) -> RustMailerResult<Vec<Label>> {
        let url = "https://gmail.googleapis.com/gmail/v1/users/me/labels";
        let client = HttpClient::new(use_proxy).await?;
        let access_token = Self::get_access_token(account_id).await?;
        let value = client.get(url, &access_token).await?;
        let list = serde_json::from_value::<LabelList>(value)
            .map_err(|e| raise_error!(format!(
                "Failed to deserialize Gmail API response into LabelList: {:#?}. Possible model mismatch or API change.",
                e
            ), ErrorCode::InternalError))?;
        Ok(list.labels)
    }

    pub async fn for_get_label_name(
        account_id: u64,
        use_proxy: Option<u64>,
    ) -> RustMailerResult<Arc<AHashMap<String, String>>> {
        if let Some(v) = GMAIL_LABELS_CACHE.get(&account_id).await {
            return Ok(v.clone());
        }
        let visible_labels = Self::list_labels(account_id, use_proxy).await?;
        let map: Arc<AHashMap<String, String>> = Arc::new(
            visible_labels
                .into_iter()
                .map(|label| (label.id, label.name))
                .collect(),
        );
        GMAIL_LABELS_CACHE.set(account_id, map.clone()).await;
        Ok(map)
    }

    pub async fn for_lookup_label_id(
        account_id: u64,
        use_proxy: Option<u64>,
        skip_cache: bool,
    ) -> RustMailerResult<AHashMap<String, String>> {
        if !skip_cache {
            if let Some(v) = GMAIL_LABELS_CACHE.get(&account_id).await {
                let map: AHashMap<String, String> =
                    v.iter().map(|(k, v)| (v.clone(), k.clone())).collect();
                return Ok(map);
            }
        }
        let visible_labels = Self::list_labels(account_id, use_proxy).await?;
        let map: Arc<AHashMap<String, String>> = Arc::new(
            visible_labels
                .into_iter()
                .map(|label| (label.id, label.name))
                .collect(),
        );
        GMAIL_LABELS_CACHE.set(account_id, map.clone()).await;
        let map: AHashMap<String, String> =
            map.iter().map(|(k, v)| (v.clone(), k.clone())).collect();
        Ok(map)
    }

    pub async fn get_label(
        account_id: u64,
        use_proxy: Option<u64>,
        label_id: &str,
    ) -> RustMailerResult<LabelDetail> {
        let url = format!(
            "https://gmail.googleapis.com/gmail/v1/users/me/labels/{}",
            label_id
        );
        let client = HttpClient::new(use_proxy).await?;
        let access_token = Self::get_access_token(account_id).await?;
        let value = client.get(url.as_str(), &access_token).await?;
        let detail = serde_json::from_value::<LabelDetail>(value)
            .map_err(|e| raise_error!(format!(
                "Failed to deserialize Gmail API response into LabelDetail: {:#?}. Possible model mismatch or API change.",
                e
            ), ErrorCode::InternalError))?;
        Ok(detail)
    }

    pub async fn create_label(
        account_id: u64,
        use_proxy: Option<u64>,
        request: &CreateMailboxRequest,
    ) -> RustMailerResult<LabelDetail> {
        let url = "https://gmail.googleapis.com/gmail/v1/users/me/labels";
        let client = HttpClient::new(use_proxy).await?;

        let mut body = json!({
            "name": request.mailbox_name,
            "messageListVisibility": "show",
            "labelListVisibility": "labelShow",
            "type": "user"
        });
        if let Some(color) = &request.label_color {
            body["color"] = json!({
                "textColor": color.text_color,
                "backgroundColor": color.background_color
            });
        }
        let access_token = Self::get_access_token(account_id).await?;
        let value = client.post(url, &access_token, Some(&body), true).await?;
        let detail = serde_json::from_value::<LabelDetail>(value)
            .map_err(|e| raise_error!(format!(
                "Failed to deserialize Gmail API response into LabelDetail: {:#?}. Possible model mismatch or API change.",
                e
            ), ErrorCode::InternalError))?;
        Ok(detail)
    }

    pub async fn delete_label(
        account_id: u64,
        use_proxy: Option<u64>,
        label_id: &str,
    ) -> RustMailerResult<()> {
        let url = format!(
            "https://gmail.googleapis.com/gmail/v1/users/me/labels/{}",
            label_id
        );
        let client = HttpClient::new(use_proxy).await?;
        let access_token = Self::get_access_token(account_id).await?;
        client.delete(url.as_str(), &access_token).await?;
        Ok(())
    }

    pub async fn update_label(
        account_id: u64,
        use_proxy: Option<u64>,
        label_id: &str,
        request: &MailboxUpdateRequest,
    ) -> RustMailerResult<()> {
        let url = format!(
            "https://gmail.googleapis.com/gmail/v1/users/me/labels/{}",
            label_id
        );

        let mut body = json!({
            "id": label_id,
            "messageListVisibility": "show",
            "labelListVisibility": "labelShow",
            "type": "user"
        });

        if let Some(new_name) = &request.new_name {
            body["name"] = json!(new_name);
        }

        if let Some(color) = &request.label_color {
            body["color"] = json!({
                "textColor": color.text_color,
                "backgroundColor": color.background_color
            });
        }

        let client = HttpClient::new(use_proxy).await?;
        let access_token = Self::get_access_token(account_id).await?;
        client.put(url.as_str(), &access_token, &body).await?;
        Ok(())
    }

    pub async fn list_messages(
        account_id: u64,
        use_proxy: Option<u64>,
        label_id: Option<&str>,
        page_token: Option<&str>,
        after: Option<&str>,
        max_results: u32,
    ) -> RustMailerResult<MessageList> {
        let mut url = format!(
            "https://gmail.googleapis.com/gmail/v1/users/me/messages?maxResults={}",
            max_results
        );

        if let Some(label_id) = label_id {
            url.push_str(&format!("&labelIds={}", label_id));
        }

        if let Some(after) = after {
            url.push_str(&format!("&q=after:{}", after));
        }

        if let Some(page_token) = page_token {
            url.push_str(&format!("&pageToken={}", page_token));
        }

        let client = HttpClient::new(use_proxy).await?;
        let access_token = Self::get_access_token(account_id).await?;
        let value = client.get(url.as_str(), &access_token).await?;
        let list = serde_json::from_value::<MessageList>(value).map_err(|e| {
            raise_error!(
                format!(
                    "Failed to deserialize Gmail API response into MessageList: {:#?}. Possible model mismatch or API change.",
                    e
                ),
                ErrorCode::InternalError
            )
        })?;
        Ok(list)
    }

    pub async fn list_threads_internal(
        account_id: u64,
        use_proxy: Option<u64>,
        label_id: Option<&str>,
        page_token: Option<&str>,
        after: Option<&str>,
        max_results: u64,
    ) -> RustMailerResult<ThreadList> {
        let mut url = format!(
            "https://gmail.googleapis.com/gmail/v1/users/me/threads?maxResults={}",
            max_results
        );

        if let Some(label_id) = label_id {
            url.push_str(&format!("&labelIds={}", label_id));
        }

        if let Some(after) = after {
            url.push_str(&format!("&q=after:{}", after));
        }

        if let Some(page_token) = page_token {
            url.push_str(&format!("&pageToken={}", page_token));
        }

        let client = HttpClient::new(use_proxy).await?;
        let access_token = Self::get_access_token(account_id).await?;
        let value = client.get(url.as_str(), &access_token).await?;
        let list = serde_json::from_value::<ThreadList>(value).map_err(|e| {
            raise_error!(
                format!(
                    "Failed to deserialize Gmail API response into ThreadList: {:#?}. Possible model mismatch or API change.",
                    e
                ),
                ErrorCode::InternalError
            )
        })?;
        Ok(list)
    }

    pub async fn get_thread_messages(
        account_id: u64,
        use_proxy: Option<u64>,
        thread_id: &str,
    ) -> RustMailerResult<ThreadMessages> {
        let url = format!(
            "https://gmail.googleapis.com/gmail/v1/users/me/threads/{}?format=metadata&metadataHeaders=Message-ID&metadataHeaders=From&metadataHeaders=To&metadataHeaders=Cc&metadataHeaders=Bcc&metadataHeaders=Subject&metadataHeaders=Date&metadataHeaders=Mime-Version&metadataHeaders=Reply-To&metadataHeaders=In-Reply-To&metadataHeaders=References&metadataHeaders=Sender",
            thread_id,
        );
        let client = HttpClient::new(use_proxy).await?;
        let access_token = Self::get_access_token(account_id).await?;
        let value = client.get(url.as_str(), &access_token).await?;
        let messages = serde_json::from_value::<ThreadMessages>(value)
            .map_err(|e| raise_error!(format!(
                "Failed to deserialize Gmail API response into MessageMeta: {:#?}. Possible model mismatch or API change.",
                e
            ), ErrorCode::InternalError))?;
        Ok(messages)
    }

    pub async fn search_messages(
        account_id: u64,
        use_proxy: Option<u64>,
        label_id: Option<&str>,
        page_token: Option<&str>,
        query: Option<&str>,
        max_results: u64,
    ) -> RustMailerResult<MessageList> {
        let mut url = Url::parse("https://gmail.googleapis.com/gmail/v1/users/me/messages")
            .map_err(|e| {
                raise_error!(
                    format!("Failed to parse base URL: {}", e),
                    ErrorCode::InternalError
                )
            })?;

        {
            let mut pairs = url.query_pairs_mut();
            pairs.append_pair("maxResults", &max_results.to_string());

            if let Some(query) = query {
                pairs.append_pair("q", query);
            }
            if let Some(label_id) = label_id {
                pairs.append_pair("labelIds", label_id);
            }
            if let Some(page_token) = page_token {
                pairs.append_pair("pageToken", page_token);
            }
        }

        let client = HttpClient::new(use_proxy).await?;
        let access_token = Self::get_access_token(account_id).await?;
        let value = client.get(url.as_str(), &access_token).await?;
        let list = serde_json::from_value::<MessageList>(value).map_err(|e| {
            raise_error!(
                format!(
                    "Failed to deserialize Gmail API response into MessageList: {:#?}. Possible model mismatch or API change.",
                    e
                ),
                ErrorCode::InternalError
            )
        })?;
        Ok(list)
    }

    pub async fn get_message(
        account_id: u64,
        use_proxy: Option<u64>,
        mid: &str,
    ) -> RustMailerResult<MessageMeta> {
        let url = format!(
            "https://gmail.googleapis.com/gmail/v1/users/me/messages/{}?format=metadata&metadataHeaders=Message-ID&metadataHeaders=From&metadataHeaders=To&metadataHeaders=Cc&metadataHeaders=Bcc&metadataHeaders=Subject&metadataHeaders=Date&metadataHeaders=Mime-Version&metadataHeaders=Reply-To&metadataHeaders=In-Reply-To&metadataHeaders=References&metadataHeaders=Sender",
            mid
        );
        let client = HttpClient::new(use_proxy).await?;
        let access_token = Self::get_access_token(account_id).await?;
        let value = client.get(url.as_str(), &access_token).await?;
        let message = serde_json::from_value::<MessageMeta>(value)
            .map_err(|e| raise_error!(format!(
                "Failed to deserialize Gmail API response into MessageMeta: {:#?}. Possible model mismatch or API change.",
                e
            ), ErrorCode::InternalError))?;
        Ok(message)
    }

    pub async fn batch_delete(
        account_id: u64,
        use_proxy: Option<u64>,
        mids: &[String],
    ) -> RustMailerResult<()> {
        let url = "https://gmail.googleapis.com/gmail/v1/users/me/messages/batchDelete";
        let client = HttpClient::new(use_proxy).await?;
        let access_token = Self::get_access_token(account_id).await?;
        let body = json!({
          "ids": mids
        });

        client.post(url, &access_token, Some(&body), false).await?;
        Ok(())
    }

    pub async fn get_full_messages(
        account_id: u64,
        use_proxy: Option<u64>,
        mid: &str,
    ) -> RustMailerResult<FullMessage> {
        let url = format!(
            "https://gmail.googleapis.com/gmail/v1/users/me/messages/{}?format=full",
            mid
        );
        let client = HttpClient::new(use_proxy).await?;
        let access_token = Self::get_access_token(account_id).await?;
        let value = client.get(url.as_str(), &access_token).await?;
        let message = serde_json::from_value::<FullMessage>(value)
            .map_err(|e| raise_error!(format!(
                "Failed to deserialize Gmail API response into FullMessage: {:#?}. Possible model mismatch or API change.",
                e
            ), ErrorCode::InternalError))?;
        Ok(message)
    }

    pub async fn get_raw_messages(
        account_id: u64,
        use_proxy: Option<u64>,
        mid: &str,
    ) -> RustMailerResult<FullMessage> {
        let url = format!(
            "https://gmail.googleapis.com/gmail/v1/users/me/messages/{}?format=raw",
            mid
        );
        let client = HttpClient::new(use_proxy).await?;
        let access_token = Self::get_access_token(account_id).await?;
        let value = client.get(url.as_str(), &access_token).await?;
        let message = serde_json::from_value::<FullMessage>(value)
            .map_err(|e| raise_error!(format!(
                "Failed to deserialize Gmail API response into FullMessage: {:#?}. Possible model mismatch or API change.",
                e
            ), ErrorCode::InternalError))?;
        Ok(message)
    }

    pub async fn get_attachment(
        account_id: u64,
        use_proxy: Option<u64>,
        mid: &str,
        aid: &str,
    ) -> RustMailerResult<PartBody> {
        let url = format!(
            "https://gmail.googleapis.com/gmail/v1/users/me/messages/{}/attachments/{}",
            mid, aid
        );
        let client = HttpClient::new(use_proxy).await?;
        let access_token = Self::get_access_token(account_id).await?;
        let value = client.get(url.as_str(), &access_token).await?;
        let result = serde_json::from_value::<PartBody>(value)
            .map_err(|e| raise_error!(format!(
                "Failed to deserialize Gmail API response into PartBody: {:#?}. Possible model mismatch or API change.",
                e
            ), ErrorCode::InternalError))?;
        Ok(result)
    }

    pub async fn list_history(
        account_id: u64,
        use_proxy: Option<u64>,
        label_id: Option<&str>,
        start_history_id: &str,
        page_token: Option<&str>,
        max_results: u32,
    ) -> RustMailerResult<HistoryList> {
        let mut url = format!("https://gmail.googleapis.com/gmail/v1/users/me/history?maxResults={}&startHistoryId={}", max_results, start_history_id);

        if let Some(label_id) = label_id {
            url.push_str(&format!("&labelId={}", label_id));
        }

        if let Some(page_token) = page_token {
            url.push_str(&format!("&pageToken={}", page_token));
        }

        let client = HttpClient::new(use_proxy).await?;
        let access_token = Self::get_access_token(account_id).await?;
        let value = client.get(url.as_str(), &access_token).await?;
        let list = serde_json::from_value::<HistoryList>(value)
            .map_err(|e| raise_error!(format!(
                "Failed to deserialize Gmail API response into ListMessagesResponse: {:#?}. Possible model mismatch or API change.",
                e
            ), ErrorCode::InternalError))?;
        Ok(list)
    }

    pub async fn create_draft(
        account_id: u64,
        use_proxy: Option<u64>,
        body: serde_json::Value,
    ) -> RustMailerResult<ReplyDraft> {
        let url = "https://gmail.googleapis.com/gmail/v1/users/me/drafts";
        let client = HttpClient::new(use_proxy).await?;
        let access_token = Self::get_access_token(account_id).await?;
        let value = client.post(url, &access_token, Some(&body), true).await?;
        let draft_id = value
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                raise_error!(
                    "Missing id from Gmail create draft response".into(),
                    ErrorCode::InternalError
                )
            })?;
        let message_id = value
            .get("message")
            .and_then(|m| m.get("id"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                raise_error!(
                    "Missing message.id from Gmail create draft response".into(),
                    ErrorCode::InternalError
                )
            })?;
        let map = Self::for_get_label_name(account_id, use_proxy).await?;
        let name = map.get("DRAFT").ok_or_else(|| {
            raise_error!(
                "Cannot find 'DRAFT' label in Gmail account label map".into(),
                ErrorCode::InternalError
            )
        })?;
        Ok(ReplyDraft {
            id: message_id.into(),
            draft_id: Some(draft_id.into()),
            draft_folder: name.into(),
        })
    }

    /// Sends an existing draft by its Gmail draft ID.
    ///
    /// The `draft_id` is the value returned in `ReplyDraft.draft_id` from `create_draft`.
    /// This is distinct from the message ID stored in `ReplyDraft.id`.
    pub async fn send_draft(
        account_id: u64,
        use_proxy: Option<u64>,
        draft_id: &str,
    ) -> RustMailerResult<()> {
        let url = "https://gmail.googleapis.com/gmail/v1/users/me/drafts/send";
        let client = HttpClient::new(use_proxy).await?;
        let access_token = Self::get_access_token(account_id).await?;
        let body = json!({ "id": draft_id });
        client.post(url, &access_token, Some(&body), true).await?;
        Ok(())
    }

    pub async fn send_email(
        account_id: u64,
        use_proxy: Option<u64>,
        raw_encoded: String,
    ) -> RustMailerResult<serde_json::Value> {
        let url = "https://gmail.googleapis.com/gmail/v1/users/me/messages/send";
        let client = HttpClient::new(use_proxy).await?;
        let access_token = Self::get_access_token(account_id).await?;
        let body = json!({
            "raw": raw_encoded
        });
        let value = client.post(url, &access_token, Some(&body), true).await?;
        Ok(value)
    }

    pub async fn batch_modify(
        account_id: u64,
        use_proxy: Option<u64>,
        mids: &Vec<String>,
        add_label_ids: Vec<String>,
        remove_label_ids: Vec<String>,
    ) -> RustMailerResult<()> {
        let url = "https://gmail.googleapis.com/gmail/v1/users/me/messages/batchModify";
        let client = HttpClient::new(use_proxy).await?;
        let access_token = Self::get_access_token(account_id).await?;
        let body = json!({
          "ids": mids,
          "addLabelIds": add_label_ids,
          "removeLabelIds": remove_label_ids
        });
        client.post(url, &access_token, Some(&body), false).await?;
        Ok(())
    }
}
