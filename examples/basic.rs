//! A simple use of the `ReverseProxy` service.

extern crate futures;
extern crate hyper;
extern crate hyper_reverse_proxy;
extern crate tokio_core;

use futures::Stream;
use hyper::Client;
use hyper::server::Http;
use hyper_reverse_proxy::ReverseProxy;
use tokio_core::net::TcpListener;
use tokio_core::reactor::Core;
use std::net::{SocketAddr, Ipv4Addr};

fn run() -> hyper::Result<()> {
    // Set up the Tokio reactor core
    let mut core = Core::new()?;
    let handle = core.handle();

    // Set up a TCP socket to listen to
    let listen_addr = SocketAddr::new(Ipv4Addr::new(127, 0, 0, 1).into(), 8080);
    let listener = TcpListener::bind(&listen_addr, &handle)?;

    // Listen to incoming requests over TCP, and forward them to a new `ReverseProxy`
    let http = Http::new();
    let server = listener.incoming().for_each(|(socket, addr)| {
        let client = Client::new(&handle);
        let service = ReverseProxy::new(client, Some(addr.ip()));
        http.bind_connection(&handle, socket, addr, service);
        Ok(())
    });

    // Start our server on the reactor core
    core.run(server)?;

    Ok(())
}

fn main() {
    use std::io::{self, Write};

    if let Err(error) = run() {
        write!(&mut io::stderr(), "{}", error).expect("Error writing to stderr");
        std::process::exit(1);
    }
}
