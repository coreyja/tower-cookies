use cookie::{Cookie, CookieJar};
use futures_util::ready;
use http::{header, HeaderValue, Request, Response};
use parking_lot::Mutex;
use pin_project::pin_project;
use std::future::Future;
use std::sync::Arc;
use std::{
    pin::Pin,
    task::{Context, Poll},
};
use tower_layer::Layer;
use tower_service::Service;

pub type MutexCookies = Arc<Mutex<Cookies>>;

#[derive(Debug, Default)]
pub struct Cookies {
    header: Option<HeaderValue>,
    jar: Option<CookieJar>,
    changed: bool,
}

impl Cookies {
    fn new(header: Option<HeaderValue>) -> Self {
        Self {
            header,
            ..Default::default()
        }
    }

    pub fn add(&mut self, cookie: Cookie<'static>) {
        self.changed = true;
        self.jar().add(cookie);
    }

    pub fn get(&mut self, name: &str) -> Option<&Cookie<'static>> {
        self.changed = true;
        self.jar().get(name)
    }

    pub fn remove(&mut self, cookie: Cookie<'static>) {
        self.changed = true;
        self.jar().remove(cookie);
    }

    pub fn iter(&mut self) -> cookie::Iter<'_> {
        self.jar().iter()
    }

    /// Cached jar
    fn jar(&mut self) -> &mut CookieJar {
        if self.jar.is_none() {
            let jar = self
                .header
                .as_ref()
                .and_then(|h| std::str::from_utf8(h.as_bytes()).ok())
                .map(|s| jar_from_str(s))
                .unwrap_or_default();
            self.jar = Some(jar);
        }
        self.jar.as_mut().unwrap()
    }
}

fn jar_from_str(s: &str) -> CookieJar {
    let mut jar = CookieJar::new();
    for cookie_str in s.split(';').map(str::trim) {
        if let Ok(cookie) = cookie::Cookie::parse_encoded(cookie_str) {
            jar.add_original(cookie.into_owned());
        }
    }
    jar
}

#[derive(Clone, Debug)]
pub struct CookieService<S> {
    inner: S,
}

impl<S> CookieService<S> {
    pub fn new(inner: S) -> Self {
        Self { inner }
    }
}

impl<ReqBody, ResBody, S> Service<Request<ReqBody>> for CookieService<S>
where
    S: Service<Request<ReqBody>, Response = Response<ResBody>>,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = ResponseFuture<S::Future>;

    #[inline]
    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut req: Request<ReqBody>) -> Self::Future {
        let value = req.headers().get(header::COOKIE).cloned();
        let cookies = Arc::new(Mutex::new(Cookies::new(value)));
        req.extensions_mut().insert(cookies.clone());

        ResponseFuture {
            future: self.inner.call(req),
            cookies: cookies.clone(),
        }
    }
}

/// Response future for [`CookieService`].
#[pin_project]
#[derive(Debug)]
pub struct ResponseFuture<F> {
    #[pin]
    future: F,
    cookies: MutexCookies,
}

impl<F, ResBody, E> Future for ResponseFuture<F>
where
    F: Future<Output = Result<Response<ResBody>, E>>,
{
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();
        let mut res = ready!(this.future.poll(cx)?);

        let mut cookies = this.cookies.lock();
        if cookies.changed {
            let values: Vec<_> = cookies
                .jar()
                .delta()
                .filter_map(|c| HeaderValue::from_str(&c.to_string()).ok())
                .collect();
            let headers = res.headers_mut();
            for value in values {
                headers.append(header::SET_COOKIE, value);
            }
        }

        Poll::Ready(Ok(res))
    }
}

#[derive(Clone, Debug)]
pub struct CookieLayer;

impl<S> Layer<S> for CookieLayer {
    type Service = CookieService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        CookieService { inner }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{Body, BoxBody},
        extract::Extension,
        handler::get,
        routing::BoxRoute,
        Router,
    };
    use tower::ServiceExt;

    fn app() -> Router<BoxRoute> {
        Router::new()
            .route(
                "/list",
                get(|cookies: Extension<MutexCookies>| async move {
                    let mut items = cookies
                        .lock()
                        .iter()
                        .map(|c| format!("{}={}", c.name(), c.value()))
                        .collect::<Vec<_>>();
                    items.sort();
                    items.join(", ")
                }),
            )
            .route(
                "/add",
                get(|cookies: Extension<MutexCookies>| async move {
                    cookies.lock().add(Cookie::new("baz", "3"));
                    cookies.lock().add(Cookie::new("spam", "4"));
                }),
            )
            .route(
                "/remove",
                get(|cookies: Extension<MutexCookies>| async move {
                    cookies.lock().remove(Cookie::new("foo", ""));
                }),
            )
            .layer(CookieLayer)
            .boxed()
    }

    async fn body_string(body: BoxBody) -> String {
        let bytes = hyper::body::to_bytes(body).await.unwrap();
        String::from_utf8_lossy(&bytes).into()
    }

    #[tokio::test]
    async fn read_cookies() {
        let req = Request::builder()
            .uri("/list")
            .header(header::COOKIE, "foo=1; bar=2")
            .body(Body::empty())
            .unwrap();
        let res = app().oneshot(req).await.unwrap();
        assert_eq!(body_string(res.into_body()).await, "bar=2, foo=1");
    }

    #[tokio::test]
    async fn add_cookies() {
        let req = Request::builder()
            .uri("/add")
            .header(header::COOKIE, "foo=1; bar=2")
            .body(Body::empty())
            .unwrap();
        let res = app().oneshot(req).await.unwrap();
        let mut hdrs: Vec<_> = res.headers().get_all(header::SET_COOKIE).iter().collect();
        hdrs.sort();
        assert_eq!(hdrs, ["baz=3", "spam=4"]);
    }

    #[tokio::test]
    async fn remove_cookies() {
        let req = Request::builder()
            .uri("/remove")
            .header(header::COOKIE, "foo=1; bar=2")
            .body(Body::empty())
            .unwrap();
        let res = app().oneshot(req).await.unwrap();
        let mut hdrs = res.headers().get_all(header::SET_COOKIE).iter();
        let hdr = hdrs.next().unwrap().to_str().unwrap();
        assert!(hdr.starts_with("foo=; Max-Age=0; Expires=Tue"));
        assert_eq!(hdrs.next(), None);
    }
}