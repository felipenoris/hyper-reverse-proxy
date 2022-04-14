
# hyper-reverse-proxy

[![License][license-img]](LICENSE)
[![CI][ci-img]][ci-url]
[![docs][docs-img]][docs-url]
[![version][version-img]][version-url]

[license-img]: https://img.shields.io/crates/l/hyper-reverse-proxy.svg
[ci-img]: https://github.com/felipenoris/hyper-reverse-proxy/workflows/CI/badge.svg
[ci-url]: https://github.com/felipenoris/hyper-reverse-proxy/actions?query=workflow%3ACI
[docs-img]: https://docs.rs/hyper-reverse-proxy/badge.svg
[docs-url]: https://docs.rs/hyper-reverse-proxy
[version-img]: https://img.shields.io/crates/v/hyper-reverse-proxy.svg
[version-url]: https://crates.io/crates/hyper-reverse-proxy

A simple reverse proxy, to be used with [Hyper].

The implementation ensures that [Hop-by-hop headers] are stripped correctly in both directions,
and adds the client's IP address to a comma-space-separated list of forwarding addresses in the
`X-Forwarded-For` header.

The implementation is based on Go's [`httputil.ReverseProxy`].

[Hyper]: http://hyper.rs/
[Hop-by-hop headers]: http://www.w3.org/Protocols/rfc2616/rfc2616-sec13.html
[`httputil.ReverseProxy`]: https://golang.org/pkg/net/http/httputil/#ReverseProxy

# Example

Add these dependencies to your `Cargo.toml` file.

```toml
[dependencies]
hyper-reverse-proxy = "0.5"
hyper = { version = "0.14", features = ["full"] }
tokio = { version = "1", features = ["full"] }
```

The following example will set up a reverse proxy listening on `127.0.0.1:13900`,
and will proxy these calls:

* `"/target/first"` will be proxied to `http://127.0.0.1:13901`

* `"/target/second"` will be proxied to `http://127.0.0.1:13902`

* All other URLs will be handled by `debug_request` function, that will display request information.

```rust,no_run
use hyper::server::conn::AddrStream;
use hyper::{Body, Request, Response, Server, StatusCode};
use hyper::service::{service_fn, make_service_fn};
use std::{convert::Infallible, net::SocketAddr};
use std::net::IpAddr;

fn debug_request(req: Request<Body>) -> Result<Response<Body>, Infallible>  {
    let body_str = format!("{:?}", req);
    Ok(Response::new(Body::from(body_str)))
}

async fn handle(client_ip: IpAddr, req: Request<Body>) -> Result<Response<Body>, Infallible> {
    if req.uri().path().starts_with("/target/first") {
        // will forward requests to port 13901
        match hyper_reverse_proxy::call(client_ip, "http://127.0.0.1:13901", req).await {
            Ok(response) => {Ok(response)}
            Err(_error) => {Ok(Response::builder()
                                  .status(StatusCode::INTERNAL_SERVER_ERROR)
                                  .body(Body::empty())
                                  .unwrap())}
        }
    } else if req.uri().path().starts_with("/target/second") {
        // will forward requests to port 13902
        match hyper_reverse_proxy::call(client_ip, "http://127.0.0.1:13902", req).await {
            Ok(response) => {Ok(response)}
            Err(_error) => {Ok(Response::builder()
                                  .status(StatusCode::INTERNAL_SERVER_ERROR)
                                  .body(Body::empty())
                                  .unwrap())}
        }
    } else {
        debug_request(req)
    }
}

#[tokio::main]
async fn main() {
    let bind_addr = "127.0.0.1:8000";
    let addr:SocketAddr = bind_addr.parse().expect("Could not parse ip:port.");

    let make_svc = make_service_fn(|conn: &AddrStream| {
        let remote_addr = conn.remote_addr().ip();
        async move {
            Ok::<_, Infallible>(service_fn(move |req| handle(remote_addr, req)))
        }
    });

    let server = Server::bind(&addr).serve(make_svc);

    println!("Running server on {:?}", addr);

    if let Err(e) = server.await {
        eprintln!("server error: {}", e);
    }
}
```
