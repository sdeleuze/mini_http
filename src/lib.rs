/*!
## mini_http


## Basic Example

```rust
# fn run() -> Result<(), Box<::std::error::Error>> {
mini_http::Server::new("127.0.0.1:3002")?
    .start(|request| {
        mini_http::Response::builder()
            .status(200)
            .body(b"Hello!\n".to_vec())
            .unwrap()
    })?;
# Ok(())
# }
```

Note: If you're experiencing poor performance on benchmarks, see
[`tcp_nodelay`](struct.Server.html#method.tcp_nodelay)
*/

#![recursion_limit = "1024"]
#[macro_use]
extern crate error_chain;
#[macro_use]
extern crate log;
extern crate http;
extern crate httparse;
extern crate mio;
extern crate slab;

#[macro_use]
mod macros;
mod errors;
mod http_stream;

pub use http::header;
pub use http::method;
pub use http::response;
pub use http::status;
pub use http::uri;
pub use http::version;
use mio::net::TcpListener;
use std::io::{self, Read, Write};

pub use errors::*;
use http_stream::HttpStreamReader;

/// Re-exported `http::Response` for constructing return responses in handlers
pub use http::Response;
use mio::{Interest, Token};

#[cfg(target_os = "wasi")]
#[cfg(not(windows))]
fn get_first_listen_fd_listener() -> Option<std::net::TcpListener> {
    #[cfg(unix)]
    use std::os::unix::io::FromRawFd;
    #[cfg(target_os = "wasi")]
    use std::os::wasi::io::FromRawFd;

    Some(unsafe { std::net::TcpListener::from_raw_fd(3) })
}

#[cfg(windows)]
fn get_first_listen_fd_listener() -> Option<std::net::TcpListener> {
    // Windows does not support `LISTEN_FDS`
    None
}

#[cfg(target_os = "wasi")]
fn get_tcp_listener(_addr: Option<String>) -> TcpListener {
    std::env::var("LISTEN_FDS").expect("LISTEN_FDS environment variable unset");
    let stdlistener = get_first_listen_fd_listener().unwrap();
    stdlistener.set_nonblocking(true).unwrap();
    TcpListener::from_std(stdlistener)
}

#[cfg(not(target_os = "wasi"))]
fn get_tcp_listener(addr: Option<String>) -> TcpListener {
    TcpListener::bind(addr.unwrap().parse().unwrap()).unwrap()
}

/// Internal `http::Response` wrapper with helpers for constructing the bytes
/// that needs to be written back a Stream
struct ResponseWrapper {
    inner: http::Response<Vec<u8>>,
    header_data: Vec<u8>,
}
impl ResponseWrapper {
    fn new(inner: http::Response<Vec<u8>>) -> Self {
        Self {
            inner,
            header_data: Vec::with_capacity(1024),
        }
    }

    fn serialize_headers(&mut self) {
        {
            let body_len = self.inner.body().len();
            let hdrs = self.inner.headers_mut();
            hdrs.insert(
                header::SERVER,
                header::HeaderValue::from_static("mini-http (rust)"),
            );
            if body_len > 0 {
                let len = header::HeaderValue::from_str(&body_len.to_string()).unwrap();
                hdrs.insert(header::CONTENT_LENGTH, len);
            }
        }
        let status = self.inner.status();
        let s = format!(
            "HTTP/1.1 {} {}\r\n",
            status.as_str(),
            status.canonical_reason().unwrap_or("Unsupported Status")
        );
        self.header_data.extend_from_slice(&s.as_bytes());

        for (key, value) in self.inner.headers().iter() {
            self.header_data.extend_from_slice(key.as_str().as_bytes());
            self.header_data.extend_from_slice(b": ");
            self.header_data.extend_from_slice(value.as_bytes());
            self.header_data.extend_from_slice(b"\r\n");
        }
        self.header_data.extend_from_slice(b"\r\n");
    }
}
impl std::ops::Deref for ResponseWrapper {
    type Target = http::Response<Vec<u8>>;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}
impl std::ops::DerefMut for ResponseWrapper {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

/// Represent everything about a request except its (possible) body
type RequestHead = http::Request<()>;

/// `Request` received and used by handlers. Wraps & `deref`s to an `http::Request`
/// and patches `Request::body` to return the correct slice of bytes from the
/// `HttpStreamReader.read_buf`
pub struct Request {
    inner: http::Request<Vec<u8>>,
    body_start: usize,
}
impl Request {
    pub fn body(&self) -> &[u8] {
        &self.inner.body()[self.body_start..]
    }
}
impl std::ops::Deref for Request {
    type Target = http::Request<Vec<u8>>;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}
impl std::ops::DerefMut for Request {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

/// Represent the tcp socket & streams being polled by `mio`
enum Socket {
    Listener {
        listener: mio::net::TcpListener,
    },
    Stream {
        stream: mio::net::TcpStream,
        reader: HttpStreamReader,
        request: Option<RequestHead>,
        done_reading: bool,
        bytes_written: usize,
    },
}
impl Socket {
    fn new_listener(l: mio::net::TcpListener) -> Self {
        Socket::Listener { listener: l }
    }

    /// Construct a new `Stream` variant accepts from a tcp listener
    fn new_stream(s: mio::net::TcpStream, reader: HttpStreamReader) -> Self {
        Socket::Stream {
            stream: s,
            reader,
            request: None,
            done_reading: false,
            bytes_written: 0,
        }
    }

    /// Construct a "continued" stream. Stream reading hasn't been completed yet
    fn continued_stream(
        stream: mio::net::TcpStream,
        reader: HttpStreamReader,
        request: Option<RequestHead>,
        done_reading: bool,
        bytes_written: usize,
    ) -> Self {
        Socket::Stream {
            stream,
            reader,
            request,
            done_reading,
            bytes_written,
        }
    }
}

pub struct Server {
    addr: Option<String>,
    no_delay: bool,
}
impl Server {
    /// Initialize a new default `Server` to run on `addr`
    #[cfg(not(target_os = "wasi"))]
    pub fn new(addr: &str) -> Result<Self> {
        Ok(Self {
            addr: Some(addr.to_string()),
            no_delay: false,
        })
    }

    /// Initialize a new default `Server` to run on preopened socket
    pub fn preopened() -> Result<Self> {
        Ok(Self {
            addr: None,
            no_delay: false,
        })
    }

    /// Configure `tcp_nodelay` setting for each server socket.
    /// Default: `false`
    ///
    /// From [`mio::net::TcpStream` docs](https://docs.rs/mio/*/mio/net/struct.TcpStream.html#method.set_nodelay):
    ///
    /// Sets the value of the TCP_NODELAY option on this socket.
    /// If set, this option disables the Nagle algorithm. This means
    /// that segments are always sent as soon as possible, even if
    /// there is only a small amount of data. When not set, data is
    /// buffered until there is a sufficient amount to send out,
    /// thereby avoiding the frequent sending of small packets.
    ///
    /// Note: `tcp_nodelay(true)` will **significantly** improve performance
    ///        for benchmarking loads, but may not be necessary for real world usage.
    ///        For example on my laptop, `wrk -t2 -c100` increases from 2.5k to 92k req/s.
    pub fn tcp_nodelay(&mut self, no_delay: bool) -> &mut Self {
        self.no_delay = no_delay;
        self
    }

    /// Start the server using the given handler function
    pub fn start<F>(&self, func: F) -> Result<()>
    where
        F: 'static + Fn(Request) -> Response<Vec<u8>>,
    {
        let mut sockets = slab::Slab::with_capacity(1024);
        let mut server = get_tcp_listener(self.addr.clone());

        // initialize poll
        let mut poll = mio::Poll::new()?;
        {
            // register our tcp listener
            let entry = sockets.vacant_entry();
            let server_token = Token(entry.key());
            poll.registry()
                .register(&mut server, server_token, Interest::READABLE)?;
            entry.insert(Socket::new_listener(server));
        }

        if let Some(addr) = &self.addr {
            info!("** Listening on {} **", addr);
        } else {
            info!("** Using preopened socket FD 3 **");
        }

        let mut events = mio::Events::with_capacity(1024);
        loop {
            debug!("Beginning of loop");
            poll.poll(&mut events, None)?;
            'next_event: for e in &events {
                let token = e.token();
                match sockets.remove(token.into()) {
                    Socket::Listener { mut listener } => {
                        if e.is_readable() {
                            match listener.accept() {
                                Ok((mut sock, addr)) => {
                                    debug!("opened socket to: {:?}", addr);

                                    // register the newly opened socket
                                    let entry = sockets.vacant_entry();
                                    let token = Token(entry.key());
                                    poll.registry().register(
                                        &mut sock,
                                        token,
                                        Interest::READABLE | Interest::WRITABLE,
                                    )?;
                                    entry.insert(Socket::new_stream(sock, HttpStreamReader::new()));
                                }
                                Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {}
                                Err(e) => {
                                    error!("{:?} - Encountered error while accepting the connection: {:?}", token, e);
                                }
                            };
                        }
                        // reregister listener
                        let entry = sockets.vacant_entry();
                        let token = Token(entry.key());
                        poll.registry()
                            .reregister(&mut listener, token, Interest::READABLE)?;
                        entry.insert(Socket::new_listener(listener));
                    }
                    Socket::Stream {
                        mut stream,
                        mut reader,
                        request,
                        mut done_reading,
                        mut bytes_written,
                    } => {
                        if e.is_read_closed() || e.is_write_closed() {
                            poll.registry().deregister(&mut stream)?;
                            continue 'next_event;
                        }

                        // Try reading and parsing a request from this stream.
                        // `try_build_request` will return `None` until the request is parsed and the
                        // body is done being read. After that, `done_reading` will be set
                        // to `true`. At that point, if this socket is readable, we still need to
                        // check if it's been closed, but we will no longer try parsing the request
                        // bytes
                        let (mut request, err_response): (
                            Option<RequestHead>,
                            Option<ResponseWrapper>,
                        ) = if e.is_readable() {
                            let mut buf = [0; 256];
                            let stream_close = loop {
                                match stream.read(&mut buf) {
                                    Ok(0) => {
                                        // the stream has ended for real
                                        break true;
                                    }
                                    Ok(n) => {
                                        reader.receive_chunk(&buf[..n]);
                                        debug!("{:?} - Read {} bytes", token, n);
                                    }
                                    Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                                        break false
                                    }
                                    Err(ref e) if e.kind() == io::ErrorKind::ConnectionReset => {
                                        break true
                                    }
                                    Err(e) => {
                                        error!("{:?} - Encountered error while reading from socket: {:?}", token, e);
                                        // let this socket die, jump to the next event
                                        break true;
                                    }
                                }
                            };
                            if stream_close {
                                debug!("{:?} - Stream closed. Killing socket.", token);
                                // jump to the next mio event
                                // TODO: if we have a `receiver` (a handler is running)
                                //       try shutting it down
                                poll.registry().deregister(&mut stream)?;
                                continue 'next_event;
                            }
                            if done_reading {
                                (request, None)
                            } else {
                                match reader.try_build_request() {
                                    Ok(r) => (r, None),
                                    Err(e) => {
                                        // TODO: return the proper status-code per error
                                        error!(
                                            "{:?} - Encountered error while parsing: {}",
                                            token, e
                                        );
                                        (
                                            None,
                                            Some(ResponseWrapper::new(
                                                Response::builder()
                                                    .status(400)
                                                    .body(b"bad request".to_vec())
                                                    .unwrap(),
                                            )),
                                        )
                                    }
                                }
                            }
                        } else {
                            (request, None)
                        };
                        if request.is_some() || err_response.is_some() {
                            done_reading = true;
                            debug!("Reading is done for token {:?}", token);
                        }

                        // Once the request is parsed, this block will execute once.
                        // The head-only request (RequestHead) will be converted into
                        // a public `Request` and the `HttpStreamReader`s `read_buf` will be
                        // swapped into the new `Request`s body. The provided
                        // `func` handler will be started for later retrieval
                        let response = if let Some(req) = request.take() {
                            debug!("Begin processing the response for token {:?}", token);
                            let (parts, _) = req.into_parts();
                            let mut body = vec![];
                            std::mem::swap(&mut body, &mut reader.read_buf);
                            let request = Request {
                                inner: http::Request::from_parts(parts, body),
                                body_start: reader.headers_length,
                            };

                            let resp = func(request);
                            let mut resp = ResponseWrapper::new(resp);
                            resp.serialize_headers();
                            debug!("Headers serialized for token {:?}", token);
                            Some(resp)
                        } else {
                            None
                        };

                        // See if a `ResponseWrapper` is available

                        // If we have a `ResponseWrapper`, start writing its headers and body
                        // back to the stream
                        let mut done_write = false;
                        if let Some(ref resp) = response {
                            debug!("Response body ready to be written for token {:?}", token);
                            if e.is_writable() {
                                debug!("Body writeable for token {:?}", token);
                                let header_data_len = resp.header_data.len();
                                let body_len = resp.body().len();
                                let total_len = header_data_len + body_len;
                                'write: loop {
                                    let (data, read_start) = if bytes_written < header_data_len {
                                        (&resp.header_data, bytes_written)
                                    } else if bytes_written < total_len {
                                        (resp.body(), bytes_written - header_data_len)
                                    } else {
                                        done_write = true;
                                        debug!("{:?} - flushing", token);
                                        // If flushing fails, something bad probably happened.
                                        // If it didn't fail because of a connection error (connection
                                        // is still alive), it will eventually be flushed by the os
                                        stream.flush().ok();
                                        break 'write;
                                    };
                                    match stream.write(&data[read_start..]) {
                                        Ok(n) => {
                                            bytes_written += n;
                                            debug!("{:?} - Wrote {} bytes", token, n);
                                        }
                                        Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                                            break 'write
                                        }
                                        Err(e) => {
                                            error!("{:?} - Encountered error while writing to socket: {:?}", token, e);
                                            // let this socket die, jump to the next event
                                            done_write = true;
                                        }
                                    }
                                }
                            } else {
                                debug!("Body not writeable for token {:?}", token);
                            }
                        }

                        if !done_write {
                            // we're not done writing our response to this socket yet
                            // reregister stream
                            debug!("Write not done, reregister stream for token {:?}", token);
                            let entry = sockets.vacant_entry();
                            let token = Token(entry.key());
                            poll.registry().reregister(
                                &mut stream,
                                token,
                                Interest::READABLE | Interest::WRITABLE,
                            )?;
                            entry.insert(Socket::continued_stream(
                                stream,
                                reader,
                                request,
                                done_reading,
                                bytes_written,
                            ));
                        } else {
                            debug!("{:?} - Done writing, killing socket", token);
                            poll.registry().deregister(&mut stream)?;
                        }
                    }
                }
            }
        }
    }
}
