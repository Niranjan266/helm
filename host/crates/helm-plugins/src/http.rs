//! Minimal HTTP/1.1 plumbing shared by the plugin host and the Home
//! Assistant client.
//!
//! [`HttpTransport`] is the seam: the agent can plug in a TLS-capable client.
//! [`StdHttpTransport`] is a small std-only client for plain `http://`
//! (the usual case for a LAN Home Assistant at `http://homeassistant.local:8123`).
//! It never follows redirects.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HttpError {
    #[error("invalid URL: {0}")]
    InvalidUrl(String),
    #[error("unsupported scheme `{0}`")]
    UnsupportedScheme(String),
    #[error("connection failed: {0}")]
    Connect(String),
    #[error("i/o error: {0}")]
    Io(String),
    #[error("timed out")]
    Timeout,
    #[error("response body exceeds {0} bytes")]
    BodyTooLarge(usize),
    #[error("malformed HTTP: {0}")]
    Protocol(String),
}

/// A parsed absolute `http`/`https` URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Url {
    pub scheme: String,
    /// Lower-cased host (IPv6 without brackets).
    pub host: String,
    pub port: u16,
    /// Always starts with `/`; fragment removed.
    pub path_and_query: String,
}

impl Url {
    pub fn parse(s: &str) -> Result<Url, HttpError> {
        let invalid = |why: &str| HttpError::InvalidUrl(format!("{why}: {s}"));
        if s.len() > 4096 || s.chars().any(|c| c.is_control() || c == ' ') {
            return Err(invalid("illegal characters"));
        }
        let (scheme, rest) = s
            .split_once("://")
            .ok_or_else(|| invalid("missing scheme"))?;
        let scheme = scheme.to_ascii_lowercase();
        let default_port = match scheme.as_str() {
            "http" => 80,
            "https" => 443,
            _ => return Err(HttpError::UnsupportedScheme(scheme)),
        };
        let (authority, path) = match rest.find(['/', '?', '#']) {
            Some(i) => (&rest[..i], &rest[i..]),
            None => (rest, ""),
        };
        if authority.contains('@') {
            return Err(invalid("userinfo not allowed"));
        }
        let (host, port_str) = if let Some(r) = authority.strip_prefix('[') {
            let (h, after) = r.split_once(']').ok_or_else(|| invalid("bad IPv6 host"))?;
            if !h
                .chars()
                .all(|c| c.is_ascii_hexdigit() || c == ':' || c == '.')
            {
                return Err(invalid("bad IPv6 host"));
            }
            match after {
                "" => (h, None),
                p => (
                    h,
                    Some(p.strip_prefix(':').ok_or_else(|| invalid("bad port"))?),
                ),
            }
        } else {
            match authority.rsplit_once(':') {
                Some((h, p)) => (h, Some(p)),
                None => (authority, None),
            }
        };
        if host.is_empty()
            || !(authority.starts_with('[')
                || host
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_')))
        {
            return Err(invalid("bad host"));
        }
        let port = match port_str {
            None => default_port,
            Some(p) => p.parse::<u16>().map_err(|_| invalid("bad port"))?,
        };
        let path = path.split('#').next().unwrap_or("");
        let path_and_query = if path.starts_with('/') {
            path.to_string()
        } else {
            format!("/{path}")
        };
        Ok(Url {
            scheme,
            host: host.to_ascii_lowercase(),
            port,
            path_and_query,
        })
    }

    fn default_port(&self) -> u16 {
        if self.scheme == "https" { 443 } else { 80 }
    }

    /// Value for the `Host` header.
    pub fn host_header(&self) -> String {
        let h = if self.host.contains(':') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };
        if self.port == self.default_port() {
            h
        } else {
            format!("{h}:{}", self.port)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpRequest {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl HttpRequest {
    pub fn get(url: impl Into<String>) -> Self {
        Self {
            method: "GET".into(),
            url: url.into(),
            headers: Vec::new(),
            body: Vec::new(),
        }
    }

    pub fn post(url: impl Into<String>, body: Vec<u8>) -> Self {
        Self {
            method: "POST".into(),
            url: url.into(),
            headers: Vec::new(),
            body,
        }
    }

    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl HttpResponse {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// Sends one request and returns the whole response.
pub trait HttpTransport: Send + Sync {
    fn send(&self, req: &HttpRequest) -> Result<HttpResponse, HttpError>;
}

/// std-only HTTP/1.1 client for `http://` URLs. `https://` is refused with
/// [`HttpError::UnsupportedScheme`]; supply a TLS transport for that.
#[derive(Debug, Clone)]
pub struct StdHttpTransport {
    pub connect_timeout: Duration,
    pub io_timeout: Duration,
    pub max_body: usize,
}

impl Default for StdHttpTransport {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(5),
            io_timeout: Duration::from_secs(10),
            max_body: 4 * 1024 * 1024,
        }
    }
}

const MAX_HEAD_BYTES: usize = 64 * 1024;

fn io_err(e: std::io::Error) -> HttpError {
    match e.kind() {
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => HttpError::Timeout,
        _ => HttpError::Io(e.to_string()),
    }
}

fn check_token(s: &str, what: &str) -> Result<(), HttpError> {
    if s.is_empty() || s.chars().any(|c| c == '\r' || c == '\n' || c == '\0') {
        return Err(HttpError::Protocol(format!("illegal {what}")));
    }
    Ok(())
}

impl HttpTransport for StdHttpTransport {
    fn send(&self, req: &HttpRequest) -> Result<HttpResponse, HttpError> {
        let url = Url::parse(&req.url)?;
        if url.scheme != "http" {
            return Err(HttpError::UnsupportedScheme(format!(
                "{} (StdHttpTransport is plain HTTP only; plug in a TLS transport)",
                url.scheme
            )));
        }
        check_token(&req.method, "method")?;
        if !req.method.chars().all(|c| c.is_ascii_uppercase()) {
            return Err(HttpError::Protocol("illegal method".into()));
        }
        let mut head = format!(
            "{} {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\nUser-Agent: helm-plugins/{}\r\n",
            req.method,
            url.path_and_query,
            url.host_header(),
            env!("CARGO_PKG_VERSION")
        );
        for (k, v) in &req.headers {
            check_token(k, "header name")?;
            if v.contains(['\r', '\n', '\0']) {
                return Err(HttpError::Protocol("illegal header value".into()));
            }
            head.push_str(&format!("{k}: {v}\r\n"));
        }
        if !req.body.is_empty() || req.method == "POST" || req.method == "PUT" {
            head.push_str(&format!("Content-Length: {}\r\n", req.body.len()));
        }
        head.push_str("\r\n");

        let addrs = (url.host.as_str(), url.port)
            .to_socket_addrs()
            .map_err(|e| HttpError::Connect(e.to_string()))?;
        let mut last = HttpError::Connect("no addresses".into());
        let mut stream = None;
        for a in addrs {
            match TcpStream::connect_timeout(&a, self.connect_timeout) {
                Ok(s) => {
                    stream = Some(s);
                    break;
                }
                Err(e) => last = HttpError::Connect(e.to_string()),
            }
        }
        let mut stream = stream.ok_or(last)?;
        stream
            .set_read_timeout(Some(self.io_timeout))
            .map_err(io_err)?;
        stream
            .set_write_timeout(Some(self.io_timeout))
            .map_err(io_err)?;
        stream.write_all(head.as_bytes()).map_err(io_err)?;
        stream.write_all(&req.body).map_err(io_err)?;
        stream.flush().map_err(io_err)?;
        read_response(BufReader::new(stream), self.max_body, req.method == "HEAD")
    }
}

fn read_line<R: BufRead>(r: &mut R, budget: &mut usize) -> Result<String, HttpError> {
    let mut buf = Vec::new();
    let n = r
        .by_ref()
        .take(*budget as u64 + 1)
        .read_until(b'\n', &mut buf)
        .map_err(io_err)?;
    if n > *budget {
        return Err(HttpError::Protocol("header section too large".into()));
    }
    *budget -= n;
    if n == 0 {
        return Err(HttpError::Protocol("unexpected end of stream".into()));
    }
    let s = String::from_utf8(buf).map_err(|_| HttpError::Protocol("non-UTF-8 header".into()))?;
    Ok(s.trim_end_matches(['\r', '\n']).to_string())
}

/// Parse a full HTTP/1.1 response from `r`.
pub(crate) fn read_response<R: BufRead>(
    mut r: R,
    max_body: usize,
    head_only: bool,
) -> Result<HttpResponse, HttpError> {
    let mut budget = MAX_HEAD_BYTES;
    let status_line = read_line(&mut r, &mut budget)?;
    let mut parts = status_line.splitn(3, ' ');
    let version = parts.next().unwrap_or("");
    if !version.starts_with("HTTP/1.") {
        return Err(HttpError::Protocol(format!(
            "bad status line `{status_line}`"
        )));
    }
    let status: u16 = parts
        .next()
        .and_then(|s| s.parse().ok())
        .filter(|s| (100..1000).contains(s))
        .ok_or_else(|| HttpError::Protocol(format!("bad status line `{status_line}`")))?;
    let mut headers = Vec::new();
    loop {
        let line = read_line(&mut r, &mut budget)?;
        if line.is_empty() {
            break;
        }
        let (k, v) = line
            .split_once(':')
            .ok_or_else(|| HttpError::Protocol("bad header line".into()))?;
        headers.push((k.trim().to_string(), v.trim().to_string()));
    }
    let resp_header = |name: &str| {
        headers
            .iter()
            .find(|(k, _): &&(String, String)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.clone())
    };
    let no_body = head_only || status == 204 || status == 304 || (100..200).contains(&status);
    let body = if no_body {
        Vec::new()
    } else if resp_header("transfer-encoding")
        .is_some_and(|v| v.to_ascii_lowercase().contains("chunked"))
    {
        read_chunked(&mut r, max_body)?
    } else if let Some(len) = resp_header("content-length") {
        let len: usize = len
            .parse()
            .map_err(|_| HttpError::Protocol("bad content-length".into()))?;
        if len > max_body {
            return Err(HttpError::BodyTooLarge(max_body));
        }
        let mut body = vec![0; len];
        r.read_exact(&mut body).map_err(io_err)?;
        body
    } else {
        let mut body = Vec::new();
        r.take(max_body as u64 + 1)
            .read_to_end(&mut body)
            .map_err(io_err)?;
        if body.len() > max_body {
            return Err(HttpError::BodyTooLarge(max_body));
        }
        body
    };
    Ok(HttpResponse {
        status,
        headers,
        body,
    })
}

fn read_chunked<R: BufRead>(r: &mut R, max_body: usize) -> Result<Vec<u8>, HttpError> {
    let mut body = Vec::new();
    let mut budget = MAX_HEAD_BYTES;
    loop {
        let line = read_line(r, &mut budget)?;
        let size_str = line.split(';').next().unwrap_or("").trim();
        let size = usize::from_str_radix(size_str, 16)
            .map_err(|_| HttpError::Protocol("bad chunk size".into()))?;
        if size == 0 {
            // Trailers, then the blank line.
            while !read_line(r, &mut budget)?.is_empty() {}
            return Ok(body);
        }
        if body.len().saturating_add(size) > max_body {
            return Err(HttpError::BodyTooLarge(max_body));
        }
        let start = body.len();
        body.resize(start + size, 0);
        r.read_exact(&mut body[start..]).map_err(io_err)?;
        let mut crlf = [0u8; 2];
        r.read_exact(&mut crlf).map_err(io_err)?;
        if &crlf != b"\r\n" {
            return Err(HttpError::Protocol("missing chunk terminator".into()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_urls() {
        let u = Url::parse("http://HomeAssistant.local:8123/api/states?x=1#frag").unwrap();
        assert_eq!(u.host, "homeassistant.local");
        assert_eq!(u.port, 8123);
        assert_eq!(u.path_and_query, "/api/states?x=1");
        assert_eq!(u.host_header(), "homeassistant.local:8123");
        let u = Url::parse("https://example.com").unwrap();
        assert_eq!((u.port, u.path_and_query.as_str()), (443, "/"));
        assert_eq!(u.host_header(), "example.com");
        let u = Url::parse("http://[::1]:9000/x").unwrap();
        assert_eq!((u.host.as_str(), u.port), ("::1", 9000));
        assert_eq!(u.host_header(), "[::1]:9000");
        for bad in [
            "ftp://x/",
            "example.com",
            "http://user@host/",
            "http://ho st/",
            "http://host:99999/",
            "http:///path",
            "http://a\r\nb/",
        ] {
            assert!(Url::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn reads_chunked_and_length_bodies() {
        let raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4\r\nWiki\r\n5;x=y\r\npedia\r\n0\r\nT: 1\r\n\r\n";
        let r = read_response(&raw[..], 100, false).unwrap();
        assert_eq!(r.body, b"Wikipedia");
        let raw = b"HTTP/1.1 404 Not Found\r\nContent-Length: 3\r\n\r\nabc";
        let r = read_response(&raw[..], 100, false).unwrap();
        assert_eq!((r.status, r.body.as_slice()), (404, &b"abc"[..]));
        let raw = b"HTTP/1.1 200 OK\r\nContent-Length: 300\r\n\r\n";
        assert_eq!(
            read_response(&raw[..], 100, false),
            Err(HttpError::BodyTooLarge(100))
        );
        let raw = b"HTTP/1.0 200 OK\r\n\r\nuntil-eof";
        assert_eq!(
            read_response(&raw[..], 100, false).unwrap().body,
            b"until-eof"
        );
        assert!(read_response(&b"garbage\r\n\r\n"[..], 100, false).is_err());
    }
}
