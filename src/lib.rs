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

#![recursion_limit="1024"]
#[macro_use] extern crate error_chain;
#[macro_use] extern crate log;
extern crate mio;
extern crate slab;
extern crate threadpool;
extern crate num_cpus;
extern crate httparse;
extern crate http;

#[macro_use] mod macros;
mod errors;
mod http_stream;

use std::io::{self, Read, Write};
use std::sync;
use std::sync::mpsc::{channel, Receiver};
use std::time;
use std::thread;
use std::net;
use mio::net::{TcpListener};
pub use http::header;
pub use http::method;
pub use http::response;
pub use http::status;
pub use http::uri;
pub use http::version;

use http_stream::HttpStreamReader;
pub use errors::*;


/// Re-exported `http::Response` for constructing return responses in handlers
pub use http::Response;
use mio::{Interest, Token};


/// Internal `http::Response` wrapper with helpers for constructing the bytes
/// that needs to be written back a Stream
struct ResponseWrapper {
    inner: http::Response<Vec<u8>>,
    header_data: Vec<u8>
}
impl ResponseWrapper {
    fn new(inner: http::Response<Vec<u8>>) -> Self {
        Self { inner, header_data: Vec::with_capacity(1024) }
    }

    fn serialize_headers(&mut self) {
        {
            let body_len = self.inner.body().len();
            let hdrs = self.inner.headers_mut();
            hdrs.insert(header::SERVER, header::HeaderValue::from_static("mini-http (rust)"));
            if body_len > 0 {
                let len = header::HeaderValue::from_str(&body_len.to_string()).unwrap();
                hdrs.insert(header::CONTENT_LENGTH, len);
            }
        }
        let status = self.inner.status();
        let s = format!("HTTP/1.1 {} {}\r\n", status.as_str(), status.canonical_reason().unwrap_or("Unsupported Status"));
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


#[derive(Debug, Clone, PartialEq, Eq)]
enum SocketStatus {
    New,
    Reused,
}

/// Represent the tcp socket & streams being polled by `mio`
enum Socket {
    Listener {
        listener: mio::net::TcpListener,
    },
    Stream {
        stream: mio::net::TcpStream,
        keep_alive: bool,
        socket_status: SocketStatus,
        reader: HttpStreamReader,
        request: Option<RequestHead>,
        done_reading: bool,
        receiver: Option<Receiver<ResponseWrapper>>,
        response: Option<ResponseWrapper>,
        bytes_written: usize,
    },
}
impl Socket {
    fn new_listener(l: mio::net::TcpListener) -> Self {
        Socket::Listener { listener: l }
    }

    /// Construct a new `Stream` variant accepts from a tcp listener
    fn new_stream(s: mio::net::TcpStream, reader: HttpStreamReader, socket_status: SocketStatus) -> Self {
        Socket::Stream {
            stream: s,
            keep_alive: true,
            socket_status: socket_status,
            reader: reader,
            request: None,
            done_reading: false,
            receiver: None,
            response: None,
            bytes_written: 0,
        }
    }

    /// Construct a "continued" stream. Stream reading hasn't been completed yet
    fn continued_stream(stream: mio::net::TcpStream,
                        keep_alive: bool,
                        socket_status: SocketStatus,
                        reader: HttpStreamReader,
                        request: Option<RequestHead>,
                        done_reading: bool,
                        receiver: Option<Receiver<ResponseWrapper>>,
                        response: Option<ResponseWrapper>,
                        bytes_written: usize) -> Self
    {
        Socket::Stream { stream, keep_alive, socket_status, reader, request, done_reading, receiver, response, bytes_written }
    }
}


enum Thread {
    Pooled {
        pool: threadpool::ThreadPool,
    },
    Threaded,
}
impl Thread {
    fn new(size: usize) -> Self {
        if size > 0 {
            Thread::Pooled { pool: threadpool::ThreadPool::new(size) }
        } else {
            Thread::Threaded
        }
    }

    fn execute<F: FnOnce() + Send + 'static>(&self, f: F) {
        match self {
            &Thread::Pooled { ref pool } => {
                pool.execute(f);
            }
            &Thread::Threaded => {
                thread::spawn(f);
            }
        }
    }
}


pub struct Server {
    addr: net::SocketAddr,
    no_delay: bool,
    pool_size: usize,
    keep_alive_dur: time::Duration,
}
impl Server {
    /// Initialize a new default `Server` to run on `addr`
    pub fn new(addr: &str) -> Result<Self> {
        Ok(Self {
            addr: addr.parse()?,
            no_delay: false,
            pool_size: num_cpus::get() * 8,
            keep_alive_dur: time::Duration::from_secs(5),
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

    /// Configure the size of the thread pool that's used for executing handlers
    ///
    /// Defaults to `num_cpus * 8`. If zero is specified, unlimited threads will
    /// be spawned instead of using a thread pool.
    pub fn pool_size(&mut self, pool_size: usize) -> &mut Self {
        self.pool_size = pool_size;
        self
    }

    /// Configure keep-alive timeout in seconds
    ///
    /// Defaults to 5 seconds
    pub fn set_keepalive_secs(&mut self, keep_alive_secs: u64) -> &mut Self {
        self.keep_alive_dur = time::Duration::from_secs(keep_alive_secs);
        self
    }

    /// Start the server using the given handler function
    pub fn start<F>(&self, func: F) -> Result<()>
        where F: Send + Sync + 'static + Fn(Request) -> Response<Vec<u8>>
    {
        let func = sync::Arc::new(func);
        let pool = Thread::new(self.pool_size);

        let mut sockets = slab::Slab::with_capacity(1024);
        let mut server = TcpListener::bind(self.addr)?;

        // initialize poll
        let mut poll = mio::Poll::new()?;
        {
            // register our tcp listener
            let entry = sockets.vacant_entry();
            let server_token = Token(entry.key());
            poll.registry().register(&mut server, server_token, Interest::READABLE)?;
            entry.insert(Socket::new_listener(server));
        }

        info!("** Listening on {} **", self.addr);

        let mut events = mio::Events::with_capacity(1024);
        loop {
            poll.poll(&mut events, None)?;
            'next_event: for e in &events {
                let token = e.token();
                match sockets.remove(token.into()) {
                    Socket::Listener { mut listener } => {
                        if e.is_readable() {
                            let (mut sock, addr) = listener.accept()?;
                            debug!("opened socket to: {:?}", addr);

                            // register the newly opened socket
                            let entry = sockets.vacant_entry();
                            let token = Token(entry.key());
                            poll.registry().register(&mut sock, token, Interest::READABLE | Interest::WRITABLE)?;
                            entry.insert(Socket::new_stream(sock, HttpStreamReader::new(), SocketStatus::New));
                        }
                        // reregister listener
                        let entry = sockets.vacant_entry();
                        let token = Token(entry.key());
                        poll.registry().reregister(&mut listener, token, Interest::READABLE)?;
                        entry.insert(Socket::new_listener(listener));
                    }
                    Socket::Stream { mut stream, mut keep_alive, socket_status, mut reader, request, mut done_reading,
                                     mut receiver, mut response, mut bytes_written } => {

                        // Try reading and parsing a request from this stream.
                        // `try_build_request` will return `None` until the request is parsed and the
                        // body is done being read. After that, `done_reading` will be set
                        // to `true`. At that point, if this socket is readable, we still need to
                        // check if it's been closed, but we will no longer try parsing the request
                        // bytes
                        let (mut request, err_response): (Option<RequestHead>, Option<ResponseWrapper>) = if e.is_readable() {
                            let mut buf = [0; 256];
                            let stream_close = loop {
                                match stream.read(&mut buf) {
                                    Ok(0) => {
                                        // the stream has ended for real
                                        break true
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
                                        break true
                                    }
                                }
                            };
                            if stream_close {
                                debug!("{:?} - Stream closed. Killing socket.", token);
                                // jump to the next mio event
                                // TODO: if we have a `receiver` (a handler is running)
                                //       try shutting it down
                                continue 'next_event
                            }
                            if done_reading {
                                (request, None)
                            } else {
                                match reader.try_build_request() {
                                    Ok(r) => (r, None),
                                    Err(e) => {
                                        // TODO: return the proper status-code per error
                                        error!("{:?} - Encountered error while parsing: {}", token, e);
                                        (None,
                                         Some(ResponseWrapper::new(
                                                 Response::builder().status(400).body(b"bad request".to_vec()).unwrap())))
                                    }
                                }
                            }
                        } else {
                            (request, None)
                        };
                        if request.is_some() || err_response.is_some() { done_reading = true; }

                        // Once the request is parsed, this block will execute once.
                        // The head-only request (RequestHead) will be converted into
                        // a public `Request` and the `HttpStreamReader`s `read_buf` will be
                        // swapped into the new `Request`s body. The provided
                        // `func` handler will be started for later retrieval
                        receiver = if let Some(req) = request.take() {
                            let (parts, _) = req.into_parts();
                            let mut body = vec![];
                            std::mem::swap(&mut body, &mut reader.read_buf);
                            let request = Request {
                                inner: http::Request::from_parts(parts, body),
                                body_start: reader.headers_length,
                            };

                            // Check for an explicit connection header, default to keep-alive = true
                            // TODO: This parsing needs to be improved to support all possible
                            //       values
                            keep_alive = {
                                request.headers().get(header::CONNECTION)
                                    .map(|v| v.as_bytes().eq_ignore_ascii_case(b"keep-alive"))
                                    .unwrap_or(keep_alive)
                            };
                            if socket_status == SocketStatus::New {
                                // Disable Nagle algorithm.
                                // The default setting (no_delay=false, Nagle enabled) causes a
                                // significant drop in performance for keep-alive connections
                                stream.set_nodelay(self.no_delay).unwrap();
                                // Remove by https://github.com/tokio-rs/mio/commit/02e9be41f27daf822575444fdd2b3067433a5996
                                //if keep_alive {
                                //    debug!("{:?} setting keep-alive", token);
                                //    stream.set_keepalive(Some(self.keep_alive_dur)).unwrap();
                                //}
                            }

                            // Kick-off the handler
                            let (send, recv) = channel();
                            let func = func.clone();
                            pool.execute(move || {
                                let resp = func(request);
                                let mut resp = ResponseWrapper::new(resp);
                                resp.serialize_headers();
                                // If sending fails there's nothing we can really do.
                                // The socket was probably closed and its receiver dropped
                                send.send(resp).ok();
                            });
                            Some(recv)
                        } else {
                            receiver
                        };

                        // See if a `ResponseWrapper` is available
                        response = if let Some(ref recv) = receiver {
                            recv.try_recv().ok()
                        } else {
                            if let Some(err_response) = err_response {
                                Some(err_response)
                            } else {
                                None
                            }
                        };

                        // If we have a `ResponseWrapper`, start writing its headers and body
                        // back to the stream
                        let mut done_write = false;
                        if let Some(ref resp) = response {
                            if e.is_writable() {
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
                                        break 'write
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
                                            continue 'next_event
                                        }
                                    }
                                }
                            }
                        }

                        if !done_write {
                            // we're not done writing our response to this socket yet
                            // reregister stream
                            let entry = sockets.vacant_entry();
                            let token = Token(entry.key());
                            poll.registry().reregister(&mut stream, token, Interest::READABLE | Interest::WRITABLE)?;
                            entry.insert(
                                Socket::continued_stream(
                                    stream, keep_alive, socket_status, reader, request, done_reading,
                                    receiver, response, bytes_written,
                                    )
                                );
                        } else if keep_alive {
                            // we're done writing, but we need to keep the socket open and reuse it
                            debug!("{:?} - Reusing stream", token);
                            let entry = sockets.vacant_entry();
                            let token = Token(entry.key());
                            poll.registry().reregister(&mut stream, token, Interest::READABLE | Interest::WRITABLE)?;
                            entry.insert(Socket::new_stream(stream, HttpStreamReader::new(), SocketStatus::Reused));
                        } else {
                            debug!("{:?} - Done writing, killing socket", token);
                        }
                    }
                }
            }
        }
    }
}

