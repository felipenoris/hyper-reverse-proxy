# hyper-reverse-proxy

[![Build Status](https://travis-ci.org/brendanzab/hyper-reverse-proxy.svg?branch=master)](https://travis-ci.org/brendanzab/hyper-reverse-proxy)
[![Documentation](https://docs.rs/hyper-reverse-proxy/badge.svg)](https://docs.rs/hyper-reverse-proxy)
[![Version](https://img.shields.io/crates/v/hyper-reverse-proxy.svg)](https://crates.io/crates/hyper-reverse-proxy)
[![License](https://img.shields.io/crates/l/hyper-reverse-proxy.svg)](https://github.com/brendanzab/hyper-reverse-proxy/blob/master/LICENSE)

A simple reverse proxy, to be used with Hyper and Tokio.

The implementation was originally based on Go's [`httputil.ReverseProxy`].

[`httputil.ReverseProxy`]: https://golang.org/pkg/net/http/httputil/#ReverseProxy
