extern crate mini_http;
extern crate log;
extern crate simple_logger;

use simple_logger::SimpleLogger;


fn run() -> Result<(), Box<dyn std::error::Error>> {
    SimpleLogger::new().init().unwrap();

    mini_http::Server::preopened()?
        .tcp_nodelay(true)
        .start(|_req| {
            mini_http::Response::builder()
                .status(200)
                .body(b"Hello!\n".to_vec())
                .unwrap()
        })?;
    Ok(())
}


pub fn main() {
    if let Err(e) = run() {
        eprintln!("Error: {:?}", e);
    }
}

