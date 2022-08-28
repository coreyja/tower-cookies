use crate::Cookies;
use async_trait::async_trait;
use axum_core::extract::FromRequest;
use http::{Request, StatusCode};

#[async_trait]
impl<State, Body> FromRequest<State, Body> for Cookies
where
    Body: Send + 'static,
    State: Send + Sync,
{
    type Rejection = (http::StatusCode, &'static str);

    async fn from_request(req: Request<Body>, _state: &State) -> Result<Self, Self::Rejection> {
        req.extensions().get::<Cookies>().cloned().ok_or((
            StatusCode::INTERNAL_SERVER_ERROR,
            "Can't extract cookies. Is `CookieManagerLayer` enabled?",
        ))
    }
}
