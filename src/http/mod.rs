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
pub const RESPONSE_BAD_REQUEST: &[u8] = b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\n\r\n";

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
    let header_end = find_header_end(buf)?;
    let (method, path, headers_len) = parse_first_line(buf)?;
    let content_length = find_content_length(&buf[headers_len..header_end]);
    let body_start = header_end;
    let body_end = body_start + content_length;
    if buf.len() < body_end {
        return None; // need more data
    }

    let keep_alive = !buf[..header_end]
        .windows(17)
        .any(|w| w.eq_ignore_ascii_case(b"Connection: close"));

    Some((
        Request {
            method,
            path: &buf[path.0..path.1],
            body: &buf[body_start..body_end],
            keep_alive,
        },
        body_end,
    ))
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n").map(|p| p + 4)
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
    let needle = b"Content-Length:";
    for window in headers.windows(needle.len()) {
        if window.eq_ignore_ascii_case(needle) {
            let start = window.as_ptr() as usize - headers.as_ptr() as usize + needle.len();
            let rest = &headers[start..];
            let val_start = rest.iter().position(|&b| !is_ws(b)).unwrap_or(0);
            let val_end = rest[val_start..]
                .iter()
                .position(|&b| b == b'\r' || is_ws(b))
                .unwrap_or(rest.len() - val_start);
            let mut n = 0usize;
            for &b in &rest[val_start..val_start + val_end] {
                if !b.is_ascii_digit() {
                    return 0;
                }
                n = n.saturating_mul(10).saturating_add((b - b'0') as usize);
            }
            return n;
        }
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
                    match parse_request(&buf[processed..used]) {
                        Some((req, consumed)) => {
                            let response = handler(&req);
                            if stream.write_all(response).is_err() {
                                return;
                            }
                            processed += consumed;
                            if !req.keep_alive {
                                return;
                            }
                        }
                        None => {
                            // Need more data
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
}
