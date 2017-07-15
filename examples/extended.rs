//! A more involved example of using the `ReverseProxy` service.

#[macro_use]
extern crate error_chain;
extern crate futures;
extern crate hyper;
extern crate hyper_reverse_proxy;
extern crate tokio_core;
extern crate tokio_signal;

use futures::{BoxFuture, Future, Stream};
use tokio_core::reactor::Handle;
use std::net::{SocketAddr, Ipv4Addr};

error_chain! {
    foreign_links {
        Io(std::io::Error);
        Hyper(hyper::Error);
    }
}

fn shutdown_future(handle: &Handle) -> BoxFuture<(), std::io::Error> {
    use tokio_signal::unix::{Signal, SIGINT, SIGTERM};

    let sigint = Signal::new(SIGINT, handle).flatten_stream();
    let sigterm = Signal::new(SIGTERM, handle).flatten_stream();

    Stream::select(sigint, sigterm)
        .into_future()
        .map(|_| ())
        .map_err(|(e, _)| e)
        .boxed()
}

fn run() -> Result<()> {
    use futures::task::{self, Task};
    use hyper::Client;
    use hyper::server::{Http, Service};
    use hyper_reverse_proxy::ReverseProxy;
    use std::rc::{Rc, Weak};
    use std::cell::RefCell;
    use std::time::Duration;
    use tokio_core::net::TcpListener;
    use tokio_core::reactor::{Core, Timeout};

    struct Info {
        active: usize,
        blocker: Option<Task>,
    }

    struct NotifyService<S> {
        inner: S,
        info: Weak<RefCell<Info>>,
    }

    impl<S: Service> Service for NotifyService<S> {
        type Request = S::Request;
        type Response = S::Response;
        type Error = S::Error;
        type Future = S::Future;

        fn call(&self, message: Self::Request) -> Self::Future {
            self.inner.call(message)
        }
    }

    impl<S> Drop for NotifyService<S> {
        fn drop(&mut self) {
            if let Some(info) = self.info.upgrade() {
                let mut info = info.borrow_mut();
                info.active -= 1;
                if info.active == 0 {
                    if let Some(task) = info.blocker.take() {
                        task.notify();
                    }
                }
            }
        }
    }

    struct WaitUntilZero {
        info: Rc<RefCell<Info>>,
    }

    impl Future for WaitUntilZero {
        type Item = ();
        type Error = std::io::Error;

        fn poll(&mut self) -> futures::Poll<(), std::io::Error> {
            use futures::Async;

            let mut info = self.info.borrow_mut();
            if info.active == 0 {
                Ok(Async::Ready(()))
            } else {
                info.blocker = Some(task::current());
                Ok(Async::NotReady)
            }
        }
    }

    // Set up the Tokio reactor core
    let mut core = Core::new()?;
    let handle = core.handle();

    // Set up a TCP socket to listen to
    let listen_addr = SocketAddr::new(Ipv4Addr::new(127, 0, 0, 1).into(), 8080);
    let listener = TcpListener::bind(&listen_addr, &handle)?;

    println!("Listening on {}", listen_addr);

    // Keep track of how many active connections we are managing
    let info = Rc::new(RefCell::new(Info {
        active: 0,
        blocker: None,
    }));

    // Listen to incoming requests over TCP, and forward them to a new `ReverseProxy`
    let http = Http::new();
    let server = listener.incoming().for_each(|(socket, addr)| {
        let client = Client::new(&handle);
        let service = NotifyService {
            inner: ReverseProxy::new(client, Some(addr.ip())),
            info: Rc::downgrade(&info),
        };

        info.borrow_mut().active += 1;
        http.bind_connection(&handle, socket, addr, service);
        Ok(())
    });

    let shutdown = shutdown_future(&handle);

    // Start our server, blocking the main thread.
    match core.run(Future::select(shutdown, server)) {
        Ok(((), _next)) => {}
        Err((error, _next)) => bail!(error),
    }

    println!("Shutting down gracefully");

    // Let the outstanding requests run for 2 seconds, then shut down the server
    let timeout = Timeout::new(Duration::from_secs(2), &handle)?;
    let wait = WaitUntilZero { info: info.clone() };
    match core.run(Future::select(wait, timeout)) {
        Ok(((), _next)) => Ok(()),
        Err((error, _next)) => Err(error.into()),
    }
}

quick_main!(run);
