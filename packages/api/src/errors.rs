//! Error types of the crate.
//!
//! [`ApiError`] is an error answer in the reference's (hapi's Boom) shape:
//! `{"statusCode": 404, "error": "Not Found", "message": "Block not found"}`.

use std::net::SocketAddr;

use serde_json::{Map, Value};
use thiserror::Error;

/// An error answer, as the reference's Boom writes it.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{status} {error}: {message}")]
pub struct ApiError {
    /// The HTTP status.
    pub status: u16,
    /// The status's reason phrase, as Boom names it.
    pub error: &'static str,
    /// The message.
    pub message: String,
}

impl ApiError {
    /// An error of `status` with `message`.
    pub fn new(status: u16, message: impl Into<String>) -> ApiError {
        ApiError {
            status,
            error: reason_phrase(status),
            message: message.into(),
        }
    }

    /// 400 Bad Request.
    pub fn bad_request(message: impl Into<String>) -> ApiError {
        ApiError::new(400, message)
    }

    /// 403 Forbidden.
    pub fn forbidden(message: impl Into<String>) -> ApiError {
        ApiError::new(403, message)
    }

    /// 404 Not Found.
    pub fn not_found(message: impl Into<String>) -> ApiError {
        ApiError::new(404, message)
    }

    /// 413 Payload Too Large.
    pub fn payload_too_large(message: impl Into<String>) -> ApiError {
        ApiError::new(413, message)
    }

    /// 422 Unprocessable Entity.
    pub fn unprocessable(message: impl Into<String>) -> ApiError {
        ApiError::new(422, message)
    }

    /// 429 Too Many Requests.
    pub fn too_many_requests(message: impl Into<String>) -> ApiError {
        ApiError::new(429, message)
    }

    /// 500 Internal Server Error, with Boom's message for it: the cause is logged, not sent.
    pub fn internal() -> ApiError {
        ApiError::new(500, "An internal server error occurred")
    }

    /// 503 Service Unavailable.
    pub fn unavailable(message: impl Into<String>) -> ApiError {
        ApiError::new(503, message)
    }

    /// The body: `statusCode`, `error` and `message`, in that order.
    pub fn body(&self) -> Value {
        let mut body = Map::new();
        body.insert("statusCode".into(), Value::from(self.status));
        body.insert("error".into(), Value::from(self.error));
        body.insert("message".into(), Value::from(self.message.as_str()));
        Value::Object(body)
    }
}

/// The reason phrase of `status`, as Boom writes it (Node 18's status texts).
pub fn reason_phrase(status: u16) -> &'static str {
    match status {
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        408 => "Request Timeout",
        413 => "Payload Too Large",
        415 => "Unsupported Media Type",
        422 => "Unprocessable Entity",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "Unknown",
    }
}

/// The API could not be served.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ServeError {
    /// The listening socket could not be opened.
    #[error("the API cannot listen on {address}: {detail}")]
    Bind {
        /// The address.
        address: SocketAddr,
        /// Why.
        detail: String,
    },
    /// `CORE_API_SSL` is set: the API serves plain HTTP only.
    #[error(
        "the API does not serve HTTPS (CORE_API_SSL is set): serve it behind a reverse proxy that terminates TLS"
    )]
    Https,
    /// An option is not valid.
    #[error("{0}")]
    Options(String),
}
