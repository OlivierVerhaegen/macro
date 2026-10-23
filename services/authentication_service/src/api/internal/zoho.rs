//! Internal Zoho credential routes for the email service (Olive Trees fork).

use crate::api::context::{ApiContext, AuthorizationService};
use crate::domain::zoho::ZohoAuthError;
use axum::{
    Json,
    extract::{Query, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use macro_authorization::{InternalOnly, MacroAuthorizationExtractor};
use uuid::Uuid;

#[derive(serde::Deserialize)]
pub struct GrantRequest {
    attempt_id: Uuid,
    owner: Uuid,
}

/// The verified, non-secret grant a completed attempt produced.
pub async fn grant_handler(
    State(ctx): State<ApiContext>,
    _authorization: MacroAuthorizationExtractor<AuthorizationService, InternalOnly>,
    Query(request): Query<GrantRequest>,
) -> Response {
    let Some(service) = ctx.zoho_auth else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match service
        .completed_grant(request.attempt_id, request.owner)
        .await
    {
        Ok(grant) => ([(header::CACHE_CONTROL, "no-store")], Json(grant)).into_response(),
        Err(ZohoAuthError::InvalidAttempt) => StatusCode::FORBIDDEN.into_response(),
        Err(_) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

#[derive(serde::Deserialize)]
pub struct TokenRequest {
    link_id: Uuid,
    generation: i64,
    sync_generation: i64,
    #[serde(default)]
    force_refresh: bool,
}

/// An access token for an active IMAP link's exact grant binding.
pub async fn access_token_handler(
    State(ctx): State<ApiContext>,
    _authorization: MacroAuthorizationExtractor<AuthorizationService, InternalOnly>,
    Query(request): Query<TokenRequest>,
) -> Response {
    let Some(service) = ctx.zoho_auth else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match service
        .access_token(
            request.link_id,
            request.generation,
            request.sync_generation,
            request.force_refresh,
        )
        .await
    {
        Ok(grant) => (
            [(header::CACHE_CONTROL, "no-store")],
            Json(serde_json::json!({
                "access_token": grant.as_str(),
                "scopes": grant.scopes,
                "data_center": grant.data_center,
                "account_id": grant.account_id,
                "email": grant.email,
            })),
        )
            .into_response(),
        Err(ZohoAuthError::ReauthorizationRequired) => StatusCode::UNAUTHORIZED.into_response(),
        Err(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            [(header::RETRY_AFTER, "2")],
        )
            .into_response(),
    }
}
