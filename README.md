# mini_http

**Note:** This project is a work in progress and shouldn't be used in any critical production environment.

> A basic asynchronous&#42; http server using [`mio`](https://docs.rs/mio) modfied to compile to WASI.

&#42;While network IO is performed asynchronously, handler functions are executed synchronously.

## Status

Thanks to @sunfishcode and @haraldh help this should work. Notice [a special `mio` branch](https://github.com/haraldh/mio/tree/combine_events) is specified in the `Cargo.toml`.

Next step could be to update the server to handle `readable` and `writeable` events separately to not require a special Mio branch.

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

WASI system calls can be traced with `RUST_LOG=wasi_common=trace wasmtime ...`.
