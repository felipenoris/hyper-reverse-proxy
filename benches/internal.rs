use criterion::{black_box, criterion_group, criterion_main, Criterion};
use hyper::client::connect::dns::GaiResolver;
use hyper::client::HttpConnector;
use hyper::header::HeaderName;
use hyper::Uri;
use hyper::{HeaderMap, Request, Response};
use hyper_reverse_proxy::ReverseProxy;
use rand::distributions::Alphanumeric;
use rand::prelude::*;
use std::net::Ipv4Addr;
use std::str::FromStr;
use test_context::AsyncTestContext;
use tokio::runtime::Runtime;
use tokiotest_httpserver::HttpTestContext;

lazy_static::lazy_static! {
    static ref  PROXY_CLIENT: ReverseProxy<HttpConnector<GaiResolver>> = {
        ReverseProxy::new(
            hyper::Client::new(),
        )
    };
}

fn create_proxied_response(b: &mut Criterion) {
    let headers_map = build_headers();

    b.bench_function("create proxied response", |t| {
        t.iter(|| {
            let mut response = Response::builder().status(200);

            *response.headers_mut().unwrap() = headers_map.clone();

            hyper_reverse_proxy::create_proxied_response(black_box(response.body(()).unwrap()));
        })
    });
}

fn generate_string() -> String {
    let take = rand::thread_rng().gen::<u8>().into();
    rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(take)
        .map(char::from)
        .collect()
}

fn build_headers() -> HeaderMap {
    let mut headers_map: HeaderMap = (&hyper_reverse_proxy::HOP_HEADERS)
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

fn proxy_call(b: &mut Criterion) {
    let rt = Runtime::new().unwrap();

    let uri = Uri::from_static("http://0.0.0.0:8080/me?hello=world");

    let http_context: HttpTestContext = rt.block_on(async { AsyncTestContext::setup().await });

    let forward_url = &format!("http://0.0.0.0:{}", http_context.port);

    let headers_map = build_headers();

    let client_ip = std::net::IpAddr::from(Ipv4Addr::from_str("0.0.0.0").unwrap());

    b.bench_function("proxy call", |c| {
        c.iter(|| {
            rt.block_on(async {
                let mut request = Request::builder().uri(uri.clone());

                *request.headers_mut().unwrap() = headers_map.clone();

                black_box(&PROXY_CLIENT)
                    .call(
                        black_box(client_ip),
                        black_box(forward_url),
                        black_box(request.body(hyper::Body::from("")).unwrap()),
                    )
                    .await
                    .unwrap();
            })
        })
    });
}

fn forward_url_with_str_ending_slash(b: &mut Criterion) {
    let uri = Uri::from_static("https://0.0.0.0:8080/me");
    let port = rand::thread_rng().gen::<u8>();
    let forward_url = &format!("https://0.0.0.0:{}/", port);

    b.bench_function("forward url with str ending slash", |b| {
        b.iter(|| {
            let request = Request::builder().uri(uri.clone()).body(());

            hyper_reverse_proxy::forward_uri(forward_url, &request.unwrap());
        })
    });
}

fn forward_url_with_str_ending_slash_and_query(b: &mut Criterion) {
    let uri = Uri::from_static("https://0.0.0.0:8080/me?hello=world");
    let port = rand::thread_rng().gen::<u8>();
    let forward_url = &format!("https://0.0.0.0:{}/", port);

    b.bench_function("forward url with str ending slash and query", |t| {
        t.iter(|| {
            let request = Request::builder().uri(uri.clone()).body(());

            hyper_reverse_proxy::forward_uri(forward_url, &request.unwrap());
        })
    });
}

fn forward_url_no_ending_slash(b: &mut Criterion) {
    let uri = Uri::from_static("https://0.0.0.0:8080/me");
    let port = rand::thread_rng().gen::<u8>();
    let forward_url = &format!("https://0.0.0.0:{}", port);

    b.bench_function("forward url no ending slash", |t| {
        t.iter(|| {
            let request = Request::builder().uri(uri.clone()).body(());

            hyper_reverse_proxy::forward_uri(forward_url, &request.unwrap());
        })
    });
}

fn forward_url_with_query(b: &mut Criterion) {
    let uri = Uri::from_static("https://0.0.0.0:8080/me?hello=world");
    let port = rand::thread_rng().gen::<u8>();
    let forward_url = &format!("https://0.0.0.0:{}", port);

    b.bench_function("forward_url_with_query", |t| {
        t.iter(|| {
            let request = Request::builder().uri(uri.clone()).body(());

            hyper_reverse_proxy::forward_uri(forward_url, &request.unwrap());
        })
    });
}

fn create_proxied_request_forwarded_for_occupied(b: &mut Criterion) {
    let uri = Uri::from_static("https://0.0.0.0:8080/me?hello=world");
    let port = rand::thread_rng().gen::<u8>();
    let forward_url = &format!("https://0.0.0.0:{}", port);

    let mut headers_map = build_headers();

    headers_map.insert(
        HeaderName::from_static("x-forwarded-for"),
        "0.0.0.0".parse().unwrap(),
    );

    let client_ip = std::net::IpAddr::from(Ipv4Addr::from_str("0.0.0.0").unwrap());

    b.bench_function("create proxied request forwarded for occupied", |t| {
        t.iter(|| {
            let mut request = Request::builder().uri(uri.clone());

            *request.headers_mut().unwrap() = headers_map.clone();

            hyper_reverse_proxy::create_proxied_request(
                client_ip,
                forward_url,
                request.body(()).unwrap(),
                None,
            )
            .unwrap();
        })
    });
}

fn create_proxied_request_forwarded_for_vacant(b: &mut Criterion) {
    let uri = Uri::from_static("https://0.0.0.0:8080/me?hello=world");
    let port = rand::thread_rng().gen::<u8>();
    let forward_url = &format!("https://0.0.0.0:{}", port);

    let headers_map = build_headers();

    let client_ip = std::net::IpAddr::from(Ipv4Addr::from_str("0.0.0.0").unwrap());

    b.bench_function("create proxied request forwarded for vacant", |t| {
        t.iter(|| {
            let mut request = Request::builder().uri(uri.clone());

            *request.headers_mut().unwrap() = headers_map.clone();

            hyper_reverse_proxy::create_proxied_request(
                client_ip,
                forward_url,
                request.body(()).unwrap(),
                None,
            )
            .unwrap();
        })
    });
}

criterion_group!(external_api, proxy_call);
criterion_group!(responses, create_proxied_response);
criterion_group!(
    url_parsing,
    forward_url_with_query,
    forward_url_no_ending_slash,
    forward_url_with_str_ending_slash_and_query,
    forward_url_with_str_ending_slash
);
criterion_group!(
    requests,
    create_proxied_request_forwarded_for_vacant,
    create_proxied_request_forwarded_for_occupied
);
criterion_main!(external_api, responses, url_parsing, requests);
