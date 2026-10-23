//! Internal SMTP password custody for IMAP mailboxes.

use crate::api::context::{ApiContext, AuthorizationService};
use crate::domain::smtp_password::{MailboxBinding, SmtpPasswordError};
use axum::{
    Json,
    extract::{Query, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use macro_authorization::{InternalOnly, MacroAuthorizationExtractor};
use uuid::Uuid;
use zeroize::Zeroizing;

#[derive(serde::Deserialize)]
pub struct BindingRequest {
    link_id: Uuid,
    generation: i64,
    sync_generation: i64,
}

impl From<&BindingRequest> for MailboxBinding {
    fn from(request: &BindingRequest) -> Self {
        Self {
            link_id: request.link_id,
            grant_generation: request.generation,
            sync_generation: request.sync_generation,
        }
    }
}

/// Secret-bearing request intentionally does not implement Debug.
#[derive(serde::Deserialize)]
pub struct StoreRequest {
    #[serde(flatten)]
    binding: BindingRequest,
    password: String,
}

fn status(error: SmtpPasswordError) -> Response {
    match error {
        SmtpPasswordError::NotConnected => StatusCode::UNAUTHORIZED.into_response(),
        SmtpPasswordError::Missing => StatusCode::NOT_FOUND.into_response(),
        SmtpPasswordError::Unavailable => (
            StatusCode::SERVICE_UNAVAILABLE,
            [(header::RETRY_AFTER, "2")],
        )
            .into_response(),
    }
}

/// Store a password the email service already verified with the SMTP server.
pub async fn store_handler(
    State(ctx): State<ApiContext>,
    _authorization: MacroAuthorizationExtractor<AuthorizationService, InternalOnly>,
    Json(request): Json<StoreRequest>,
) -> Response {
    let Some(service) = ctx.smtp_passwords else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match service
        .store(
            MailboxBinding::from(&request.binding),
            Zeroizing::new(request.password),
        )
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => status(error),
    }
}

/// The password for an active IMAP link's exact binding, for one SMTP session.
pub async fn read_handler(
    State(ctx): State<ApiContext>,
    _authorization: MacroAuthorizationExtractor<AuthorizationService, InternalOnly>,
    Query(request): Query<BindingRequest>,
) -> Response {
    let Some(service) = ctx.smtp_passwords else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match service.password(MailboxBinding::from(&request)).await {
        Ok(password) => (
            [(header::CACHE_CONTROL, "no-store")],
            Json(serde_json::json!({ "password": password.as_str() })),
        )
            .into_response(),
        Err(error) => status(error),
    }
}
