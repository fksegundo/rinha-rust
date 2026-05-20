use std::io::{Read, Write};
use std::net::TcpStream;

pub const RESPONSE_READY: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok";
pub const RESPONSE_FRAUD_0: &[u8] =
    b"HTTP/1.1 200 OK\r\nContent-Length: 35\r\n\r\n{\"approved\":true,\"fraud_score\":0.0}";
pub const RESPONSE_FRAUD_1: &[u8] =
    b"HTTP/1.1 200 OK\r\nContent-Length: 35\r\n\r\n{\"approved\":true,\"fraud_score\":0.2}";
pub const RESPONSE_FRAUD_2: &[u8] =
    b"HTTP/1.1 200 OK\r\nContent-Length: 35\r\n\r\n{\"approved\":true,\"fraud_score\":0.4}";
pub const RESPONSE_FRAUD_3: &[u8] =
    b"HTTP/1.1 200 OK\r\nContent-Length: 36\r\n\r\n{\"approved\":false,\"fraud_score\":0.6}";
pub const RESPONSE_FRAUD_4: &[u8] =
    b"HTTP/1.1 200 OK\r\nContent-Length: 36\r\n\r\n{\"approved\":false,\"fraud_score\":0.8}";
pub const RESPONSE_FRAUD_5: &[u8] =
    b"HTTP/1.1 200 OK\r\nContent-Length: 36\r\n\r\n{\"approved\":false,\"fraud_score\":1.0}";
pub const RESPONSE_BAD_REQUEST: &[u8] =
    b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
pub const RESPONSE_NOT_FOUND: &[u8] =
    b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";

const MAX_BODY_LEN: usize = 8192;

pub const FRAUD_RESPONSES: [&[u8]; 6] = [
    RESPONSE_FRAUD_0,
    RESPONSE_FRAUD_1,
    RESPONSE_FRAUD_2,
    RESPONSE_FRAUD_3,
    RESPONSE_FRAUD_4,
    RESPONSE_FRAUD_5,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
}

pub struct Request<'a> {
    pub method: Method,
    pub path: &'a [u8],
    pub body: &'a [u8],
    pub keep_alive: bool,
}

pub fn parse_request(buf: &[u8]) -> Option<(Request<'_>, usize)> {
    match parse_request_result(buf) {
        ParseResult::Complete(req, consumed) => Some((req, consumed)),
        ParseResult::NeedMore | ParseResult::Reject(_, _) => None,
    }
}

pub(crate) enum ParseResult<'a> {
    Complete(Request<'a>, usize),
    Reject(&'static [u8], usize),
    NeedMore,
}

pub(crate) fn parse_request_result(buf: &[u8]) -> ParseResult<'_> {
    let header_end = match find_header_end(buf) {
        Some(header_end) => header_end,
        None => return ParseResult::NeedMore,
    };
    let (method, path, headers_len) = match parse_first_line(buf) {
        Some(parts) => parts,
        None => return ParseResult::Reject(RESPONSE_BAD_REQUEST, header_end),
    };
    let content_length = find_content_length(&buf[headers_len..header_end]);

    let path_bytes = &buf[path.0..path.1];
    if let Some(response) = early_rejection(method, path_bytes, content_length) {
        return ParseResult::Reject(response, header_end);
    }

    let body_start = header_end;
    let body_end = body_start + content_length;
    if buf.len() < body_end {
        return ParseResult::NeedMore;
    }

    let keep_alive = !buf[..header_end]
        .windows(17)
        .any(|w| w.eq_ignore_ascii_case(b"Connection: close"));

    ParseResult::Complete(
        Request {
            method,
            path: &buf[path.0..path.1],
            body: &buf[body_start..body_end],
            keep_alive,
        },
        body_end,
    )
}

fn early_rejection(method: Method, path: &[u8], content_length: usize) -> Option<&'static [u8]> {
    match (method, path) {
        (Method::Get, b"/ready") => None,
        (Method::Post, b"/fraud-score") if content_length <= MAX_BODY_LEN => None,
        (Method::Post, b"/fraud-score") => Some(RESPONSE_BAD_REQUEST),
        _ => Some(RESPONSE_NOT_FOUND),
    }
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    let n = buf.len();
    let mut i = 3;
    while i < n {
        if buf[i] == b'\n' && buf[i - 1] == b'\r' && buf[i - 2] == b'\n' && buf[i - 3] == b'\r' {
            return Some(i + 1);
        }
        i += 1;
    }
    None
}

fn parse_first_line(buf: &[u8]) -> Option<(Method, (usize, usize), usize)> {
    let end = buf.iter().position(|&b| b == b'\r')?;
    let method_end = buf[..end].iter().position(|&b| b == b' ')?;
    let method = if &buf[..method_end] == b"GET" {
        Method::Get
    } else if &buf[..method_end] == b"POST" {
        Method::Post
    } else {
        return None;
    };

    let path_start = method_end + 1;
    if path_start >= end {
        return None;
    }
    let rel_path_end = buf[path_start..end].iter().position(|&b| b == b' ')?;
    let path_end = path_start + rel_path_end;
    if path_end == path_start {
        return None;
    }
    Some((method, (path_start, path_end), end + 2))
}

fn find_content_length(headers: &[u8]) -> usize {
    const NEEDLE: &[u8] = b"content-length:";
    let n = headers.len();
    if n < NEEDLE.len() {
        return 0;
    }
    let mut i = 0;
    while i + NEEDLE.len() <= n {
        if headers[i].to_ascii_lowercase() == b'c' {
            let window = &headers[i..i + NEEDLE.len()];
            if window.eq_ignore_ascii_case(NEEDLE) {
                let rest = &headers[i + NEEDLE.len()..];
                let val_start = rest.iter().position(|&b| !is_ws(b)).unwrap_or(0);
                let val_end = rest[val_start..]
                    .iter()
                    .position(|&b| b == b'\r' || is_ws(b))
                    .unwrap_or(rest.len() - val_start);
                let mut num = 0usize;
                for &b in &rest[val_start..val_start + val_end] {
                    if !b.is_ascii_digit() {
                        return 0;
                    }
                    num = num.saturating_mul(10).saturating_add((b - b'0') as usize);
                }
                return num;
            }
        }
        i += 1;
    }
    0
}

fn is_ws(b: u8) -> bool {
    matches!(b, b' ' | b'\t')
}

pub fn handle_connection<F>(mut stream: TcpStream, mut handler: F)
where
    F: FnMut(&Request) -> &'static [u8],
{
    let mut buf = [0u8; 8192];
    let mut used = 0usize;
    loop {
        match stream.read(&mut buf[used..]) {
            Ok(0) => break,
            Ok(n) => {
                used += n;
                let mut processed = 0usize;
                while processed < used {
                    match parse_request_result(&buf[processed..used]) {
                        ParseResult::Complete(req, consumed) => {
                            let response = handler(&req);
                            if stream.write_all(response).is_err() {
                                return;
                            }
                            processed += consumed;
                            if !req.keep_alive {
                                return;
                            }
                        }
                        ParseResult::Reject(response, _consumed) => {
                            let _ = stream.write_all(response);
                            return;
                        }
                        ParseResult::NeedMore => {
                            if used >= buf.len() {
                                let _ = stream.write_all(RESPONSE_BAD_REQUEST);
                                return;
                            }
                            break;
                        }
                    }
                }
                if processed > 0 {
                    buf.copy_within(processed..used, 0);
                    used -= processed;
                }
            }
            Err(_) => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::{Shutdown, TcpListener, TcpStream};
    use std::thread;
    use std::time::Duration;

    #[test]
    fn parses_first_line_without_allocating_parts() {
        let request =
            b"POST /fraud-score HTTP/1.1\r\nHost: localhost\r\nContent-Length: 2\r\n\r\n{}";
        let (parsed, consumed) = parse_request(request).expect("request should parse");

        assert_eq!(parsed.method, Method::Post);
        assert_eq!(parsed.path, b"/fraud-score");
        assert_eq!(parsed.body, b"{}");
        assert_eq!(consumed, request.len());
    }

    #[test]
    fn parses_content_length_digits_directly() {
        assert_eq!(
            find_content_length(b"Host: x\r\nContent-Length: 123\r\n"),
            123
        );
        assert_eq!(find_content_length(b"content-length:\t42\r\n"), 42);
        assert_eq!(find_content_length(b"Content-Length: nope\r\n"), 0);
    }

    #[test]
    fn rejects_unknown_path_after_headers_without_waiting_for_body() {
        let request = b"POST /missing HTTP/1.1\r\nHost: localhost\r\nContent-Length: 64000\r\n\r\n";

        match parse_request_result(request) {
            ParseResult::Reject(response, consumed) => {
                assert_eq!(response, RESPONSE_NOT_FOUND);
                assert_eq!(consumed, request.len());
            }
            _ => panic!("unknown path should be rejected after headers"),
        }
    }

    #[test]
    fn rejects_oversized_fraud_body_after_headers_without_waiting_for_body() {
        let request =
            b"POST /fraud-score HTTP/1.1\r\nHost: localhost\r\nContent-Length: 64000\r\n\r\n";

        match parse_request_result(request) {
            ParseResult::Reject(response, consumed) => {
                assert_eq!(response, RESPONSE_BAD_REQUEST);
                assert_eq!(consumed, request.len());
            }
            _ => panic!("oversized fraud body should be rejected after headers"),
        }
    }

    #[test]
    fn oversized_request_is_rejected_when_buffer_fills() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test listener");
        let addr = listener.local_addr().expect("listener addr");

        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().expect("accept test connection");
            handle_connection(stream, |_| RESPONSE_READY);
        });

        let mut stream = TcpStream::connect(addr).expect("connect test server");
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("set read timeout");

        let body = vec![b'x'; 9_000];
        write!(
            stream,
            "POST /fraud-score HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\n\r\n",
            body.len()
        )
        .expect("write headers");
        stream.write_all(&body).expect("write body");
        stream.shutdown(Shutdown::Write).expect("shutdown write");

        let mut response = [0u8; 128];
        let n = stream.read(&mut response).expect("read response");
        server.join().expect("server thread");

        assert!(response[..n].starts_with(RESPONSE_BAD_REQUEST));
    }
}
