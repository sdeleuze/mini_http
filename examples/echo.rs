extern crate mini_http;
extern crate env_logger;
#[macro_use] extern crate log;


fn init_logger() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::Builder::new()
        .format(|buf, record| {
            writeln!(buf, "[{}] - [{}] -> {}",
                record.level(),
                record.module_path().unwrap_or("<unkown>"),
                record.args()
                )
            })
        .parse(&::std::env::var("LOG").unwrap_or_default())
        .init();
    Ok(())
}


fn run() -> Result<(), Box<dyn std::error::Error>> {
    init_logger()?;
    mini_http::Server::new("127.0.0.1:3000")?
        .tcp_nodelay(true)
        .start(|request| {
            info!("request body: {:?}", std::str::from_utf8(request.body()));
            let resp = if request.body().len() > 0 {
                request.body().to_vec()
            } else {
                b"Send me data!\n`curl localhost:3000 -i -d 'data'`\n".to_vec()
            };
            mini_http::Response::builder()
                .status(200)
                .header("X-What-Up", "Nothin")
                .body(resp)
                .unwrap()
        })?;
    Ok(())
}


pub fn main() {
    if let Err(e) = run() {
        eprintln!("Error: {}", e);
    }
}

