use http;

error_chain! {
    foreign_links {
        Io(::std::io::Error);
        Utf8(::std::str::Utf8Error);
        AddrParse(::std::net::AddrParseError);
        Http(http::Error);
    }
    errors {
        MalformedHttpRequest(s: String) {
            description("Malformed HTTP Request")
            display("MalformedHttpRequest: {}", s)
        }
        IncompleteHttpRequest(s: String) {
            description("Incomplete HTTP Request")
            display("IncompleteHttpRequest: {}", s)
        }
        RequestHeadersTooLarge(s: String) {
            description("Request Headers Too Large")
            display("RequestHeadersTooLarge: {}", s)
        }
        RequestBodyTooLarge(s: String) {
            description("Request Body Too Large")
            display("RequestBodyTooLarge: {}", s)
        }
    }
}
