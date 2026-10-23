use crate::api::context::{ApiContext, AuthorizationService};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};
use email::domain::mailbox::smtp_password::SmtpPasswordError;
use macro_authorization::{MacroAuthorizationExtractor, UserOrInternal};
use model::response::ErrorResponse;
use utoipa::ToSchema;
use uuid::Uuid;
use zeroize::Zeroizing;

/// The SMTP password of an IMAP inbox, such as a Zoho app password.
/// Secret-bearing request intentionally does not implement Debug.
#[derive(serde::Deserialize, ToSchema)]
pub struct SetSmtpPasswordRequest {
    pub password: String,
}

#[derive(Debug, thiserror::Error)]
#[error(transparent)]
pub struct SetSmtpPasswordError(SmtpPasswordError);

impl IntoResponse for SetSmtpPasswordError {
    fn into_response(self) -> Response {
        let status = match self.0 {
            SmtpPasswordError::NotFound => StatusCode::NOT_FOUND,
            SmtpPasswordError::Rejected => StatusCode::UNPROCESSABLE_ENTITY,
            SmtpPasswordError::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
        };
        (
            status,
            Json(ErrorResponse {
                message: self.0.to_string().into(),
            }),
        )
            .into_response()
    }
}

/// Sets the password an IMAP inbox sends with. The mail server must accept it
/// first; it is then kept encrypted by the authentication service.
#[utoipa::path(
    put,
    tag = "Links",
    path = "/email/links/{link_id}/smtp_password",
    operation_id = "set_link_smtp_password",
    params(
        ("link_id" = Uuid, Path, description = "Inbox link ID."),
    ),
    request_body = SetSmtpPasswordRequest,
    responses(
            (status = 204),
            (status = 401, body=ErrorResponse),
            (status = 404, body=ErrorResponse),
            (status = 422, body=ErrorResponse),
            (status = 503, body=ErrorResponse),
    )
)]
#[tracing::instrument(skip_all, fields(%link_id, user_id=authorization.authorization.user.user_context.user_id), err)]
pub async fn set_smtp_password_handler(
    State(ctx): State<ApiContext>,
    authorization: MacroAuthorizationExtractor<AuthorizationService, UserOrInternal>,
    Path(link_id): Path<Uuid>,
    Json(request): Json<SetSmtpPasswordRequest>,
) -> Result<StatusCode, SetSmtpPasswordError> {
    ctx.smtp_passwords
        .set(
            &authorization.authorization.user.macro_user_id,
            link_id,
            Zeroizing::new(request.password),
        )
        .await
        .map_err(SetSmtpPasswordError)?;
    Ok(StatusCode::NO_CONTENT)
}
