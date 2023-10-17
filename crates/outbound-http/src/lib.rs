pub mod allowed_http_hosts;
#[cfg(feature = "runtime")]
mod host_component;

use anyhow::Result;
use http::HeaderMap;
#[cfg(feature = "runtime")]
use reqwest::{Client, Url};
#[cfg(feature = "runtime")]
use spin_app::MetadataKey;
#[cfg(feature = "runtime")]
use spin_core::async_trait;
#[cfg(feature = "runtime")]
use spin_world::v1::{
    http as outbound_http,
    http_types::{Headers, HttpError, Method, Request, Response},
};

use allowed_http_hosts::AllowedHttpHosts;
#[cfg(feature = "runtime")]
pub use host_component::OutboundHttpComponent;

pub const ALLOWED_HTTP_HOSTS_KEY: MetadataKey<Vec<String>> = MetadataKey::new("allowed_http_hosts");

/// A very simple implementation for outbound HTTP requests.
#[cfg(feature = "runtime")]
#[derive(Default, Clone)]
pub struct OutboundHttp {
    /// List of hosts guest modules are allowed to make requests to.
    pub allowed_hosts: AllowedHttpHosts,
    /// During an incoming HTTP request, origin is set to the host of that incoming HTTP request.
    /// This is used to direct outbound requests to the same host when allowed.
    pub origin: String,
    client: Option<Client>,
}

#[cfg(feature = "runtime")]
impl OutboundHttp {
    /// Check if guest module is allowed to send request to URL, based on the list of
    /// allowed hosts defined by the runtime. If the url passed in is a relative path,
    /// only allow if allowed_hosts contains `self`. If the list of allowed hosts contains
    /// `insecure:allow-all`, then all hosts are allowed.
    /// If `None` is passed, the guest module is not allowed to send the request.
    fn is_allowed(&mut self, url: &str) -> Result<bool, HttpError> {
        if url.starts_with('/') {
            return Ok(self.allowed_hosts.allow_relative_url());
        }

        let url = Url::parse(url).map_err(|_| HttpError::InvalidUrl)?;
        Ok(self.allowed_hosts.allow(&url))
    }
}

#[cfg(feature = "runtime")]
#[async_trait]
impl outbound_http::Host for OutboundHttp {
    async fn send_request(&mut self, req: Request) -> Result<Result<Response, HttpError>> {
        Ok(async {
            tracing::log::trace!("Attempting to send outbound HTTP request to {}", req.uri);
            if !self
                .is_allowed(&req.uri)
                .map_err(|_| HttpError::RuntimeError)?
            {
                tracing::log::info!("Destination not allowed: {}", req.uri);
                if let Some(host) = host(&req.uri) {
                    terminal::warn!("A component tried to make a HTTP request to non-allowed host '{host}'.");
                    eprintln!("To allow requests, add 'allowed_http_hosts = [\"{host}\"]' to the manifest component section.");
                }
                return Err(HttpError::DestinationNotAllowed);
            }

            let method = method_from(req.method);

            let abs_url = if req.uri.starts_with('/') {
                format!("{}{}", self.origin, req.uri)
            } else {
                req.uri.clone()
            };

            let req_url = Url::parse(&abs_url).map_err(|_| HttpError::InvalidUrl)?;

            let headers = request_headers(req.headers).map_err(|_| HttpError::RuntimeError)?;
            let body = req.body.unwrap_or_default().to_vec();

            if !req.params.is_empty() {
                tracing::log::warn!("HTTP params field is deprecated");
            }

            // Allow reuse of Client's internal connection pool for multiple requests
            // in a single component execution
            let client = self.client.get_or_insert_with(Default::default);

            let resp = client
                .request(method, req_url)
                .headers(headers)
                .body(body)
                .send()
                .await
                .map_err(log_reqwest_error)?;
            tracing::log::trace!("Returning response from outbound request to {}", req.uri);
            response_from_reqwest(resp).await
        }
        .await)
    }
}

#[cfg(feature = "runtime")]
fn log_reqwest_error(err: reqwest::Error) -> HttpError {
    let error_desc = if err.is_timeout() {
        "timeout error"
    } else if err.is_connect() {
        "connection error"
    } else if err.is_body() || err.is_decode() {
        "message body error"
    } else if err.is_request() {
        "request error"
    } else {
        "error"
    };
    tracing::warn!(
        "Outbound HTTP {}: URL {}, error detail {:?}",
        error_desc,
        err.url()
            .map(|u| u.to_string())
            .unwrap_or_else(|| "<unknown>".to_owned()),
        err
    );
    HttpError::RuntimeError
}

#[cfg(feature = "runtime")]
fn method_from(m: Method) -> http::Method {
    match m {
        Method::Get => http::Method::GET,
        Method::Post => http::Method::POST,
        Method::Put => http::Method::PUT,
        Method::Delete => http::Method::DELETE,
        Method::Patch => http::Method::PATCH,
        Method::Head => http::Method::HEAD,
        Method::Options => http::Method::OPTIONS,
    }
}

#[cfg(feature = "runtime")]
async fn response_from_reqwest(res: reqwest::Response) -> Result<Response, HttpError> {
    let status = res.status().as_u16();
    let headers = response_headers(res.headers()).map_err(|_| HttpError::RuntimeError)?;

    let body = Some(
        res.bytes()
            .await
            .map_err(|_| HttpError::RuntimeError)?
            .to_vec(),
    );

    Ok(Response {
        status,
        headers,
        body,
    })
}

#[cfg(feature = "runtime")]
fn request_headers(h: Headers) -> anyhow::Result<HeaderMap> {
    let mut res = HeaderMap::new();
    for (k, v) in h {
        res.insert(
            http::header::HeaderName::try_from(k)?,
            http::header::HeaderValue::try_from(v)?,
        );
    }
    Ok(res)
}

#[cfg(feature = "runtime")]
fn response_headers(h: &HeaderMap) -> anyhow::Result<Option<Vec<(String, String)>>> {
    let mut res: Vec<(String, String)> = vec![];

    for (k, v) in h {
        res.push((
            k.to_string(),
            std::str::from_utf8(v.as_bytes())?.to_string(),
        ));
    }

    Ok(Some(res))
}

#[cfg(feature = "runtime")]
fn host(url: &str) -> Option<String> {
    url::Url::parse(url)
        .ok()
        .and_then(|u| u.host().map(|h| h.to_string()))
}
