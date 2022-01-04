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
//! hyper = "0.13"
//! tokio = { version = "0.2", features = ["full"] }
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
//! use hyper::http::uri::InvalidUri;
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
//!             Err(error) => {Ok(Response::builder()
//!                                   .status(StatusCode::INTERNAL_SERVER_ERROR)
//!                                   .body(Body::empty())
//!                                   .unwrap())}
//!         }
//!     } else if req.uri().path().starts_with("/target/second") {
//!
//!         // will forward requests to port 13902
//!         match hyper_reverse_proxy::call(client_ip, "http://127.0.0.1:13902", req).await {
//!             Ok(response) => {Ok(response)}
//!             Err(error) => {Ok(Response::builder()
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
//!     if let Err(e) = server.await {
//!         eprintln!("server error: {}", e);
//!     }
//!
//!     println!("Running server on {:?}", addr);
//! }
//! ```
//!

use hyper::header::{HeaderMap, HeaderValue};
use hyper::http::header::{InvalidHeaderValue, ToStrError};
use hyper::http::uri::InvalidUri;
use hyper::{Body, Client, Error, Request, Response, Uri};
use lazy_static::lazy_static;
use std::net::IpAddr;
use std::str::FromStr;

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
    use unicase::Ascii;

    // A list of the headers, using `unicase` to help us compare without
    // worrying about the case, and `lazy_static!` to prevent reallocation
    // of the vector.
    lazy_static! {
        static ref HOP_HEADERS: Vec<Ascii<&'static str>> = vec![
            Ascii::new("Connection"),
            Ascii::new("Keep-Alive"),
            Ascii::new("Proxy-Authenticate"),
            Ascii::new("Proxy-Authorization"),
            Ascii::new("Te"),
            Ascii::new("Trailers"),
            Ascii::new("Transfer-Encoding"),
            Ascii::new("Upgrade"),
        ];
    }

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

fn forward_uri<B>(forward_url: &str, req: &Request<B>) -> Result<Uri, InvalidUri> {
    let forward_uri = match req.uri().query() {
        Some(query) => format!("{}{}?{}", forward_url, req.uri().path(), query),
        None => format!("{}{}", forward_url, req.uri().path()),
    };

    Uri::from_str(forward_uri.as_str())
}

fn create_proxied_request<B>(
    client_ip: IpAddr,
    forward_url: &str,
    mut request: Request<B>,
) -> Result<Request<B>, ProxyError> {
    *request.headers_mut() = remove_hop_headers(request.headers());
    *request.uri_mut() = forward_uri(forward_url, &request)?;

    let x_forwarded_for_header_name = "x-forwarded-for";

    // Add forwarding information in the headers
    match request.headers_mut().entry(x_forwarded_for_header_name) {
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

pub async fn call(
    client_ip: IpAddr,
    forward_uri: &str,
    request: Request<Body>,
) -> Result<Response<Body>, ProxyError> {
    let proxied_request = create_proxied_request(client_ip, &forward_uri, request)?;

    let client = Client::new();
    let response = client.request(proxied_request).await?;
    let proxied_response = create_proxied_response(response);
    Ok(proxied_response)
}
