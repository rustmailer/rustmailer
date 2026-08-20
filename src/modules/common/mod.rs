// Copyright © 2025-2026 rustmailer.com
// Licensed under RustMailer License Agreement v1.0
// Unauthorized copying, modification, or distribution is prohibited.

use crate::base64_decode_url_safe;
use crate::modules::error::RustMailerResult;
use crate::raise_error;

use super::error::code::ErrorCode;
use super::error::RustMailerError;
use mail_parser::{Addr as ImapAddr, Address as ImapAddress};
use mail_send::mail_builder::headers::address::Address as SmtpAddress;
use mail_send::mail_builder::headers::address::EmailAddress as SmtpEmailAddress;
use poem::error::ResponseError;
use poem::Body;
use poem::{http::StatusCode, Error, Response};
use poem_openapi::Object;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::ops::Deref;
use tracing::error;

pub mod auth;
pub mod error;
pub mod http;
pub mod log;
pub mod lru;
pub mod paginated;
pub mod parallel;
pub mod rustls;
pub mod signal;
pub mod timeout;
pub mod tls;
pub mod validator;
pub mod watchdog;

#[derive(Debug, PartialEq, Eq, Clone, Serialize, Deserialize, Object)]
pub struct Addr {
    /// The optional display name associated with the email address (e.g., "John Doe").
    /// If `None`, no display name is specified.
    pub name: Option<String>,
    /// The optional email address (e.g., "john.doe@example.com").
    /// If `None`, the address is unavailable, though typically at least one of `name` or `address` is provided.
    pub address: Option<String>,
}

impl Addr {
    pub fn parse(s: &str) -> Self {
        let re = Regex::new(r#"(?:(?P<name>.*)\s*)?<(?P<email>[^<>]+)>"#).unwrap();
        if let Some(caps) = re.captures(s) {
            let name: Option<String> = caps.name("name").map(|m| m.as_str().trim().into());
            let email: Option<String> = caps.name("email").map(|m| m.as_str().trim().into());
            Addr {
                name: if let Some(n) = name {
                    if n.is_empty() {
                        None
                    } else {
                        Some(n)
                    }
                } else {
                    None
                },
                address: email,
            }
        } else {
            let s_trimmed = s.trim();
            Addr {
                name: None,
                address: if s_trimmed.is_empty() {
                    None
                } else {
                    Some(s_trimmed.into())
                },
            }
        }
    }
}

impl std::fmt::Display for Addr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (&self.name, &self.address) {
            (Some(name), Some(address)) => write!(f, "{} <{}>", name, address),
            (None, Some(address)) => write!(f, "<{}>", address),
            (Some(name), None) => write!(f, "{}", name),
            (None, None) => write!(f, ""),
        }
    }
}

impl<'x> From<&ImapAddr<'x>> for Addr {
    fn from(original: &ImapAddr<'x>) -> Self {
        Addr {
            name: original.name.as_ref().map(|s| s.to_string()),
            address: original.address.as_ref().map(|s| s.to_string()),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AddrVec(pub Vec<Addr>);

impl Deref for AddrVec {
    type Target = Vec<Addr>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<'x> From<&ImapAddress<'x>> for AddrVec {
    fn from(original: &ImapAddress<'x>) -> Self {
        let vec = match original {
            ImapAddress::List(addrs) => addrs.iter().map(Addr::from).collect(),
            ImapAddress::Group(groups) => groups
                .iter()
                .flat_map(|group| group.addresses.iter().map(Addr::from))
                .collect(),
        };
        AddrVec(vec)
    }
}

impl<'x> From<SmtpEmailAddress<'x>> for Addr {
    fn from(email: SmtpEmailAddress<'x>) -> Self {
        Addr {
            name: email.name.map(|n| n.into_owned()),
            address: Some(email.email.into_owned()),
        }
    }
}

impl<'x> From<&SmtpAddress<'x>> for AddrVec {
    fn from(address: &SmtpAddress<'x>) -> Self {
        fn collect_addresses<'x>(address: &SmtpAddress<'x>, result: &mut Vec<Addr>) {
            match address {
                SmtpAddress::Address(email) => {
                    let addr = Addr::from(email.clone());
                    result.push(addr);
                }
                SmtpAddress::Group(group) => {
                    for addr in &group.addresses {
                        collect_addresses(addr, result);
                    }
                }
                SmtpAddress::List(list) => {
                    for addr in list {
                        collect_addresses(addr, result);
                    }
                }
            }
        }
        let mut addresses = Vec::new();
        collect_addresses(address, &mut addresses);
        AddrVec(addresses)
    }
}

impl<'x> From<Addr> for SmtpAddress<'x> {
    fn from(addr: Addr) -> Self {
        SmtpAddress::Address(SmtpEmailAddress {
            name: addr.name.map(Cow::Owned),
            email: Cow::Owned(addr.address.unwrap_or_default()),
        })
    }
}

// #[derive(Serialize)]
// pub struct ErrorResponse {
//     pub message: String,
// }

#[inline]
fn create_rust_mailer_error(message: &str, code: ErrorCode) -> RustMailerError {
    RustMailerError::Generic {
        message: message.into(),
        location: snafu::Location::default(),
        code,
    }
}

#[inline]
pub fn create_api_error_response(message: &str, code: ErrorCode) -> Error {
    let rust_mailer_error = create_rust_mailer_error(message, code);
    rust_mailer_error.into()
}

impl ResponseError for RustMailerError {
    fn status(&self) -> StatusCode {
        match self {
            RustMailerError::Generic {
                message: _,
                location: _,
                code,
            } => code.status(),
        }
    }

    fn as_response(&self) -> Response
    where
        Self: std::error::Error + Send + Sync + 'static,
    {
        match self {
            RustMailerError::Generic {
                message,
                location,
                code,
            } => {
                error!(
                    error_code = *code as u32,
                    error_message = %message,
                    error_location = ?location
                );

                let body = Body::from_json(serde_json::json!({
                    "code": *code as u32,
                    "message": message.to_string(),
                }))
                .unwrap();

                Response::builder().status(self.status()).body(body)
            }
        }
    }
}

pub fn decode_page_token(next_page_token: Option<&str>) -> RustMailerResult<u64> {
    match next_page_token {
        Some(token) => {
            let decoded = base64_decode_url_safe!(token)
                .ok()
                .and_then(|bytes| String::from_utf8(bytes).ok())
                .and_then(|s| s.parse::<u64>().ok());

            decoded.ok_or_else(|| {
                raise_error!(
                    "Invalid next_page_token: not a valid page token".into(),
                    ErrorCode::InvalidParameter
                )
            })
        }
        None => Ok(1),
    }
}
