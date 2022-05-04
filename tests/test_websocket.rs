use std::{
    convert::Infallible,
    net::{IpAddr, SocketAddr},
    process::exit,
    time::Duration,
};

use async_tungstenite::tokio::{accept_async, connect_async};
use futures::{SinkExt, StreamExt};
use hyper::{
    client::{connect::dns::GaiResolver, HttpConnector},
    server::conn::AddrStream,
    service::{make_service_fn, service_fn},
    Body, Request, Response, Server,
};
use hyper_reverse_proxy::ReverseProxy;
use test_context::{test_context, AsyncTestContext};
use tokio::{net::TcpListener, sync::oneshot::Sender, task::JoinHandle};
use tokiotest_httpserver::take_port;
use tungstenite::Message;
use url::Url;

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
    ws_handler: JoinHandle<()>,
    port: u16,
}

#[test_context(ProxyTestContext)]
#[tokio::test]
async fn test_websocket(ctx: &mut ProxyTestContext) {
    let (mut client, _) =
        connect_async(Url::parse(&format!("ws://127.0.0.1:{}", ctx.port)).unwrap())
            .await
            .unwrap();

    client.send(Message::Ping("hello".into())).await.unwrap();
    let msg = client.next().await.unwrap().unwrap();

    assert!(
        matches!(&msg, Message::Pong(inner) if inner == "hello".as_bytes()),
        "did not get pong, but {:?}",
        msg
    );

    let msg = client.next().await.unwrap().unwrap();

    assert!(
        matches!(&msg, Message::Text(inner) if inner == "All done"),
        "did not get text, but {:?}",
        msg
    );
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
        Err(err) => panic!("did not expect error: {:?}", err),
    }
}

#[async_trait::async_trait]
impl<'a> AsyncTestContext for ProxyTestContext {
    async fn setup() -> ProxyTestContext {
        tokio::spawn(async {
            tokio::time::sleep(Duration::from_secs(5)).await;
            println!("Unit test executed too long, perhaps its stuck...");
            exit(1);
        });

        let (sender, receiver) = tokio::sync::oneshot::channel::<()>();
        let ws_port = take_port();

        let ws_handler = tokio::spawn(async move {
            let ws_server = TcpListener::bind(("127.0.0.1", ws_port)).await.unwrap();

            if let Ok((stream, _)) = ws_server.accept().await {
                let mut websocket = accept_async(stream).await.unwrap();

                let msg = websocket.next().await.unwrap().unwrap();
                assert!(
                    matches!(&msg, Message::Ping(inner) if inner == "hello".as_bytes()),
                    "did not get ping, but: {:?}",
                    msg
                );
                // Tungstenite will auto send a Pong as a response to a Ping

                websocket
                    .send(Message::Text("All done".to_string()))
                    .await
                    .unwrap();
            }
        });

        let make_svc = make_service_fn(move |conn: &AddrStream| {
            let remote_addr = conn.remote_addr().ip();
            async move { Ok::<_, Infallible>(service_fn(move |req| handle(remote_addr, req, ws_port))) }
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
            ws_handler,
            port,
        }
    }
    async fn teardown(self) {
        let _ = self.sender.send(()).unwrap();
        let _ = tokio::join!(self.proxy_handler, self.ws_handler);
    }
}
