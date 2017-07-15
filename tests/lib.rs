extern crate futures;
#[macro_use]
extern crate hyper;
extern crate hyper_reverse_proxy;

use futures::future::{self, Future, FutureResult};
use hyper::{Get, Request, Response};
use hyper::server::Service;
use hyper_reverse_proxy::ReverseProxy;

struct MockService<F: Fn(Request) -> Response>(F);

impl<F: Fn(Request) -> Response> Service for MockService<F> {
    type Request = Request;
    type Response = Response;
    type Error = hyper::Error;
    type Future = FutureResult<Response, hyper::Error>;

    fn call(&self, request: Self::Request) -> Self::Future {
        future::ok((self.0)(request))
    }
}

#[test]
#[ignore]
fn adds_forwarded_for_header() {
    // TODO: https://github.com/hyperium/hyper/issues/1258
    unimplemented!()
}

#[test]
fn forwards_the_bodies() {
    use futures::Stream;

    let mut request = Request::new(Get, "/".parse().unwrap());
    request.set_body("request");

    let service = ReverseProxy::new(MockService(|request| {
        let body = request.body().concat2().wait().unwrap();
        assert_eq!(body.as_ref(), b"request");

        Response::new().with_body("response")
    }));

    let response = service.call(request).wait().unwrap();
    let body = response.body().concat2().wait().unwrap();
    assert_eq!(body.as_ref(), b"response");
}

#[test]
fn clones_headers() {
    header! { (XTestHeader1, "X-Test-Header1") => [String] }
    header! { (XTestHeader2, "X-Test-Header2") => [String] }

    let mut request = Request::new(Get, "/".parse().unwrap());
    request.headers_mut().set(XTestHeader1("Test1".to_owned()));
    request.headers_mut().set(XTestHeader2("Test2".to_owned()));

    let service = ReverseProxy::new(MockService(|request| {
        let header1 = request.headers().get::<XTestHeader1>().unwrap();
        let header2 = request.headers().get::<XTestHeader2>().unwrap();
        assert_eq!(header1, &XTestHeader1("Test1".to_owned()));
        assert_eq!(header2, &XTestHeader2("Test2".to_owned()));
        Response::new()
    }));

    service.call(request).wait().unwrap();
}

#[test]
fn removes_request_hop_headers() {
    use hyper::header::{Connection, TransferEncoding, Upgrade};

    let mut request = Request::new(Get, "/".parse().unwrap());
    request.headers_mut().set(Connection(vec![]));
    request.headers_mut().set_raw("Keep-Alive", "");
    request.headers_mut().set_raw("Proxy-Authenticate", "");
    request.headers_mut().set_raw("Proxy-Authorization", "");
    request.headers_mut().set_raw("TE", "");
    request.headers_mut().set_raw("Trailers", "");
    request.headers_mut().set(TransferEncoding(vec![]));
    request.headers_mut().set(Upgrade(vec![]));

    let service = ReverseProxy::new(MockService(|request| {
        assert_eq!(request.headers().get::<Connection>(), None);
        assert_eq!(request.headers().get_raw("Keep-Alive"), None);
        assert_eq!(request.headers().get_raw("Proxy-Authenticate"), None);
        assert_eq!(request.headers().get_raw("Proxy-Authorization"), None);
        assert_eq!(request.headers().get_raw("TE"), None);
        assert_eq!(request.headers().get_raw("Trailers"), None);
        assert_eq!(request.headers().get::<TransferEncoding>(), None);
        assert_eq!(request.headers().get::<Upgrade>(), None);
        Response::new()
    }));

    service.call(request).wait().unwrap();
}

#[test]
fn removes_response_hop_headers() {
    use hyper::header::{Connection, TransferEncoding, Upgrade};

    let request = Request::new(Get, "/".parse().unwrap());

    let service = ReverseProxy::new(MockService(|_| {
        let mut response = Response::new();
        response.headers_mut().set(Connection(vec![]));
        response.headers_mut().set_raw("Keep-Alive", "");
        response.headers_mut().set_raw("Proxy-Authenticate", "");
        response.headers_mut().set_raw("Proxy-Authorization", "");
        response.headers_mut().set_raw("TE", "");
        response.headers_mut().set_raw("Trailers", "");
        response.headers_mut().set(TransferEncoding(vec![]));
        response.headers_mut().set(Upgrade(vec![]));
        response
    }));

    let response = service.call(request).wait().unwrap();
    assert_eq!(response.headers().get::<Connection>(), None);
    assert_eq!(response.headers().get_raw("Keep-Alive"), None);
    assert_eq!(response.headers().get_raw("Proxy-Authenticate"), None);
    assert_eq!(response.headers().get_raw("Proxy-Authorization"), None);
    assert_eq!(response.headers().get_raw("TE"), None);
    assert_eq!(response.headers().get_raw("Trailers"), None);
    assert_eq!(response.headers().get::<TransferEncoding>(), None);
    assert_eq!(response.headers().get::<Upgrade>(), None);
}
