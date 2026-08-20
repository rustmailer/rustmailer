// Copyright © 2025-2026 rustmailer.com
// Licensed under RustMailer License Agreement v1.0
// Unauthorized copying, modification, or distribution is prohibited.

use crate::base64_encode_url_safe;
use crate::modules::account::entity::MailerType;
use crate::modules::cache::imap::address::AddressEntity;
use crate::modules::cache::imap::migration::EmailEnvelopeV3;
use crate::modules::cache::imap::sync::flow::generate_uid_sequence_hashset;
use crate::modules::cache::model::Envelope;
use crate::modules::cache::vendor::gmail::sync::client::GmailClient;
use crate::modules::cache::vendor::gmail::sync::envelope::GmailEnvelope;
use crate::modules::cache::vendor::outlook::sync::envelope::OutlookEnvelope;
use crate::modules::common::decode_page_token;
use crate::modules::common::paginated::paginate_vec;
use crate::modules::common::parallel::run_with_limit;
use crate::modules::database::Paginated;
use crate::modules::error::code::ErrorCode;
use crate::modules::message::search::cache::IMAP_SEARCH_CACHE;
use crate::modules::rest::response::CursorDataPage;
use crate::{
    encode_mailbox_name,
    modules::{
        account::migration::AccountModel, context::executors::RUST_MAIL_CONTEXT,
        envelope::extractor::extract_envelope, error::RustMailerResult, rest::response::DataPage,
    },
    raise_error,
};
use ahash::AHashMap;
use chrono::NaiveDate;
use poem_openapi::{Enum, Object, Union};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::info;

/// Represents a single search condition for email messages  
///  
/// A condition consists of a field to search (e.g., FROM, SUBJECT) and an optional value to match.  
/// Some conditions (like ALL, ANSWERED) don't require a value.  
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, Object)]
pub struct Condition {
    /// The type of condition to apply (which field or attribute to search)  
    pub condition: Conditions,
    /// The value to search for (may be null for some condition types)  
    pub value: Option<String>,
}

/// Represents a logical expression combining multiple search conditions  
///  
/// This allows for complex searches using logical operators (AND, OR, NOT)  
/// to combine multiple conditions.  
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, Object)]
pub struct Logic {
    pub operator: Operator,
    pub children: Vec<MessageSearch>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, Enum)]
pub enum Operator {
    And,
    Or,
    Not,
}

/// Enumeration of all possible email search condition types  
///  
/// These conditions correspond to standard IMAP search criteria and allow  
/// searching by various email attributes like sender, recipient, subject, date, etc.  
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, Enum)]
pub enum Conditions {
    /// Match all messages  
    All,
    /// Messages with the Answered flag set  
    Answered,
    /// Messages with the specified text in the BCC field  
    Bcc,
    /// Messages received before the specified date  
    Before,
    /// Messages with the specified text in the body  
    Body,
    /// Messages with the specified text in the CC field  
    Cc,
    /// Messages with the Deleted flag set  
    Deleted,
    /// Messages with the Draft flag set  
    Draft,
    /// Messages with the Flagged flag set  
    Flagged,
    /// Messages with the specified text in the FROM field  
    From,
    /// This is a full Gmail search expression, only available for Gmail API accounts.
    /// Messages with a specific header containing the specified text  
    GmailSeacrch,
    /// Search emails by a specific header value.
    Header,
    /// Messages with the specified keyword flag set  
    Keyword,
    /// Messages larger than the specified size in bytes  
    Larger,
    /// Messages that are new (recently arrived and not seen)  
    New,
    /// Messages that are old (not recently arrived)  
    Old,
    /// Messages received on the specified date  
    On,
    /// Messages that have been recently delivered  
    Recent,
    /// Messages that have been seen (read)  
    Seen,
    /// Messages sent before the specified date  
    SentBefore,
    /// Messages sent on the specified date  
    SentOn,
    /// Messages sent since the specified date  
    SentSince,
    /// Messages received since the specified date  
    Since,
    /// Messages smaller than the specified size in bytes  
    Smaller,
    /// Messages with the specified text in the subject  
    Subject,
    /// Messages containing the specified text in headers or body  
    Text,
    /// Messages with the specified text in the TO field  
    To,
    /// Messages with the specified UID  
    Uid,
    /// Messages that have not been answered  
    Unanswered,
    /// Messages that have not been deleted  
    Undeleted,
    /// Messages that are not drafts  
    Undraft,
    /// Messages that are not flagged  
    Unflagged,
    /// Messages that don't have the specified keyword flag  
    Unkeyword,
    /// Messages that have not been seen (unread)  
    Unseen,
}

/// Represents search criteria for finding email messages  
///  
/// This enum supports two types of search expressions:  
/// 1. Simple conditions (e.g., "FROM contains example@example.com")  
/// 2. Logical expressions that combine multiple conditions (e.g., "FROM contains X AND SUBJECT contains Y")  
///  
/// Example JSON payload:  
/// ```json  
/// {  
///    "type": "Logic",  
///    "operator": "AND",  
///    "children": [  
///      {  
///        "type": "Condition",  
///        "condition": "FROM",  
///        "value": "example@example.com"  
///      },  
///      {  
///        "type": "Condition",  
///        "condition": "SUBJECT",  
///        "value": "Hello"  
///      }  
///    ]  
/// }  
/// ```  
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, Union)]
#[oai(discriminator_name = "type")]
pub enum MessageSearch {
    /// A single search condition  
    Condition(Condition),
    /// A logical expression combining multiple search conditions  
    Logic(Logic),
}
// payload example:
//  {
//     "type": "Logic",
//     "operator": "AND",
//     "children": [
//       {
//         "type": "Condition",
//         "condition": "FROM",
//         "value": "example@example.com"
//       },
//       {
//         "type": "Condition",
//         "condition": "SUBJECT",
//         "value": "Hello"
//       }
//     ]
//  }

impl MessageSearch {
    pub fn to_imap_command(&self, top_level: bool) -> RustMailerResult<String> {
        match self {
            Self::Condition(condition) => {
                let c = &condition.condition;
                let value = condition.value.as_deref();

                let command = match c {
                    Conditions::All => "ALL".into(),
                    Conditions::Answered => "ANSWERED".into(),
                    Conditions::Bcc => format!("BCC {}", Self::quote_value(value)?),
                    Conditions::Before => format!("BEFORE {}", Self::format_date(value)?),
                    Conditions::Body => format!("BODY {}", Self::quote_value(value)?),
                    Conditions::Cc => format!("CC {}", Self::quote_value(value)?),
                    Conditions::Deleted => "DELETED".into(),
                    Conditions::Draft => "DRAFT".into(),
                    Conditions::Flagged => "FLAGGED".into(),
                    Conditions::From => format!("FROM {}", Self::quote_value(value)?),
                    Conditions::Header => {
                        let parts: Vec<&str> = value.unwrap_or("").splitn(2, ' ').collect();
                        if parts.len() == 2 {
                            format!(
                                "HEADER {} {}",
                                Self::quote_value(Some(parts[0]))?,
                                Self::quote_value(Some(parts[1]))?
                            )
                        } else {
                            return Err(raise_error!(
                                "Invalid HEADER format (expected 'header_name value')".into(),
                                ErrorCode::InvalidParameter
                            ));
                        }
                    }
                    Conditions::Keyword => format!("KEYWORD {}", Self::quote_value(value)?),
                    Conditions::Larger => format!("LARGER {}", Self::validate_number(value)?),
                    Conditions::New => "NEW".into(),
                    Conditions::Old => "OLD".into(),
                    Conditions::On => format!("ON {}", Self::format_date(value)?),
                    Conditions::Recent => "RECENT".into(),
                    Conditions::Seen => "SEEN".into(),
                    Conditions::SentBefore => format!("SENTBEFORE {}", Self::format_date(value)?),
                    Conditions::SentOn => format!("SENTON {}", Self::format_date(value)?),
                    Conditions::SentSince => format!("SENTSINCE {}", Self::format_date(value)?),
                    Conditions::Since => format!("SINCE {}", Self::format_date(value)?),
                    Conditions::Smaller => format!("SMALLER {}", Self::validate_number(value)?),
                    Conditions::Subject => format!("SUBJECT {}", Self::quote_value(value)?),
                    Conditions::Text => format!("TEXT {}", Self::quote_value(value)?),
                    Conditions::To => format!("TO {}", Self::quote_value(value)?),
                    Conditions::Uid => {
                        let uid_value = value.ok_or_else(|| {
                            raise_error!(
                                "UID value is required".into(),
                                ErrorCode::InvalidParameter
                            )
                        })?;
                        Self::validate_uid(uid_value)?;
                        format!("UID {}", uid_value)
                    }
                    Conditions::Unanswered => "UNANSWERED".into(),
                    Conditions::Undeleted => "UNDELETED".into(),
                    Conditions::Undraft => "UNDRAFT".into(),
                    Conditions::Unflagged => "UNFLAGGED".into(),
                    Conditions::Unkeyword => format!("UNKEYWORD {}", Self::quote_value(value)?),
                    Conditions::Unseen => "UNSEEN".into(),
                    Conditions::GmailSeacrch => {
                        return Err(raise_error!(
                            "This condition is only supported for Gmail API accounts".into(),
                            ErrorCode::InvalidParameter
                        ));
                    }
                };
                Ok(command)
            }
            Self::Logic(logic) => match logic.operator {
                Operator::And => {
                    let parts: Vec<String> = logic
                        .children
                        .iter()
                        .map(|child| child.to_imap_command(false))
                        .collect::<Result<_, _>>()?;

                    let command = parts.join(" ");
                    if top_level {
                        Ok(command)
                    } else {
                        Ok(format!("({})", command))
                    }
                }
                Operator::Or => {
                    if logic.children.len() < 2 {
                        return Err(raise_error!(
                            "OR must have at least 2 conditions".into(),
                            ErrorCode::InvalidParameter
                        ));
                    }

                    let mut children = logic.children.iter().rev();
                    let first = children.next().unwrap().to_imap_command(false)?;
                    let mut command = first;

                    let remaining = children.len();
                    for (i, child) in children.enumerate() {
                        let child_cmd = child.to_imap_command(false)?;
                        command = format!("OR {} {}", child_cmd, command);
                        if i < remaining - 1 {
                            command = format!("({})", command);
                        }
                    }

                    if !top_level {
                        command = format!("({})", command);
                    }

                    Ok(command)
                }
                Operator::Not => {
                    if logic.children.len() != 1 {
                        return Err(raise_error!(
                            "NOT must have exactly 1 condition".into(),
                            ErrorCode::InvalidParameter
                        ));
                    }
                    let inner = logic.children[0].to_imap_command(false)?;
                    let command = format!("NOT {}", inner);
                    if top_level {
                        Ok(command)
                    } else {
                        Ok(format!("({})", command))
                    }
                }
            },
        }
    }

    pub fn to_gmail_api_search_command(&self) -> RustMailerResult<String> {
        const ERR_MSG: &str = r#"Invalid GmailSeacrch condition format.
            The JSON must include:
            {
                "type": "Condition",
                "condition": "GmailSeacrch",
                "value": "from:example@example.com OR subject:\"Invoice\" after:2025/01/01"
            }
            - "type" must be "Condition"
            - "condition" must be "GmailSeacrch"
            - "value" must be a full Gmail API search query
            "#;

        match self {
            Self::Condition(condition) => match condition.condition {
                Conditions::GmailSeacrch => {
                    let value = condition
                        .value
                        .as_deref()
                        .ok_or_else(|| raise_error!(ERR_MSG.into(), ErrorCode::InvalidParameter))?;
                    Ok(value.into())
                }
                _ => Err(raise_error!(ERR_MSG.into(), ErrorCode::InvalidParameter)),
            },
            Self::Logic(_) => Err(raise_error!(ERR_MSG.into(), ErrorCode::InvalidParameter)),
        }
    }

    fn format_date(date: Option<&str>) -> RustMailerResult<String> {
        let date = date.ok_or_else(|| {
            raise_error!("Date value is required".into(), ErrorCode::InvalidParameter)
        })?;
        let naive_date = NaiveDate::parse_from_str(date, "%Y-%m-%d").map_err(|_| {
            raise_error!(
                "Invalid date format (expected YYYY-MM-DD)".into(),
                ErrorCode::InvalidParameter
            )
        })?;
        Ok(naive_date.format("%d-%b-%Y").to_string())
    }

    fn validate_number(number: Option<&str>) -> RustMailerResult<String> {
        let number = number.ok_or_else(|| {
            raise_error!(
                "Number value is required".into(),
                ErrorCode::InvalidParameter
            )
        })?;
        if number.parse::<u64>().is_err() {
            return Err(raise_error!(
                "Invalid number format".into(),
                ErrorCode::InvalidParameter
            ));
        }
        Ok(number.to_string())
    }

    fn validate_uid(uid_value: &str) -> RustMailerResult<()> {
        let uid_regex =
            regex::Regex::new(r"^(\d+|\d+:\d+)(\s+\d+|\s+\d+:\d+)*$").map_err(|_| {
                raise_error!(
                    "Failed to compile UID regex".into(),
                    ErrorCode::InvalidParameter
                )
            })?;

        if !uid_regex.is_match(uid_value) {
            return Err(raise_error!(
                "Invalid UID format (expected single number, multiple numbers, or range)".into(),
                ErrorCode::InvalidParameter
            ));
        }
        for part in uid_value.split_whitespace() {
            if part.contains(':') {
                let range_parts: Vec<&str> = part.split(':').collect();
                if range_parts.len() != 2 {
                    return Err(raise_error!(
                        "Invalid UID range format (expected 'start:end')".into(),
                        ErrorCode::InvalidParameter
                    ));
                }
                let start = range_parts[0].parse::<u64>().map_err(|_| {
                    raise_error!(
                        "Invalid UID range start value".into(),
                        ErrorCode::InvalidParameter
                    )
                })?;
                let end = range_parts[1].parse::<u64>().map_err(|_| {
                    raise_error!(
                        "Invalid UID range end value".into(),
                        ErrorCode::InvalidParameter
                    )
                })?;
                if start > end {
                    return Err(raise_error!(
                        "UID range start must be less than or equal to end".into(),
                        ErrorCode::InvalidParameter
                    ));
                }
            } else {
                part.parse::<u64>().map_err(|_| {
                    raise_error!("Invalid UID value".into(), ErrorCode::InvalidParameter)
                })?;
            }
        }

        Ok(())
    }

    fn quote_value(value: Option<&str>) -> RustMailerResult<String> {
        let value = value
            .ok_or_else(|| raise_error!("Value is required".into(), ErrorCode::InvalidParameter))?
            .trim();

        let value = if (value.starts_with('"') && value.ends_with('"'))
            || (value.starts_with('\'') && value.ends_with('\''))
        {
            &value[1..value.len() - 1]
        } else {
            value
        };

        let value = value.replace("\\\"", "\"").replace("\\'", "'");
        Ok(format!("\"{}\"", value))
    }
}

/// Request for searching messages in a specific mailbox  
///  
/// This structure combines search criteria with a target mailbox.  
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, Object)]
pub struct MessageSearchRequest {
    /// The search criteria to apply (can be a simple condition or complex logical expression)  
    pub search: MessageSearch,
    /// The name of the mailbox to search in
    /// - For **IMAP accounts**, this field is **required** and specifies which mailbox
    ///   (e.g. `INBOX`, `Sent`, or a custom folder) the search will run against.
    /// - For **Gmail API accounts**, this field is **optional**. If provided, it is treated
    ///   as a label name and will override any label filter specified in the `query` string.
    pub mailbox: Option<String>,
}

impl MessageSearchRequest {
    fn imap_search_cache_key(
        &self,
        account_id: u64,
        page_size: u64,
        desc: bool,
        mailbox: &str,
        search_query: &str,
    ) -> String {
        format!(
            "{}_{}_{}_{}_{}",
            account_id, mailbox, page_size, desc, search_query
        )
    }

    pub async fn search_impl(
        &self,
        account_id: u64,
        next_page_token: Option<&str>,
        page_size: u64,
        desc: bool,
    ) -> RustMailerResult<CursorDataPage<Envelope>> {
        let account = AccountModel::get(account_id).await?;
        if !account.enabled {
            return Err(raise_error!(
                format!("Account id='{account_id}' is disabled"),
                ErrorCode::AccountDisabled
            ));
        }

        match account.mailer_type {
            MailerType::ImapSmtp => {
                self.imap_search_impl(&account, next_page_token, page_size, desc)
                    .await
            }
            MailerType::GmailApi => {
                self.gmail_api_search_impl(&account, next_page_token, page_size)
                    .await
            }
            MailerType::GraphApi => todo!(),
        }
    }

    async fn gmail_api_search_impl(
        &self,
        account: &AccountModel,
        next_page_token: Option<&str>,
        page_size: u64,
    ) -> RustMailerResult<CursorDataPage<Envelope>> {
        if page_size == 0 {
            return Err(raise_error!(
                "page_size must be greater than 0.".into(),
                ErrorCode::InvalidParameter
            ));
        }
        if page_size > 500 {
            return Err(raise_error!(
                "The page_size exceeds the maximum allowed limit of 500.".into(),
                ErrorCode::InvalidParameter
            ));
        }

        let query = self.search.to_gmail_api_search_command()?;
        let label_map: AHashMap<String, String> =
            GmailClient::for_lookup_label_id(account.id, account.use_proxy, false).await?;

        let label_id = match self.mailbox.as_deref() {
            Some(name) => label_map.get(name).cloned().map(Some).ok_or_else(|| {
                raise_error!(
                    format!("Label '{}' not found in Gmail account", name),
                    ErrorCode::InvalidParameter
                )
            })?,
            None => None,
        };

        let message_list = GmailClient::search_messages(
            account.id,
            account.use_proxy,
            label_id.as_deref(),
            next_page_token,
            Some(query.as_str()),
            page_size,
        )
        .await?;

        let total = message_list.result_size_estimate.ok_or_else(|| {
            raise_error!(
                "Missing 'resultSizeEstimate' in Gmail API response".into(),
                ErrorCode::InternalError
            )
        })?;

        let messages = message_list.messages;
        let messages = match messages {
            Some(ref msgs) if !msgs.is_empty() => msgs,
            _ => {
                return Ok(CursorDataPage {
                    next_page_token: None,
                    page_size: Some(page_size),
                    total_items: 0,
                    items: vec![],
                    total_pages: Some(0),
                })
            }
        };

        let account_id = account.id;
        let use_proxy = account.use_proxy;
        let next_page_token = message_list.next_page_token;
        let batch_messages = run_with_limit(5, messages.iter().cloned(), move |index| async move {
            GmailClient::get_message(account_id, use_proxy, &index.id).await
        })
        .await?;

        let label_map = GmailClient::for_get_label_name(account.id, account.use_proxy).await?;

        let envelopes: Vec<Envelope> = batch_messages
            .into_iter()
            .map(|m| {
                let mut envelope: GmailEnvelope = m.try_into()?;
                envelope.account_id = account_id;
                Ok(envelope.into_envelope(&label_map))
            })
            .collect::<RustMailerResult<Vec<Envelope>>>()?;

        let total_pages = (total as f64 / page_size as f64).ceil() as u64;

        Ok(CursorDataPage {
            next_page_token,
            page_size: Some(page_size),
            total_items: total,
            items: envelopes,
            total_pages: Some(total_pages),
        })
    }

    async fn imap_search_impl(
        &self,
        account: &AccountModel,
        next_page_token: Option<&str>,
        page_size: u64,
        desc: bool,
    ) -> RustMailerResult<CursorDataPage<Envelope>> {
        // Validate page and page_size
        if page_size == 0 {
            return Err(raise_error!(
                "page_size must be greater than 0.".into(),
                ErrorCode::InvalidParameter
            ));
        }
        if page_size > 500 {
            return Err(raise_error!(
                "The page_size exceeds the maximum allowed limit of 500.".into(),
                ErrorCode::InvalidParameter
            ));
        }

        let page = decode_page_token(next_page_token)?;
        let mailbox = self.mailbox.as_deref().ok_or_else(|| {
            raise_error!(
                "IMAP accounts must specify a mailbox (e.g. INBOX, Sent, or custom folder)".into(),
                ErrorCode::InvalidParameter
            )
        })?;

        let search_query = self.search.to_imap_command(true)?;

        info!(
            "Executing remote search for account_id: {}, mailbox: {}, with query: {}",
            account.id, mailbox, &search_query
        );
        let excutor = RUST_MAIL_CONTEXT.imap(account.id).await?;
        let cache_key =
            self.imap_search_cache_key(account.id, page_size, desc, mailbox, &search_query);

        // Attempt to retrieve from cache
        if let Some(v) = IMAP_SEARCH_CACHE.get(&cache_key).await {
            let uid_pages = &v.0;
            let total = v.1;
            let total_pages = (total as f64 / page_size as f64).ceil() as u64;

            if page > total_pages {
                return Ok(CursorDataPage::new(
                    None,
                    Some(page_size),
                    total,
                    Some(total_pages),
                    Vec::new(),
                ));
            }

            let current_page_uids = &uid_pages[(page - 1) as usize];
            let fetches = excutor
                .uid_fetch_meta(
                    current_page_uids,
                    encode_mailbox_name!(mailbox).as_str(),
                    false,
                )
                .await?;
            let mut envelopes = Vec::new();
            for fetch in fetches {
                let envelope = extract_envelope(&fetch, account.id, mailbox)?;
                envelopes.push(envelope.into());
            }

            let next_page_token = if page == total_pages {
                None
            } else {
                Some(base64_encode_url_safe!((page + 1).to_string()))
            };

            return Ok(CursorDataPage::new(
                next_page_token,
                Some(page_size),
                total,
                Some(total_pages),
                envelopes,
            ));
        }

        // Cache miss, perform search and fetch data
        let uid_sets = excutor
            .uid_search(&encode_mailbox_name!(mailbox), &search_query)
            .await?;
        if uid_sets.is_empty() {
            IMAP_SEARCH_CACHE
                .set(cache_key, Arc::new((vec![], 0)))
                .await;
            return Ok(CursorDataPage::new(
                None,
                Some(page_size),
                0,
                None,
                Vec::new(),
            ));
        }

        let total_items = uid_sets.len() as u64;
        let total_pages = (total_items as f64 / page_size as f64).ceil() as u64;

        let mut nums: Vec<u32> = uid_sets.into_iter().collect();
        nums.sort();

        let pages = generate_uid_sequence_hashset(nums, page_size as usize, desc);
        assert_eq!(total_pages, pages.len() as u64);
        IMAP_SEARCH_CACHE
            .set(cache_key, Arc::new((pages.clone(), total_items)))
            .await;

        if page > total_pages {
            return Ok(CursorDataPage::new(
                None,
                Some(page_size),
                total_items,
                Some(total_pages),
                Vec::new(),
            ));
        }

        let current_page_uids = &pages[(page - 1) as usize];
        let fetches = excutor
            .uid_fetch_meta(current_page_uids, &encode_mailbox_name!(mailbox), false)
            .await?;
        let mut envelopes = Vec::new();
        for fetch in fetches {
            let envelope = extract_envelope(&fetch, account.id, mailbox)?;
            envelopes.push(envelope.into());
        }
        let next_page_token = if page == total_pages {
            None
        } else {
            Some(base64_encode_url_safe!((page + 1).to_string()))
        };

        Ok(CursorDataPage::new(
            next_page_token,
            Some(page_size),
            total_items,
            Some(total_pages),
            envelopes,
        ))
    }
}

/// Query parameters for unified customer email search.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, Object)]
pub struct UnifiedSearchRequest {
    /// Optional list of account IDs to search within. If omitted, search all accessible accounts.
    pub accounts: Option<Vec<u64>>,
    /// Customer email address to search for (appears in from, to, cc, bcc).
    pub email: String,
    /// Optional start timestamp (UTC milliseconds). Filters messages after this time.
    pub after: Option<i64>,
    /// Optional end timestamp (UTC milliseconds). Filters messages before this time.
    pub before: Option<i64>,
}

impl UnifiedSearchRequest {
    pub async fn search(
        &self,
        page: u64,
        page_size: u64,
        desc: bool,
    ) -> RustMailerResult<DataPage<Envelope>> {
        if page == 0 || page_size == 0 {
            return Err(raise_error!(
                "'page' and 'page_size' must be greater than 0.".into(),
                ErrorCode::InvalidParameter
            ));
        }

        let filter = |entities: Vec<AddressEntity>| {
            entities.into_iter().filter(|e| {
                let ts = e.internal_date.unwrap_or(0);
                let time_match =
                    self.after.map_or(true, |a| ts >= a) && self.before.map_or(true, |b| ts <= b);
                let account_match = self
                    .accounts
                    .as_ref()
                    .map_or(true, |accounts| accounts.contains(&e.account_id));
                time_match && account_match
            })
        };

        let from = filter(AddressEntity::from(&self.email).await?);
        let to = filter(AddressEntity::to(&self.email).await?);
        let cc = filter(AddressEntity::cc(&self.email).await?);
        let all = from.into_iter().chain(to.into_iter()).chain(cc.into_iter());

        let mut hash_map: AHashMap<u64, (u64, i64)> = AHashMap::new();
        for entity in all {
            let ts = entity.internal_date.unwrap_or(0);
            hash_map
                .entry(entity.envelope_hash)
                .or_insert((entity.account_id, ts));
        }

        let mut vec: Vec<(u64, u64, i64)> = hash_map
            .into_iter()
            .map(|(hash, (account_id, ts))| (hash, account_id, ts))
            .collect();
        vec.sort_by(|a, b| b.2.cmp(&a.2));

        if !desc {
            vec.reverse();
        }

        let result = paginate_vec(&vec, Some(page), Some(page_size))?;
        let mut items = Vec::new();
        for (id, account_id, _) in result.items {
            let account = AccountModel::get(account_id).await?;
            let envelope = match account.mailer_type {
                MailerType::ImapSmtp => EmailEnvelopeV3::get(id)
                    .await?
                    .ok_or_else(|| {
                        raise_error!(
                            format!(
                                "Failed to get EmailEnvelope for hash {id} in search operation"
                            ),
                            ErrorCode::InternalError
                        )
                    })?
                    .into(),
                MailerType::GmailApi => {
                    let label_map =
                        GmailClient::for_get_label_name(account_id, account.use_proxy).await?;
                    let envelope = GmailEnvelope::get(id).await?.ok_or_else(|| {
                        raise_error!(
                            format!(
                                "Failed to get GmailEnvelope for hash {id} in search operation"
                            ),
                            ErrorCode::InternalError
                        )
                    })?;
                    envelope.into_envelope(&label_map)
                }
                MailerType::GraphApi => {
                    let envelope = OutlookEnvelope::get(id).await?.ok_or_else(|| {
                        raise_error!(
                            format!(
                                "Failed to get OutlookEnvelope for hash {id} in search operation"
                            ),
                            ErrorCode::InternalError
                        )
                    })?;
                    envelope.into()
                }
            };
            items.push(envelope);
        }

        let paginated = Paginated::new(
            result.page,
            result.page_size,
            result.total_items,
            result.total_pages,
            items,
        );

        Ok(paginated.into())
    }
}
