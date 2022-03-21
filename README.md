# mini_http

**Note:** This project is a work in progress and shouldn't be used in any critical production environment.

> A basic asynchronous&#42; http server using [`mio`](https://docs.rs/mio) modfied to compile to WASI.

&#42;While network IO is performed asynchronously, handler functions are executed synchronously.

## Status

WASI version is currently broken for some reason.

### Native

Run `cargo run --example hello` then in another terminal `curl http://127.0.0.1:3000` which displays successfully `Hello!`.

```
2022-03-22T07:17:02.085Z TRACE [mio::poll] registering event source with poller: token=Token(0), interests=READABLE
2022-03-22T07:17:02.085Z INFO [mini_http] ** Listening on 127.0.0.1:3000 **
2022-03-22T07:17:02.085Z DEBUG [mini_http] Beginning of loop
2022-03-22T07:17:07.864Z DEBUG [mini_http] opened socket to: 127.0.0.1:33512
2022-03-22T07:17:07.864Z TRACE [mio::poll] registering event source with poller: token=Token(0), interests=READABLE | WRITABLE
2022-03-22T07:17:07.864Z TRACE [mio::poll] reregistering event source with poller: token=Token(1), interests=READABLE
2022-03-22T07:17:07.864Z DEBUG [mini_http] Beginning of loop
2022-03-22T07:17:07.864Z DEBUG [mini_http] Token(0) - Read 82 bytes
2022-03-22T07:17:07.865Z DEBUG [mini_http::http_stream] headers complete: 82, Ok("GET / HTTP/1.1\r\nHost: 127.0.0.1:3000\r\nUser-Agent: curl/7.81.0-DEV\r\nAccept: */*\r\n\r\n")
2022-03-22T07:17:07.865Z DEBUG [mini_http::http_stream] trailing body bytes read: 0, Ok("")
2022-03-22T07:17:07.866Z DEBUG [mini_http] Reading is done for token Token(0)
2022-03-22T07:17:07.866Z DEBUG [mini_http] Begin processing the response for token Token(0)
2022-03-22T07:17:07.866Z DEBUG [mini_http] Headers serialized for token Token(0)
2022-03-22T07:17:07.866Z DEBUG [mini_http] Response body ready to be written for token Token(0)
2022-03-22T07:17:07.866Z DEBUG [mini_http] Body writeable for token Token(0)
2022-03-22T07:17:07.866Z DEBUG [mini_http] Token(0) - Wrote 64 bytes
2022-03-22T07:17:07.866Z DEBUG [mini_http] Token(0) - Wrote 7 bytes
2022-03-22T07:17:07.866Z DEBUG [mini_http] Token(0) - flushing
2022-03-22T07:17:07.866Z DEBUG [mini_http] Token(0) - Done writing, killing socket
2022-03-22T07:17:07.866Z DEBUG [mini_http] Beginning of loop
```

### WASI

Run `cargo +nightly build --target wasm32-wasi --example hello_wasi` then
`wasmtime run --tcplisten 127.0.0.1:3000 --env 'LISTEN_FDS=1' target/wasm32-wasi/debug/examples/hello_wasi.wasm`.
In another terminal run `curl http://127.0.0.1:3000`, it fails with `curl: (52) Empty reply from server`.

The server has crashed with the following logs:
```
2022-03-22T07:19:33.940Z TRACE [mio::poll] registering event source with poller: token=Token(0), interests=READABLE
2022-03-22T07:19:33.940Z INFO  [mini_http] ** Using preopened socket FD 3 **
2022-03-22T07:19:33.940Z DEBUG [mini_http] Beginning of loop
2022-03-22T07:19:37.645Z DEBUG [mini_http] opened socket to: 0.0.0.0:0
2022-03-22T07:19:37.645Z TRACE [mio::poll] registering event source with poller: token=Token(0), interests=READABLE | WRITABLE
2022-03-22T07:19:37.645Z TRACE [mio::poll] reregistering event source with poller: token=Token(1), interests=READABLE
2022-03-22T07:19:37.645Z DEBUG [mini_http] Beginning of loop
2022-03-22T07:19:37.645Z DEBUG [mini_http] Token(0) - Read 82 bytes
2022-03-22T07:19:37.645Z DEBUG [mini_http::http_stream] headers complete: 82, Ok("GET / HTTP/1.1\r\nHost: 127.0.0.1:3000\r\nUser-Agent: curl/7.81.0-DEV\r\nAccept: */*\r\n\r\n")
2022-03-22T07:19:37.645Z DEBUG [mini_http::http_stream] trailing body bytes read: 0, Ok("")
2022-03-22T07:19:37.645Z DEBUG [mini_http] Reading is done for token Token(0)
2022-03-22T07:19:37.645Z DEBUG [mini_http] Begin processing the response for token Token(0)
2022-03-22T07:19:37.645Z DEBUG [mini_http] Headers serialized for token Token(0)
2022-03-22T07:19:37.645Z DEBUG [mini_http] Response body ready to be written for token Token(0)
2022-03-22T07:19:37.645Z DEBUG [mini_http] Body not writeable for token Token(0)
2022-03-22T07:19:37.645Z DEBUG [mini_http] Write not done, reregister stream for token Token(0)
2022-03-22T07:19:37.645Z TRACE [mio::poll] reregistering event source with poller: token=Token(0), interests=READABLE | WRITABLE
2022-03-22T07:19:37.645Z DEBUG [mini_http] Beginning of loop
Error: Error(Io(Os { code: 8, kind: Uncategorized, message: "Bad file descriptor" }), State { next_error: None, backtrace: InternalBacktrace { backtrace: None } })
```

## Usage

See [`examples`](https://github.com/sdeleuze/mini_http/tree/main/examples)

```rust
extern crate mini_http;

fn run() -> Result<(), Box<std::error::Error>> {
    mini_http::Server::new("127.0.0.1:3000")?
        .start(|request| {
            println!("{:?}", std::str::from_utf8(request.body()));
            let resp = if request.body().len() > 0 {
                request.body().to_vec()
            } else {
                b"hello!".to_vec()
            };
            mini_http::Response::builder()
                .status(200)
                .header("X-What-Up", "Nothin")
                .body(resp)
                .unwrap()
        })?;
    Ok(())
}
```

