use hyper::client::connect::dns::GaiResolver;
use hyper::client::HttpConnector;
use hyper::header::{CONNECTION, HOST, UPGRADE};
use hyper::server::conn::AddrStream;
use hyper::service::{make_service_fn, service_fn};
use hyper::{Body, Client, HeaderMap, Request, Response, Server, StatusCode, Uri};
use hyper_reverse_proxy::ReverseProxy;
use std::convert::Infallible;
use std::net::{IpAddr, SocketAddr};
use test_context::test_context;
use test_context::AsyncTestContext;
use tokio::sync::oneshot::Sender;
use tokio::task::JoinHandle;
use tokiotest_httpserver::handler::HandlerBuilder;
use tokiotest_httpserver::{take_port, HttpTestContext};

lazy_static::lazy_static! {
    static ref  PROXY_CLIENT: ReverseProxy<HttpConnector<GaiResolver>> = {
        ReverseProxy::new(
            hyper::Client::new(),
        )
    };
}

struct ProxyTestContext {
    sender: Sender<()>,
    proxy_handler: JoinHandle<Result<(), hyper::Error>>,
    http_back: HttpTestContext,
    port: u16,
}

#[test_context(ProxyTestContext)]
#[tokio::test]
async fn test_get_error_500(ctx: &mut ProxyTestContext) {
    let client = Client::new();
    let resp = client
        .request(
            Request::builder()
                .header("keep-alive", "treu")
                .method("GET")
                .uri(ctx.uri("/500"))
                .body(Body::from(""))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(500, resp.status());
}

#[test_context(ProxyTestContext)]
#[tokio::test]
async fn test_upgrade_mismatch(ctx: &mut ProxyTestContext) {
    ctx.http_back.add(
        HandlerBuilder::new("/ws")
            .status_code(StatusCode::SWITCHING_PROTOCOLS)
            .build(),
    );
    let resp = Client::new()
        .request(
            Request::builder()
                .header(CONNECTION, "Upgrade")
                .header(UPGRADE, "websocket")
                .method("GET")
                .uri(ctx.uri("/ws"))
                .body(Body::from(""))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), 502);
}

#[test_context(ProxyTestContext)]
#[tokio::test]
async fn test_upgrade_unrequested(ctx: &mut ProxyTestContext) {
    ctx.http_back.add(
        HandlerBuilder::new("/wrong_switch")
            .status_code(StatusCode::SWITCHING_PROTOCOLS)
            .build(),
    );
    let resp = Client::new().get(ctx.uri("/wrong_switch")).await.unwrap();
    assert_eq!(resp.status(), 502);
}

#[test_context(ProxyTestContext)]
#[tokio::test]
async fn test_get(ctx: &mut ProxyTestContext) {
    let mut headers = HeaderMap::new();
    headers.insert(
        HOST,
        format!("127.0.0.1:{}", ctx.http_back.port).parse().unwrap(),
    );

    ctx.http_back.add(
        HandlerBuilder::new("/foo")
            .status_code(StatusCode::OK)
            .headers(headers)
            .build(),
    );
    let resp = Client::new().get(ctx.uri("/foo")).await.unwrap();
    assert_eq!(200, resp.status());
}

#[test_context(ProxyTestContext)]
#[tokio::test]
async fn test_headers(ctx: &mut ProxyTestContext) {
    let mut headers = HeaderMap::new();
    headers.insert("x-forwarded-for", "127.0.0.1".parse().unwrap());
    headers.insert(
        "x-forwarded-host",
        format!("localhost:{}", ctx.port).parse().unwrap(),
    );

    ctx.http_back.add(
        HandlerBuilder::new("/foo")
            .status_code(StatusCode::OK)
            .headers(headers)
            .build(),
    );
    let resp = Client::new().get(ctx.uri("/foo")).await.unwrap();
    assert_eq!(200, resp.status());
}

async fn handle(
    client_ip: IpAddr,
    req: Request<Body>,
    backend_port: u16,
) -> Result<Response<Body>, Infallible> {
    match PROXY_CLIENT
        .call(
            client_ip,
            format!("http://127.0.0.1:{}", backend_port).as_str(),
            req,
        )
        .await
    {
        Ok(response) => Ok(response),
        Err(_) => Ok(Response::builder().status(502).body(Body::empty()).unwrap()),
    }
}

#[async_trait::async_trait]
impl<'a> AsyncTestContext for ProxyTestContext {
    async fn setup() -> ProxyTestContext {
        let http_back: HttpTestContext = AsyncTestContext::setup().await;
        let (sender, receiver) = tokio::sync::oneshot::channel::<()>();
        let bp_to_move = http_back.port;

        let make_svc = make_service_fn(move |conn: &AddrStream| {
            let remote_addr = conn.remote_addr().ip();
            let back_port = bp_to_move;
            async move {
                Ok::<_, Infallible>(service_fn(move |req| handle(remote_addr, req, back_port)))
            }
        });
        let port = take_port();
        let addr = SocketAddr::new("127.0.0.1".parse().unwrap(), port);
        let server = Server::bind(&addr)
            .serve(make_svc)
            .with_graceful_shutdown(async {
                receiver.await.ok();
            });
        let proxy_handler = tokio::spawn(server);
        ProxyTestContext {
            sender,
            proxy_handler,
            http_back,
            port,
        }
    }
    async fn teardown(self) {
        let _ = AsyncTestContext::teardown(self.http_back);
        let _ = self.sender.send(()).unwrap();
        let _ = tokio::join!(self.proxy_handler);
    }
}
impl ProxyTestContext {
    pub fn uri(&self, path: &str) -> Uri {
        format!("http://{}:{}{}", "localhost", self.port, path)
            .parse::<Uri>()
            .unwrap()
    }
}
