//! Zoho Mail account linking (Olive Trees fork).

use crate::domain::zoho::ZohoAuthError;
use axum::{
    Json,
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use macro_authorization::{MacroAuthorizationExtractor, UserOrInternal};
use model::response::ErrorResponse;
use serde_utils::urlencode::UrlEncoded;
use url::Url;
use zoho_client::ZohoDataCenter;

use crate::api::{
    context::{ApiContext, AuthorizationService},
    oauth2::format_redirect_uri,
};

/// Response returned when a Zoho Mail link is initiated.
#[derive(Debug, serde::Deserialize, serde::Serialize, utoipa::ToSchema)]
pub struct InitZohoLinkResponse {
    /// The Zoho consent URL to redirect the user to.
    pub authorization_url: String,
    /// The link ID for tracking the OAuth flow.
    pub link_id: uuid::Uuid,
}

/// Errors that can occur while initiating a Zoho Mail link.
#[derive(Debug, thiserror::Error)]
pub enum InitZohoLinkError {
    #[error("a professional subscription is required to link an additional inbox")]
    PaymentRequired,
    #[error("invalid return URL")]
    InvalidReturnUrl,
    #[error("too many in progress links")]
    TooManyInProgressLinks,
    #[error("Zoho linking is not configured")]
    NotConfigured,
    #[error("internal error occurred")]
    Internal(ZohoAuthError),
}

impl IntoResponse for InitZohoLinkError {
    fn into_response(self) -> Response {
        let status_code = match &self {
            Self::PaymentRequired => StatusCode::PAYMENT_REQUIRED,
            Self::InvalidReturnUrl => StatusCode::BAD_REQUEST,
            Self::TooManyInProgressLinks => StatusCode::TOO_MANY_REQUESTS,
            Self::NotConfigured => StatusCode::NOT_FOUND,
            Self::Internal(error) => {
                tracing::error!(error=?error, "failed to initiate Zoho link");
                StatusCode::INTERNAL_SERVER_ERROR
            }
        };
        (
            status_code,
            Json(ErrorResponse {
                message: self.to_string().into(),
            }),
        )
            .into_response()
    }
}

#[derive(Debug, serde::Deserialize)]
pub(crate) struct InitZohoLinkQueryParams {
    data_center: ZohoDataCenter,
    reconnect_link_id: Option<uuid::Uuid>,
    original_url: Option<UrlEncoded<Url>>,
}

/// Initiates a Zoho Mail account link for an authenticated user.
#[utoipa::path(
    post,
    operation_id = "init_zoho_link",
    path = "/link/zoho",
    params(
        ("data_center" = String, Query, description = "The Zoho data center of the account: com, eu, in, com.au, jp, ca, sa or com.cn."),
        ("original_url" = Option<String>, Query, description = "**OPTIONAL**. The original URL to redirect to."),
        ("reconnect_link_id" = Option<uuid::Uuid>, Query, description = "Existing accessible inbox being reconnected.")
    ),
    responses(
        (status = 200, body = InitZohoLinkResponse),
        (status = 400, body = ErrorResponse),
        (status = 401, body = ErrorResponse),
        (status = 402, body = ErrorResponse),
        (status = 404, body = ErrorResponse),
        (status = 429, body = ErrorResponse),
        (status = 500, body = ErrorResponse),
    )
)]
#[tracing::instrument(skip_all, fields(user_id=%authorization.authorization.user.user_context.user_id, data_center=%params.data_center), err)]
pub async fn init_zoho_link_handler(
    State(ctx): State<ApiContext>,
    Query(params): Query<InitZohoLinkQueryParams>,
    authorization: MacroAuthorizationExtractor<AuthorizationService, UserOrInternal>,
) -> Result<Json<InitZohoLinkResponse>, InitZohoLinkError> {
    let service = ctx
        .zoho_auth
        .as_ref()
        .ok_or(InitZohoLinkError::NotConfigured)?;
    let original_url = params.original_url.map(|url| url.0);
    if original_url
        .as_ref()
        .is_some_and(|url| !crate::api::login::sso::is_allowed_original_url(url))
    {
        return Err(InitZohoLinkError::InvalidReturnUrl);
    }
    let owner =
        uuid::Uuid::parse_str(&authorization.authorization.user.user_context.fusion_user_id)
            .map_err(|_| InitZohoLinkError::Internal(ZohoAuthError::InvalidIdentity))?;
    let started = service
        .start_link(
            owner,
            params.data_center,
            format_redirect_uri("zoho"),
            original_url.map(String::from),
            params.reconnect_link_id,
        )
        .await
        .map_err(|error| match error {
            ZohoAuthError::PaymentRequired => InitZohoLinkError::PaymentRequired,
            ZohoAuthError::NotConfigured => InitZohoLinkError::NotConfigured,
            ZohoAuthError::TooManyAttempts => InitZohoLinkError::TooManyInProgressLinks,
            error => InitZohoLinkError::Internal(error),
        })?;
    Ok(Json(InitZohoLinkResponse {
        authorization_url: started.authorization_url,
        link_id: started.id,
    }))
}
