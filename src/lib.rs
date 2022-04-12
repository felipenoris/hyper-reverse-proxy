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
#![cfg_attr(all(not(stable), test), feature(test))]

#[cfg(all(not(stable), test))]
extern crate test;

use hyper::client::{connect::dns::GaiResolver, HttpConnector};
use hyper::header::{HeaderName, HeaderValue, HOST};
use hyper::http::header::{InvalidHeaderValue, ToStrError};
use hyper::http::uri::InvalidUri;
use hyper::{Body, Client, Error, HeaderMap, Request, Response};
use lazy_static::lazy_static;
use std::net::IpAddr;

lazy_static! {
    static ref TE_HEADER: HeaderName = HeaderName::from_static("te");
    static ref CONNECTION_HEADER: HeaderName = HeaderName::from_static("connection");
    static ref UPGRADE_HEADER: HeaderName = HeaderName::from_static("upgrade");
    // A list of the headers, using hypers actual HeaderName comparison
    static ref HOP_HEADERS: [HeaderName; 8] = [
        CONNECTION_HEADER.clone(),
        TE_HEADER.clone(),
        HeaderName::from_static("keep-alive"),
        HeaderName::from_static("proxy-authenticate"),
        HeaderName::from_static("proxy-authorization"),
        HeaderName::from_static("trailer"),
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

fn remove_hop_headers(headers: &mut HeaderMap) {
    for header in &*HOP_HEADERS {
        headers.remove(header);
    }
}

fn get_upgrade_type(headers: &HeaderMap) -> Option<String> {
    if headers
        .get(&*CONNECTION_HEADER)
        .map(|value| {
            value
                .to_str()
                .unwrap()
                .split(",")
                .any(|e| e.to_lowercase() == "upgrade")
        })
        .unwrap_or(false)
    {
        if let Some(upgrade_value) = headers.get(&*UPGRADE_HEADER) {
            return Some(upgrade_value.to_str().unwrap().to_owned());
        }
    }
    None
}

fn remove_connection_headers(headers: &mut HeaderMap) {
    if headers.get(&*CONNECTION_HEADER).is_some() {
        let value = headers.get(&*CONNECTION_HEADER).map(|e| e.clone()).unwrap();

        for name in value.to_str().unwrap().split(",") {
            if !name.trim().is_empty() {
                headers.remove(name.trim());
            }
        }
    }
}

fn create_proxied_response<B>(mut response: Response<B>) -> Response<B> {
    remove_hop_headers(response.headers_mut());
    remove_connection_headers(response.headers_mut());

    response
}

fn forward_uri<B>(forward_url: &str, req: &Request<B>) -> String {
    if let Some(query) = req.uri().query() {
        let mut forwarding_uri =
            String::with_capacity(forward_url.len() + req.uri().path().len() + query.len() + 1);

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
    let contains_te_trailers_value = request
        .headers()
        .get(&*TE_HEADER)
        .map(|value| {
            value
                .to_str()
                .unwrap()
                .split(",")
                .any(|e| e.to_lowercase() == "trailers")
        })
        .unwrap_or(false);
    let upgrade_type = get_upgrade_type(request.headers());

    let uri: hyper::Uri = forward_uri(forward_url, &request).parse()?;
    request
        .headers_mut()
        .insert(HOST, HeaderValue::from_str(uri.host().unwrap())?);

    *request.uri_mut() = uri;

    remove_hop_headers(request.headers_mut());
    remove_connection_headers(request.headers_mut());

    if contains_te_trailers_value {
        request
            .headers_mut()
            .insert(&*TE_HEADER, HeaderValue::from_static("trailers"));
    }

    if let Some(value) = upgrade_type {
        request
            .headers_mut()
            .insert(&*UPGRADE_HEADER, value.parse().unwrap());
        request
            .headers_mut()
            .insert(&*CONNECTION_HEADER, HeaderValue::from_static("UPGRADE"));
    }

    // Add forwarding information in the headers
    match request.headers_mut().entry(&*X_FORWARDED_FOR) {
        hyper::header::Entry::Vacant(entry) => {
            entry.insert(client_ip.to_string().parse()?);
        }

        hyper::header::Entry::Occupied(entry) => {
            let client_ip_str = client_ip.to_string();
            let mut addr =
                String::with_capacity(entry.get().as_bytes().len() + 2 + client_ip_str.len());

            addr.push_str(std::str::from_utf8(entry.get().as_bytes()).unwrap());
            addr.push(',');
            addr.push(' ');
            addr.push_str(&client_ip_str);
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

#[cfg(all(not(stable), test))]
mod tests {
    use hyper::header::HeaderName;
    use hyper::Uri;
    use hyper::{HeaderMap, Request, Response};
    use rand::distributions::Alphanumeric;
    use rand::prelude::*;
    use std::net::Ipv4Addr;
    use std::str::FromStr;
    use test::Bencher;
    use test_context::AsyncTestContext;
    use tokiotest_httpserver::HttpTestContext;

    fn generate_string() -> String {
        let take = rand::thread_rng().gen::<u8>().into();
        rand::thread_rng()
            .sample_iter(&Alphanumeric)
            .take(take)
            .map(char::from)
            .collect()
    }

    fn build_headers() -> HeaderMap {
        let mut headers_map: HeaderMap = (&*super::HOP_HEADERS)
            .iter()
            .map(|el: &'static HeaderName| (el.clone(), generate_string().parse().unwrap()))
            .collect();

        for _i in 0..20 {
            'inserted: loop {
                if let Ok(value) =
                    hyper::header::HeaderName::from_str(&generate_string().to_lowercase())
                {
                    headers_map.insert(value, generate_string().parse().unwrap());

                    break 'inserted;
                }
            }
        }

        headers_map
    }

    #[bench]
    fn proxy_call(b: &mut Bencher) {
        use tokio::runtime::Runtime;
        let rt = Runtime::new().unwrap();

        let uri = Uri::from_static("http://0.0.0.0:8080/me?hello=world");

        let http_context: HttpTestContext = rt.block_on(async { AsyncTestContext::setup().await });

        let forward_url = &format!("http://0.0.0.0:{}", http_context.port);

        let headers_map = build_headers();

        let client_ip = std::net::IpAddr::from(Ipv4Addr::from_str("0.0.0.0").unwrap());

        b.iter(|| {
            rt.block_on(async {
                let mut request = Request::builder().uri(uri.clone());

                *request.headers_mut().unwrap() = headers_map.clone();

                super::call(
                    client_ip,
                    forward_url,
                    request.body(hyper::Body::from("")).unwrap(),
                )
                .await
                .unwrap();
            })
        });
    }

    #[bench]
    fn create_proxied_response(b: &mut Bencher) {
        let headers_map = build_headers();

        b.iter(|| {
            let mut response = Response::builder().status(200);

            *response.headers_mut().unwrap() = headers_map.clone();

            super::create_proxied_response(
                response.body(()).unwrap(),
                hyper::header::HeaderValue::from_static("me"),
            );
        });
    }

    #[bench]
    fn forward_url_with_str_ending_slash(b: &mut Bencher) {
        let uri = Uri::from_static("http://0.0.0.0:8080/me");
        let port = rand::thread_rng().gen::<u8>();
        let forward_url = &format!("http://0.0.0.0:{}/", port);

        b.iter(|| {
            let request = Request::builder().uri(uri.clone()).body(());

            super::forward_uri(forward_url, &request.unwrap());
        });
    }

    #[bench]
    fn forward_url_with_str_ending_slash_and_query(b: &mut Bencher) {
        let uri = Uri::from_static("http://0.0.0.0:8080/me?hello=world");
        let port = rand::thread_rng().gen::<u8>();
        let forward_url = &format!("http://0.0.0.0:{}/", port);

        b.iter(|| {
            let request = Request::builder().uri(uri.clone()).body(());

            super::forward_uri(forward_url, &request.unwrap());
        });
    }

    #[bench]
    fn forward_url_no_ending_slash(b: &mut Bencher) {
        let uri = Uri::from_static("http://0.0.0.0:8080/me");
        let port = rand::thread_rng().gen::<u8>();
        let forward_url = &format!("http://0.0.0.0:{}", port);

        b.iter(|| {
            let request = Request::builder().uri(uri.clone()).body(());

            super::forward_uri(forward_url, &request.unwrap());
        });
    }

    #[bench]
    fn forward_url_with_query(b: &mut Bencher) {
        let uri = Uri::from_static("http://0.0.0.0:8080/me?hello=world");
        let port = rand::thread_rng().gen::<u8>();
        let forward_url = &format!("http://0.0.0.0:{}", port);

        b.iter(|| {
            let request = Request::builder().uri(uri.clone()).body(());

            super::forward_uri(forward_url, &request.unwrap());
        });
    }

    #[bench]
    fn create_proxied_request_forwarded_for_occupied(b: &mut Bencher) {
        let uri = Uri::from_static("http://0.0.0.0:8080/me?hello=world");
        let port = rand::thread_rng().gen::<u8>();
        let forward_url = &format!("http://0.0.0.0:{}", port);

        let mut headers_map = build_headers();

        headers_map.insert(
            HeaderName::from_static("x-forwarded-for"),
            "0.0.0.0".parse().unwrap(),
        );

        let client_ip = std::net::IpAddr::from(Ipv4Addr::from_str("0.0.0.0").unwrap());

        b.iter(|| {
            let mut request = Request::builder().uri(uri.clone());

            *request.headers_mut().unwrap() = headers_map.clone();

            super::create_proxied_request(client_ip, forward_url, request.body(()).unwrap())
                .unwrap();
        });
    }

    #[bench]
    fn create_proxied_request_forwarded_for_vacant(b: &mut Bencher) {
        let uri = Uri::from_static("http://0.0.0.0:8080/me?hello=world");
        let port = rand::thread_rng().gen::<u8>();
        let forward_url = &format!("http://0.0.0.0:{}", port);

        let headers_map = build_headers();

        let client_ip = std::net::IpAddr::from(Ipv4Addr::from_str("0.0.0.0").unwrap());

        b.iter(|| {
            let mut request = Request::builder().uri(uri.clone());

            *request.headers_mut().unwrap() = headers_map.clone();

            super::create_proxied_request(client_ip, forward_url, request.body(()).unwrap())
                .unwrap();
        });
    }
}
