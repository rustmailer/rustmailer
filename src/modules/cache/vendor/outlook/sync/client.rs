use bytes::Bytes;
use serde_json::json;
use tracing::error;

use crate::{
    modules::{
        cache::vendor::outlook::model::{
            CategoriesListResponse, Category, MailFolder, MailFoldersResponse, Message, MessageListResponse
        },
        common::http::HttpClient,
        error::{RustMailerResult, code::ErrorCode},
        message::{append::ReplyDraft},
        oauth2::token::OAuth2AccessToken,
    },
    raise_error,
};
use std::{collections::HashMap, future::Future, pin::Pin};

pub struct OutlookClient;

impl OutlookClient {
    pub async fn get_access_token(account_id: u64) -> RustMailerResult<String> {
        let record = OAuth2AccessToken::get(account_id).await?;
        record.and_then(|r| r.access_token).ok_or_else(|| {
            raise_error!(
                "Graph API requires an OAuth2 access token, but authorization is incomplete."
                    .into(),
                ErrorCode::MissingConfiguration
            )
        })
    }

    async fn fetch_mailfolders_page(
        client: &HttpClient,
        url: &str,
        access_token: &str,
    ) -> RustMailerResult<MailFoldersResponse> {
        let value = client.get(url, access_token).await.map_err(|e| {
            raise_error!(format!("Request error: {e:#?}"), ErrorCode::InternalError)
        })?;
        let folders = serde_json::from_value::<MailFoldersResponse>(value)
            .map_err(|e| raise_error!(format!(
                "Failed to deserialize Graph API response into MailFoldersResponse: {:#?}. Possible model mismatch or API change.",
                e
            ), ErrorCode::InternalError))?;
        Ok(folders)
    }

    fn fetch_recursive<'a>(
        client: &'a HttpClient,
        folder_id: Option<&'a str>,
        prefix: &'a str,
        output: &'a mut Vec<MailFolder>,
        access_token: &'a str,
    ) -> Pin<Box<dyn Future<Output = RustMailerResult<()>> + Send + 'a>> {
        Box::pin(async move {
            let mut url = match folder_id {
                Some(id) => {
                    format!("https://graph.microsoft.com/v1.0/me/mailFolders/{id}/childFolders")
                }
                None => "https://graph.microsoft.com/v1.0/me/mailFolders".to_string(),
            };
            loop {
                let resp = Self::fetch_mailfolders_page(client, &url, access_token).await?;
                for mut folder in resp.value {
                    let full_name = if prefix.is_empty() {
                        folder.display_name.clone()
                    } else {
                        format!("{}/{}", prefix, folder.display_name)
                    };
                    folder.display_name = full_name.clone();
                    output.push(folder.clone());
                    if folder.child_folder_count.unwrap_or(0) > 0 {
                        Self::fetch_recursive(
                            client,
                            Some(&folder.id),
                            &full_name,
                            output,
                            access_token,
                        )
                        .await?;
                    }
                }
                if let Some(next) = resp.next_link {
                    url = next;
                } else {
                    break;
                }
            }
            Ok(())
        })
    }

    pub async fn get_folder(
        account_id: u64,
        use_proxy: Option<u64>,
        default_folder_name: &str,
    ) -> RustMailerResult<MailFolder> {
        let client = HttpClient::new(use_proxy).await?;
        let access_token = Self::get_access_token(account_id).await?;
        let url = format!("https://graph.microsoft.com/v1.0/me/mailFolders/{default_folder_name}");
        let value = client.get(&url, &access_token).await.map_err(|e| {
            raise_error!(format!("Request error: {e:#?}"), ErrorCode::InternalError)
        })?;
        let folder = serde_json::from_value::<MailFolder>(value)
            .map_err(|e| raise_error!(format!(
                "Failed to deserialize Graph API response into MailFolder: {:#?}. Possible model mismatch or API change.",
                e
            ), ErrorCode::InternalError))?;
        Ok(folder)
    }

    pub async fn list_mailfolders(
        account_id: u64,
        use_proxy: Option<u64>,
    ) -> RustMailerResult<Vec<MailFolder>> {
        let client = HttpClient::new(use_proxy).await?;
        let access_token = Self::get_access_token(account_id).await?;
        let mut result = Vec::new();
        Self::fetch_recursive(&client, None, "", &mut result, &access_token).await?;

        let inbox = Self::get_folder(account_id, use_proxy, "inbox").await?;
        let sentitems = Self::get_folder(account_id, use_proxy, "sentitems").await?;
        let drafts = Self::get_folder(account_id, use_proxy, "sentitems").await?;

        for folder in &mut result {
            if folder.id == inbox.id {
                folder.display_name = "INBOX".to_string();
            } else if folder.id == sentitems.id {
                folder.display_name = "SENTITEMS".to_string();
            } else if folder.id == drafts.id {
                folder.display_name = "DRAFTS".to_string();
            }
        }
        Ok(result)
    }

    pub async fn list_messages(
        account_id: u64,
        use_proxy: Option<u64>,
        folder_id: &str,
        page: u64,
        page_size: u64,
        after: Option<&str>,
    ) -> RustMailerResult<MessageListResponse> {
        assert!(page > 0, "page must be greater than 0");
        let skip = (page - 1) * page_size;
        let base_url = format!(
            "https://graph.microsoft.com/v1.0/me/mailFolders/{folder_id}/messages?\
            $top={page_size}&\
            $skip={skip}&\
            $orderBy=receivedDateTime desc&\
            $select=id,isRead,conversationId,internetMessageId,from,body,toRecipients,ccRecipients,\
            bccRecipients,replyTo,sender,subject,receivedDateTime,sentDateTime,isRead,bodyPreview,categories&\
            $expand=attachments($select=id,name,contentType,size,isInline,microsoft.graph.fileAttachment/contentId)",
            folder_id = folder_id,
            page_size = page_size,
            skip = skip,
        );

        let url = if let Some(after) = after {
            format!("{}&$filter=receivedDateTime ge {}", base_url, after)
        } else {
            base_url
        };

        let client = HttpClient::new(use_proxy).await?;
        let access_token = Self::get_access_token(account_id).await?;
        let value = client.get(url.as_str(), &access_token).await?;
        let list = match serde_json::from_value::<MessageListResponse>(value.clone()) {
            Ok(res) => res,
            Err(e) => {
                error!(
                    "Failed to deserialize Graph API response into MessageListResponse: {:#?}",
                    e
                );
                error!("Original JSON: {}", value);
                return Err(raise_error!(format!("Failed to deserialize Graph API response into MessageListResponse: {:#?}. Possible model mismatch or API change.",e),ErrorCode::InternalError));
            }
        };

        Ok(list)
    }


    pub async fn get_thread_messages(
        account_id: u64,
        use_proxy: Option<u64>,
        conversation_id: &str,
    ) -> RustMailerResult<MessageListResponse> {
        let url = format!(
            "https://graph.microsoft.com/v1.0/me/messages?\
            $filter=conversationId eq '{}'&\
            $select=id,isRead,conversationId,internetMessageId,from,body,toRecipients,ccRecipients,\
            bccRecipients,replyTo,sender,subject,receivedDateTime,sentDateTime,isRead,bodyPreview,categories&\
            $expand=attachments($select=id,name,contentType,size,isInline,microsoft.graph.fileAttachment/contentId)",
            conversation_id
        );

        let client = HttpClient::new(use_proxy).await?;
        let access_token = Self::get_access_token(account_id).await?;
        let value = client.get(url.as_str(), &access_token).await?;
        let list = match serde_json::from_value::<MessageListResponse>(value.clone()) {
            Ok(res) => res,
            Err(e) => {
                error!(
                    "Failed to deserialize Graph API response into MessageListResponse: {:#?}",
                    e
                );
                error!("Original JSON: {}", value);
                return Err(raise_error!(format!("Failed to deserialize Graph API response into MessageListResponse: {:#?}. Possible model mismatch or API change.",e),ErrorCode::InternalError));
            }
        };

        Ok(list)
    }

    pub async fn get_delta_link(
        account_id: u64,
        use_proxy: Option<u64>,
        folder_id: &str,
    ) -> RustMailerResult<String> {
        let mut url = format!(
            "https://graph.microsoft.com/v1.0/me/mailFolders/{folder_id}/messages/delta?$select=id"
        );
        let client = HttpClient::new(use_proxy).await?;
        let access_token = Self::get_access_token(account_id).await?;
        loop {
            let value = client.get(url.as_str(), &access_token).await?;
            if let Some(next_link) = value.get("@odata.nextLink") {
                url = next_link
                    .as_str()
                    .ok_or_else(|| {
                        raise_error!(
                            format!("unexpected type for @odata.nextLink in response at URL={url}"),
                            ErrorCode::InternalError
                        )
                    })?
                    .to_string();
            } else if let Some(delta_link) = value.get("@odata.deltaLink") {
                return Ok(delta_link
                    .as_str()
                    .ok_or_else(|| {
                        raise_error!(
                            format!(
                                "unexpected type for @odata.deltaLink in response at URL={url}"
                            ),
                            ErrorCode::InternalError
                        )
                    })?
                    .to_string());
            } else {
                return Err(raise_error!(format!(
                    "neither @odata.nextLink nor @odata.deltaLink found in Graph API response at URL={url}"
                ), ErrorCode::InternalError));
            }
        }
    }

    pub async fn get_message(
        account_id: u64,
        use_proxy: Option<u64>,
        id: &str,
    ) -> RustMailerResult<Message> {
        let url = format!("https://graph.microsoft.com/v1.0/me/messages/{id}?\
               $select=id,isRead,conversationId,internetMessageId,from,body,toRecipients,ccRecipients,\
               bccRecipients,replyTo,sender,subject,receivedDateTime,sentDateTime,isRead,bodyPreview,categories&\
               $expand=attachments($select=id,name,contentType,size,isInline,microsoft.graph.fileAttachment/contentId)");

        let client = HttpClient::new(use_proxy).await?;
        let access_token = Self::get_access_token(account_id).await?;
        let value = client.get(url.as_str(), &access_token).await?;
        let message = match serde_json::from_value::<Message>(value.clone()) {
            Ok(msg) => msg,
            Err(e) => {
                error!(
                    "Failed to deserialize Graph API response into Message: {:#?}",
                    e
                );
                error!("Original JSON: {}", value);
                return Err(raise_error!(
                    format!(
                        "Failed to deserialize Graph API response into Message: {:#?}. Possible model mismatch or API change.",
                        e
                    ),
                    ErrorCode::InternalError
                ));
            }
        };

        Ok(message)
    }

    pub async fn get_raw_message(
        account_id: u64,
        use_proxy: Option<u64>,
        id: &str,
    ) -> RustMailerResult<Bytes> {
        let url = format!("https://graph.microsoft.com/v1.0/me/messages/{id}/$value");
        let client = HttpClient::new(use_proxy).await?;
        let access_token = Self::get_access_token(account_id).await?;
        let value = client.get_bytes(url.as_str(), &access_token).await?;
        Ok(value)
    }

    pub async fn get_attachment(
        account_id: u64,
        use_proxy: Option<u64>,
        mid: &str,
        aid: &str,
    ) -> RustMailerResult<String> {
        let url = format!("https://graph.microsoft.com/v1.0/me/messages/{mid}/attachments/{aid}");
        let client = HttpClient::new(use_proxy).await?;
        let access_token = Self::get_access_token(account_id).await?;
        let value = client.get(url.as_str(), &access_token).await?;
        let data = value
            .get("contentBytes")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                raise_error!(
                    "Attachment missing contentBytes — possibly a reference or item attachment."
                        .into(),
                    ErrorCode::InternalError
                )
            })?;
        Ok(data.into())
    }

    pub async fn create_reply(
        account_id: u64,
        use_proxy: Option<u64>,
        mid: &str,
        text: Option<&str>,
        html: Option<&str>,
        reply_all: bool
    ) -> RustMailerResult<ReplyDraft> {
        let url = if reply_all {
            format!("https://graph.microsoft.com/v1.0/me/messages/{mid}/createReplyAll")
        } else {
            format!("https://graph.microsoft.com/v1.0/me/messages/{mid}/createReply")
        };
        let client = HttpClient::new(use_proxy).await?;
        let access_token = Self::get_access_token(account_id).await?;
        let value = client
            .post::<()>(url.as_str(), &access_token, None, true)
            .await?;
        let id = value.get("id").and_then(|v| v.as_str()).ok_or_else(|| {
            raise_error!(
                "Missing id from createReply response.".into(),
                ErrorCode::InternalError
            )
        })?;

        let (content_type, content) = if let Some(html_body) = html {
            ("html", html_body)
        } else if let Some(text_body) = text {
            ("text", text_body)
        } else {
            ("text", "")
        };

        let url = format!("https://graph.microsoft.com/v1.0/me/messages/{id}");
        let data = json!({
            "body": {
                "contentType": content_type,
                "content": content,
            },
            "isRead": false
        });
        client.patch(&url, &access_token, &data).await?;

        let url = format!("https://graph.microsoft.com/v1.0/me/messages/{mid}");
        let data = json!({
            "isRead": false
        });
        client.patch(&url, &access_token, &data).await?;

        let url = format!("https://graph.microsoft.com/v1.0/me/mailFolders/drafts");
        let value = client.get(&url, &access_token).await.map_err(|e| {
            raise_error!(format!("Request error: {e:#?}"), ErrorCode::InternalError)
        })?;
        let folder = serde_json::from_value::<MailFolder>(value)
            .map_err(|e| raise_error!(format!(
                "Failed to deserialize Graph API response into MailFolder: {:#?}. Possible model mismatch or API change.",
                e
            ), ErrorCode::InternalError))?;
        Ok(ReplyDraft {
            id: id.into(),
            draft_id: None,
            draft_folder: folder.display_name,
        })
    }

    /// Creates a new draft message (not a reply) in the Outlook drafts folder.
    pub async fn create_draft(
        account_id: u64,
        use_proxy: Option<u64>,
        raw_base64: String,
    ) -> RustMailerResult<ReplyDraft> {
        let url = "https://graph.microsoft.com/v1.0/me/messages";
        let client = HttpClient::new(use_proxy).await?;
        let access_token = Self::get_access_token(account_id).await?;
        let value = client
            .post_text::<()>(url, &access_token, raw_base64, true)
            .await?;
        let id = value.get("id").and_then(|v| v.as_str()).ok_or_else(|| {
            raise_error!(
                "Missing id from Graph create draft response.".into(),
                ErrorCode::InternalError
            )
        })?;

        let url = format!("https://graph.microsoft.com/v1.0/me/mailFolders/drafts");
        let value = client.get(&url, &access_token).await.map_err(|e| {
            raise_error!(format!("Request error: {e:#?}"), ErrorCode::InternalError)
        })?;
        let folder = serde_json::from_value::<MailFolder>(value)
            .map_err(|e| raise_error!(format!(
                "Failed to deserialize Graph API response into MailFolder: {:#?}. Possible model mismatch or API change.",
                e
            ), ErrorCode::InternalError))?;
        Ok(ReplyDraft {
            id: id.into(),
            draft_id: None,
            draft_folder: folder.display_name,
        })
    }


    pub async fn send_email(
        account_id: u64,
        use_proxy: Option<u64>,
        message_base64: String,
    ) -> RustMailerResult<()> {
        let url = "https://graph.microsoft.com/v1.0/me/sendMail";
        let client = HttpClient::new(use_proxy).await?;
        let access_token = Self::get_access_token(account_id).await?;
        client
            .post_text::<()>(url, &access_token, message_base64, false)
            .await?;
       Ok(())
    }

    /// Sends an existing draft message by its Graph API message ID.
    ///
    /// The `message_id` is the value returned in `ReplyDraft.id` from `create_draft`.
    pub async fn send_draft(
        account_id: u64,
        use_proxy: Option<u64>,
        message_id: &str,
    ) -> RustMailerResult<()> {
        let url = format!("https://graph.microsoft.com/v1.0/me/messages/{message_id}/send");
        let client = HttpClient::new(use_proxy).await?;
        let access_token = Self::get_access_token(account_id).await?;
        client.post::<()>(&url, &access_token, None, false).await?;
        Ok(())
    }

    pub async fn batch_get_categories(
        account_id: u64,
        use_proxy: Option<u64>,
        mids: &[String],
    ) -> RustMailerResult<HashMap<String, Vec<String>>> {
        let url = "https://graph.microsoft.com/v1.0/$batch";
        let client = HttpClient::new(use_proxy).await?;
        let access_token = Self::get_access_token(account_id).await?;

        let mut requests = Vec::new();
        for (_, mid) in mids.iter().enumerate() {
            let request = json!({
                "id": mid,
                "method": "GET",
                "url": format!("/me/messages/{}?$select=categories", mid),
                "headers": {}
            });
            requests.push(request);
        }

        let batch_body = json!({
            "requests": requests
        });

        let response_value = client
            .post(url, &access_token, Some(&batch_body), true)
            .await?;

        let responses = response_value
            .get("responses")
            .ok_or_else(|| raise_error!("Missing 'responses' array in $batch response.".into(), ErrorCode::InternalError))?
            .as_array()
            .ok_or_else(|| raise_error!("'responses' field is not an array.".into(), ErrorCode::InternalError))?;

        let mut categories_map = HashMap::new();

        for res in responses {
            let batch_id = res.get("id").and_then(|v| v.as_str()).unwrap_or("Unknown ID");
            let status = res.get("status").and_then(|s| s.as_i64()).unwrap_or(500);

            if status >= 400 {
                eprintln!(
                    "Graph $batch GET sub-request failed for MID {} with status {}. Error body: {:?}",
                    batch_id,
                    status,
                    res.get("body")
                );
                return Err(raise_error!(
                    format!(
                        "Failed to get categories for one or more messages (MID: {}, Status: {}). Please try again.",
                        batch_id,
                        status
                    ),
                    ErrorCode::ApiCallFailed 
                ));
            }

            if status == 200 || status == 203 {
                if let Some(body) = res.get("body") {
                    let categories = body
                        .get("categories")
                        .and_then(|c| c.as_array())
                        .map(|arr| {
                            arr.iter()
                            .filter_map(|v| v.as_str().map(|s| s.to_string()))
                            .collect::<Vec<String>>()
                        })
                        .unwrap_or_default();
                    
                    categories_map.insert(batch_id.to_string(), categories);
                }
            }
        }

        Ok(categories_map)
    }

    pub async fn batch_modify_categories(
        account_id: u64,
        use_proxy: Option<u64>,
        updates: &[MessageCategoryUpdate],
    ) -> RustMailerResult<()> {
        tracing::debug!("{:#?}", updates);
        let url = "https://graph.microsoft.com/v1.0/$batch";
        let client = HttpClient::new(use_proxy).await?;
        let access_token = Self::get_access_token(account_id).await?;

        let mut requests = Vec::new();
        for (index, update) in updates.iter().enumerate() {
            let request = json!({
                "id": (index + 1).to_string(),
                "method": "PATCH",
                "url": format!("/me/messages/{}", update.mid),
                "body": {
                    "categories": &update.categories,
                },
                "headers": {
                    "Content-Type": "application/json"
                }
            });
            requests.push(request);
        }

        let batch_body = json!({
            "requests": requests
        });

        let response_value = client
            .post(url, &access_token, Some(&batch_body), true)
            .await?;
        let responses = response_value
            .get("responses")
            .ok_or_else(|| {
                raise_error!(
                    "Missing 'responses' array in $batch response.".into(),
                    ErrorCode::InternalError
                )
            })?
            .as_array()
            .ok_or_else(|| {
                raise_error!(
                    "'responses' field is not an array.".into(),
                    ErrorCode::InternalError
                )
            })?;

        for res in responses {
            if let Some(status) = res.get("status").and_then(|s| s.as_i64()) {
                if status >= 400 {
                    return Err(raise_error!(
                        format!(
                            "Graph $batch sub-request failed for id {} with status {}.",
                            res.get("id")
                                .unwrap_or(&json!("Unknown"))
                                .as_str()
                                .unwrap_or("Unknown"),
                            status
                        ),
                        ErrorCode::ApiCallFailed
                    ));
                }
            }
        }

        Ok(())
    }

    pub async fn list_categories(
        account_id: u64,
        use_proxy: Option<u64>
    ) -> RustMailerResult<Vec<Category>> {
        let url = "https://graph.microsoft.com/v1.0/me/outlook/masterCategories".to_string();
        let client = HttpClient::new(use_proxy).await?;
        let access_token = Self::get_access_token(account_id).await?;
        let value = client.get(url.as_str(), &access_token).await?;
        let list = serde_json::from_value::<CategoriesListResponse>(value)
            .map_err(|e| raise_error!(format!(
                "Failed to deserialize Graph API response into CategoriesListResponse: {:#?}. Possible model mismatch or API change.",
                e
            ), ErrorCode::InternalError))?;
        Ok(list.value)
    }

    pub async fn create_categories(
        account_id: u64,
        use_proxy: Option<u64>,
        name: &str,
        color: &str
    ) -> RustMailerResult<()> {
        let url = "https://graph.microsoft.com/v1.0/me/outlook/masterCategories";
        let client = HttpClient::new(use_proxy).await?;
        let access_token = Self::get_access_token(account_id).await?;
        
        let data = json!({
            "color": color,
            "displayName": name
        });

        client
            .post(url, &access_token, Some(&data), false)
            .await?;
        Ok(())
    }

    pub async fn copy_message(
        account_id: u64,
        use_proxy: Option<u64>,
        mid: &str,
        target_folder_id: &str,
    ) -> RustMailerResult<()> {
        let url = format!("https://graph.microsoft.com/v1.0/me/messages/{mid}/copy");
        let client = HttpClient::new(use_proxy).await?;
        let access_token = Self::get_access_token(account_id).await?;

        let data = json!({
          "destinationId": target_folder_id
        });

        client
            .post(url.as_str(), &access_token, Some(&data), false)
            .await?;
        Ok(())
    }

    pub async fn move_message(
        account_id: u64,
        use_proxy: Option<u64>,
        mid: &str,
        target_folder_id: &str,
    ) -> RustMailerResult<()> {
        let url = format!("https://graph.microsoft.com/v1.0/me/messages/{mid}/move");
        let client = HttpClient::new(use_proxy).await?;
        let access_token = Self::get_access_token(account_id).await?;

        let data = json!({
          "destinationId": target_folder_id
        });

        client
            .post(url.as_str(), &access_token, Some(&data), false)
            .await?;
        Ok(())
    }

    pub async fn delete_message(
        account_id: u64,
        use_proxy: Option<u64>,
        mid: &str,
    ) -> RustMailerResult<()> {
        let url = format!("https://graph.microsoft.com/v1.0/me/messages/{mid}");
        let client = HttpClient::new(use_proxy).await?;
        let access_token = Self::get_access_token(account_id).await?;
        client.delete(url.as_str(), &access_token).await
    }

    pub async fn create_folder(
        account_id: u64,
        use_proxy: Option<u64>,
        parent_name: Option<String>,
        folder_name: &str,
    ) -> RustMailerResult<()> {
        let client = HttpClient::new(use_proxy).await?;
        let access_token = Self::get_access_token(account_id).await?;
        let mut url = "https://graph.microsoft.com/v1.0/me/mailFolders".to_string();
        let body = json!({ "displayName": folder_name });

        if let Some(parent_name) = parent_name {
            let mailboxes = Self::list_mailfolders(account_id, use_proxy).await?;
            if let Some(parent) = mailboxes
                .into_iter()
                .find(|m| m.display_name == parent_name)
            {
                url = format!(
                    "https://graph.microsoft.com/v1.0/me/mailFolders/{}/childFolders",
                    parent.id
                );
            } else {
                return Err(raise_error!(
                    format!("Parent folder not found: {}", parent_name),
                    ErrorCode::InternalError
                ));
            }
        }

        client.post(&url, &access_token, Some(&body), false).await?;
        Ok(())
    }

    pub async fn delete_folder(
        account_id: u64,
        use_proxy: Option<u64>,
        folder_id: &str,
    ) -> RustMailerResult<()> {
        let url = format!("https://graph.microsoft.com/v1.0/me/mailFolders/{folder_id}");
        let client = HttpClient::new(use_proxy).await?;
        let access_token = Self::get_access_token(account_id).await?;
        client.delete(url.as_str(), &access_token).await
    }

    pub async fn rename_folder(
        account_id: u64,
        use_proxy: Option<u64>,
        folder_id: &str,
        new_name: &str,
    ) -> RustMailerResult<()> {
        let url = format!("https://graph.microsoft.com/v1.0/me/mailFolders/{folder_id}");
        let client = HttpClient::new(use_proxy).await?;
        let access_token = Self::get_access_token(account_id).await?;
        let data = json!({
          "displayName": new_name
        });
        client.patch(url.as_str(), &access_token, &data).await?;
        Ok(())
    }
}

#[derive(Debug)]
pub struct MessageCategoryUpdate {
    pub mid: String,
    pub categories: Vec<String>,
}
