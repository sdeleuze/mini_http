CARGO_TARGET_WASM32_WASI_RUNNER="wasmtime run --tcplisten 127.0.0.1:3000 --env FD_COUNT=1" cargo run --release --target wasm32-wasi --example hello_wasi
