//!
//! A simple reverse proxy, to be used with [Hyper].
//!
//! The implementation ensures that [Hop-by-hop headers] are stripped correctly in both directions,
//! and adds the client's IP address to a comma-space-separated list of forwarding addresses in the
//! `X-Forwarded-For` header.
//!
//! The implementation is based on Go's [`httputil.ReverseProxy`].
//!
//! [Hyper]: http://hyper.rs/
//! [Hop-by-hop headers]: http://www.w3.org/Protocols/rfc2616/rfc2616-sec13.html
//! [`httputil.ReverseProxy`]: https://golang.org/pkg/net/http/httputil/#ReverseProxy
//!
//! # Example
//!
//! Add these dependencies to your `Cargo.toml` file.
//!
//! ```toml
//! [dependencies]
//! hyper-reverse-proxy = "0.5"
//! hyper = { version = "0.14", features = ["full"] }
//! tokio = { version = "1", features = ["full"] }
//! ```
//!
//! To enable support for connecting to downstream HTTPS servers, enable the `https` feature:
//!
//! ```toml
//! hyper-reverse-proxy = { version = "0.4", features = ["https"] }
//! ```
//!
//! The following example will set up a reverse proxy listening on `127.0.0.1:13900`,
//! and will proxy these calls:
//!
//! * `"/target/first"` will be proxied to `http://127.0.0.1:13901`
//!
//! * `"/target/second"` will be proxied to `http://127.0.0.1:13902`
//!
//! * All other URLs will be handled by `debug_request` function, that will display request information.
//!
//! ```rust,no_run
//! use hyper::server::conn::AddrStream;
//! use hyper::{Body, Request, Response, Server, StatusCode};
//! use hyper::service::{service_fn, make_service_fn};
//! use std::{convert::Infallible, net::SocketAddr};
//! use std::net::IpAddr;
//!
//! fn debug_request(req: Request<Body>) -> Result<Response<Body>, Infallible>  {
//!     let body_str = format!("{:?}", req);
//!     Ok(Response::new(Body::from(body_str)))
//! }
//!
//! async fn handle(client_ip: IpAddr, req: Request<Body>) -> Result<Response<Body>, Infallible> {
//!     if req.uri().path().starts_with("/target/first") {
//!         // will forward requests to port 13901
//!         match hyper_reverse_proxy::call(client_ip, "http://127.0.0.1:13901", req).await {
//!             Ok(response) => {Ok(response)}
//!             Err(_error) => {Ok(Response::builder()
//!                                   .status(StatusCode::INTERNAL_SERVER_ERROR)
//!                                   .body(Body::empty())
//!                                   .unwrap())}
//!         }
//!     } else if req.uri().path().starts_with("/target/second") {
//!         // will forward requests to port 13902
//!         match hyper_reverse_proxy::call(client_ip, "http://127.0.0.1:13902", req).await {
//!             Ok(response) => {Ok(response)}
//!             Err(_error) => {Ok(Response::builder()
//!                                   .status(StatusCode::INTERNAL_SERVER_ERROR)
//!                                   .body(Body::empty())
//!                                   .unwrap())}
//!         }
//!     } else {
//!         debug_request(req)
//!     }
//! }
//!
//! #[tokio::main]
//! async fn main() {
//!     let bind_addr = "127.0.0.1:8000";
//!     let addr:SocketAddr = bind_addr.parse().expect("Could not parse ip:port.");
//!
//!     let make_svc = make_service_fn(|conn: &AddrStream| {
//!         let remote_addr = conn.remote_addr().ip();
//!         async move {
//!             Ok::<_, Infallible>(service_fn(move |req| handle(remote_addr, req)))
//!         }
//!     });
//!
//!     let server = Server::bind(&addr).serve(make_svc);
//!
//!     println!("Running server on {:?}", addr);
//!
//!     if let Err(e) = server.await {
//!         eprintln!("server error: {}", e);
//!     }
//! }
//! ```
//!

use hyper::client::{connect::dns::GaiResolver, HttpConnector};
use hyper::header::{HeaderMap, HeaderValue, HeaderName, HOST};
use hyper::http::header::{InvalidHeaderValue, ToStrError};
use hyper::http::uri::InvalidUri;
use hyper::{Body, Client, Error, Request, Response};
use lazy_static::lazy_static;
use std::net::IpAddr;

lazy_static! {
    // A list of the headers, using hypers actual HeaderName comparison
    static ref HOP_HEADERS: [HeaderName; 8] = [
        HeaderName::from_static("connection"),
        HeaderName::from_static("keep-alive"),
        HeaderName::from_static("proxy-authenticate"),
        HeaderName::from_static("proxy-authorization"),
        HeaderName::from_static("te"),
        HeaderName::from_static("trailers"),
        HeaderName::from_static("transfer-encoding"),
        HeaderName::from_static("upgrade"),
    ];
    
    static ref X_FORWARDED_FOR: HeaderName = HeaderName::from_static("x-forwarded-for");
}

#[derive(Debug)]
pub enum ProxyError {
    InvalidUri(InvalidUri),
    HyperError(Error),
    ForwardHeaderError,
}

impl From<Error> for ProxyError {
    fn from(err: Error) -> ProxyError {
        ProxyError::HyperError(err)
    }
}

impl From<InvalidUri> for ProxyError {
    fn from(err: InvalidUri) -> ProxyError {
        ProxyError::InvalidUri(err)
    }
}

impl From<ToStrError> for ProxyError {
    fn from(_err: ToStrError) -> ProxyError {
        ProxyError::ForwardHeaderError
    }
}

impl From<InvalidHeaderValue> for ProxyError {
    fn from(_err: InvalidHeaderValue) -> ProxyError {
        ProxyError::ForwardHeaderError
    }
}

fn is_hop_header(name: &str) -> bool {
    HOP_HEADERS.iter().any(|h| h == &name)
}

/// Returns a clone of the headers without the [hop-by-hop headers].
///
/// [hop-by-hop headers]: http://www.w3.org/Protocols/rfc2616/rfc2616-sec13.html
fn remove_hop_headers(headers: &HeaderMap<HeaderValue>) -> HeaderMap<HeaderValue> {
    let mut result = HeaderMap::new();
    for (k, v) in headers.iter() {
        if !is_hop_header(k.as_str()) {
            result.insert(k.clone(), v.clone());
        }
    }
    result
}

fn create_proxied_response<B>(mut response: Response<B>) -> Response<B> {
    *response.headers_mut() = remove_hop_headers(response.headers());
    response
}


fn forward_uri<B>(forward_url: &str, req: &Request<B>) -> String {
    if let Some(query) = req.uri().query() {
        let mut forwarding_uri = String::with_capacity(forward_url.len() + req.uri().path().len() + query.len() + 1);

        forwarding_uri.push_str(forward_url);
        forwarding_uri.push_str(req.uri().path());
        forwarding_uri.push('?');
        forwarding_uri.push_str(query);

        forwarding_uri
    } else {
        let mut forwarding_uri = String::with_capacity(forward_url.len() + req.uri().path().len());

        forwarding_uri.push_str(forward_url);
        forwarding_uri.push_str(req.uri().path());

        forwarding_uri
    }
}

fn create_proxied_request<B>(
    client_ip: IpAddr,
    forward_url: &str,
    mut request: Request<B>,
) -> Result<Request<B>, ProxyError> {
    let uri: hyper::Uri = forward_uri(forward_url, &request).parse()?;
    
    request
        .headers_mut()
        .insert(HOST, HeaderValue::from_str(uri.host().unwrap())?);

    *request.uri_mut() = uri;

    *request.headers_mut() = remove_hop_headers(request.headers());

    // Add forwarding information in the headers
    match request.headers_mut().entry(&*X_FORWARDED_FOR) {
        hyper::header::Entry::Vacant(entry) => {
            entry.insert(client_ip.to_string().parse()?);
        }

        hyper::header::Entry::Occupied(mut entry) => {
            let addr = format!("{}, {}", entry.get().to_str()?, client_ip);
            entry.insert(addr.parse()?);
        }
    }

    Ok(request)
}

#[cfg(feature = "https")]
fn build_client() -> Client<hyper_tls::HttpsConnector<HttpConnector<GaiResolver>>, hyper::Body> {
    let https = hyper_tls::HttpsConnector::new();
    Client::builder().build::<_, hyper::Body>(https)
}

#[cfg(not(feature = "https"))]
fn build_client() -> Client<HttpConnector<GaiResolver>, hyper::Body> {
    Client::new()
}

pub async fn call(
    client_ip: IpAddr,
    forward_uri: &str,
    request: Request<Body>,
) -> Result<Response<Body>, ProxyError> {
    let proxied_request = create_proxied_request(client_ip, forward_uri, request)?;

    let client = build_client();
    let response = client.request(proxied_request).await?;
    let proxied_response = create_proxied_response(response);
    Ok(proxied_response)
}
