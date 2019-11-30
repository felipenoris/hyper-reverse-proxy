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
//! hyper-reverse-proxy = "0.4"
//! hyper = "0.12"
//! futures = "0.1"
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
//! use hyper::{Body, Request, Response, Server};
//! use hyper::service::{service_fn, make_service_fn};
//! use futures::future::{self, Future};
//!
//! type BoxFut = Box<Future<Item=Response<Body>, Error=hyper::Error> + Send>;
//!
//! fn debug_request(req: Request<Body>) -> BoxFut {
//!     let body_str = format!("{:?}", req);
//!     let response = Response::new(Body::from(body_str));
//!     Box::new(future::ok(response))
//! }
//!
//! fn main() {
//!
//!     // This is our socket address...
//!     let addr = ([127, 0, 0, 1], 13900).into();
//!
//!     // A `Service` is needed for every connection.
//!     let make_svc = make_service_fn(|socket: &AddrStream| {
//!         let remote_addr = socket.remote_addr();
//!         service_fn(move |req: Request<Body>| { // returns BoxFut
//!
//!             if req.uri().path().starts_with("/target/first") {
//!
//!                 // will forward requests to port 13901
//!                 return hyper_reverse_proxy::call(remote_addr.ip(), "http://127.0.0.1:13901", req)
//!
//!             } else if req.uri().path().starts_with("/target/second") {
//!
//!                 // will forward requests to port 13902
//!                 return hyper_reverse_proxy::call(remote_addr.ip(), "http://127.0.0.1:13902", req)
//!
//!             } else {
//!                 debug_request(req)
//!             }
//!         })
//!     });
//!
//!     let server = Server::bind(&addr)
//!         .serve(make_svc)
//!         .map_err(|e| eprintln!("server error: {}", e));
//!
//!     println!("Running server on {:?}", addr);
//!
//!     // Run this server for... forever!
//!     hyper::rt::run(server);
//! }
//! ```
//!

use futures::future::{self, Future};
use hyper::header::{HeaderMap, HeaderValue};
use hyper::Body;
use hyper::{Client, Request, Response, StatusCode, Uri};
use lazy_static::lazy_static;
use std::net::IpAddr;
use std::str::FromStr;

type BoxFut = Box<Future<Item = Response<Body>, Error = hyper::Error> + Send>;

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

fn forward_uri<B>(forward_url: &str, req: &Request<B>) -> Uri {
    let forward_uri = match req.uri().query() {
        Some(query) => format!("{}{}?{}", forward_url, req.uri().path(), query),
        None => format!("{}{}", forward_url, req.uri().path()),
    };

    Uri::from_str(forward_uri.as_str()).unwrap()
}

fn create_proxied_request<B>(
    client_ip: IpAddr,
    forward_url: &str,
    mut request: Request<B>,
) -> Request<B> {
    *request.headers_mut() = remove_hop_headers(request.headers());
    *request.uri_mut() = forward_uri(forward_url, &request);

    let x_forwarded_for_header_name = "x-forwarded-for";

    // Add forwarding information in the headers
    match request.headers_mut().entry(x_forwarded_for_header_name) {
        Ok(header_entry) => match header_entry {
            hyper::header::Entry::Vacant(entry) => {
                let addr = format!("{}", client_ip);
                entry.insert(addr.parse().unwrap());
            }

            hyper::header::Entry::Occupied(mut entry) => {
                let addr = format!("{}, {}", entry.get().to_str().unwrap(), client_ip);
                entry.insert(addr.parse().unwrap());
            }
        },

        // shouldn't happen...
        Err(_) => panic!("Invalid header name: {}", x_forwarded_for_header_name),
    }

    request
}

pub fn call(client_ip: IpAddr, forward_uri: &str, request: Request<Body>) -> BoxFut {
    let proxied_request = create_proxied_request(client_ip, forward_uri, request);

    let client = Client::new();
    let response = client.request(proxied_request).then(|response| {
        let proxied_response = match response {
            Ok(response) => create_proxied_response(response),
            Err(error) => {
                println!("Error: {}", error); // TODO: Configurable logging
                Response::builder()
                    .status(StatusCode::INTERNAL_SERVER_ERROR)
                    .body(Body::empty())
                    .unwrap()
            }
        };

        future::ok(proxied_response)
    });

    Box::new(response)
}
