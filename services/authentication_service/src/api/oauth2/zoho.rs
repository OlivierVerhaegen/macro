use crate::api::{
    context::ApiContext,
    oauth2::{OAuthState, account_link::build_callback_redirect},
};
use crate::domain::zoho::ZohoAuthError;
use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use model::response::ErrorResponse;

pub(in crate::api::oauth2) async fn handler(
    ctx: &ApiContext,
    code: &str,
    state: &OAuthState,
) -> Result<Response, Response> {
    let id = state
        .link_id
        .ok_or_else(|| error_response(ZohoAuthError::InvalidAttempt))?;
    let service = ctx
        .zoho_auth
        .as_ref()
        .ok_or_else(|| error_response(ZohoAuthError::NotConfigured))?;
    let return_uri = service
        .complete_link(id, code)
        .await
        .map_err(error_response)?;
    match return_uri {
        Some(uri) => build_callback_redirect(&uri, &id).map_err(IntoResponse::into_response),
        None => Ok(StatusCode::OK.into_response()),
    }
}

fn error_response(error: ZohoAuthError) -> Response {
    let status = match error {
        ZohoAuthError::InvalidAttempt | ZohoAuthError::InvalidIdentity => StatusCode::BAD_REQUEST,
        ZohoAuthError::OwnershipConflict => StatusCode::CONFLICT,
        ZohoAuthError::NotConfigured => StatusCode::NOT_FOUND,
        _ => StatusCode::SERVICE_UNAVAILABLE,
    };
    (
        status,
        Json(ErrorResponse {
            message: error.to_string().into(),
        }),
    )
        .into_response()
}
