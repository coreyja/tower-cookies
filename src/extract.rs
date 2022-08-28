use crate::Cookies;
use async_trait::async_trait;
use axum_core::extract::FromRequestParts;
use http::{request::Parts, StatusCode};

#[async_trait]
impl<State> FromRequestParts<State> for Cookies
where
    State: Send + Sync,
{
    type Rejection = (http::StatusCode, &'static str);

    async fn from_request_parts(
        parts: &mut Parts,
        _state: &State,
    ) -> Result<Self, Self::Rejection> {
        parts.extensions.get::<Cookies>().cloned().ok_or((
            StatusCode::INTERNAL_SERVER_ERROR,
            "Can't extract cookies. Is `CookieManagerLayer` enabled?",
        ))
    }
}
