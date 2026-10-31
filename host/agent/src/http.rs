//! HTTPS for plugins and Home Assistant: an [`HttpTransport`] that speaks `https://` with the
//! agent's rustls (ring, Mozilla roots from `webpki-roots`) and hands `http://` to the
//! std-only client of `helm-plugins`. One request per connection (`Connection: close`),
//! no redirects, bounded response size.

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::time::Duration;

use helm_plugins::http::{
    HttpError, HttpRequest, HttpResponse, HttpTransport, StdHttpTransport, Url,
};
use rustls::pki_types::ServerName;

const MAX_HEAD_BYTES: usize = 64 * 1024;

pub struct TlsHttpTransport {
    plain: StdHttpTransport,
    tls: Arc<rustls::ClientConfig>,
    pub connect_timeout: Duration,
    pub io_timeout: Duration,
    pub max_body: usize,
}

impl std::fmt::Debug for TlsHttpTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TlsHttpTransport")
            .field("max_body", &self.max_body)
            .finish_non_exhaustive()
    }
}

impl Default for TlsHttpTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl TlsHttpTransport {
    /// Trusts the public web PKI (webpki roots).
    pub fn new() -> TlsHttpTransport {
        let mut roots = rustls::RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        Self::with_roots(roots)
    }

    /// Trusts exactly `roots` (tests use a self-signed root).
    pub fn with_roots(roots: rustls::RootCertStore) -> TlsHttpTransport {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let tls = rustls::ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .expect("ring supports the default TLS versions")
            .with_root_certificates(roots)
            .with_no_client_auth();
        let plain = StdHttpTransport::default();
        TlsHttpTransport {
            connect_timeout: plain.connect_timeout,
            io_timeout: plain.io_timeout,
            max_body: plain.max_body,
            plain,
            tls: Arc::new(tls),
        }
    }

    fn connect(&self, url: &Url) -> Result<TcpStream, HttpError> {
        let addrs = (url.host.as_str(), url.port)
            .to_socket_addrs()
            .map_err(|e| HttpError::Connect(e.to_string()))?;
        let mut last = HttpError::Connect("no addresses".into());
        for a in addrs {
            match TcpStream::connect_timeout(&a, self.connect_timeout) {
                Ok(s) => {
                    s.set_read_timeout(Some(self.io_timeout)).map_err(io_err)?;
                    s.set_write_timeout(Some(self.io_timeout)).map_err(io_err)?;
                    return Ok(s);
                }
                Err(e) => last = HttpError::Connect(e.to_string()),
            }
        }
        Err(last)
    }
}

fn io_err(e: std::io::Error) -> HttpError {
    match e.kind() {
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => HttpError::Timeout,
        _ => HttpError::Io(e.to_string()),
    }
}

/// `METHOD path HTTP/1.1` + headers + blank line, refusing CR/LF injection.
pub fn request_head(req: &HttpRequest, url: &Url) -> Result<String, HttpError> {
    if req.method.is_empty() || !req.method.chars().all(|c| c.is_ascii_uppercase()) {
        return Err(HttpError::Protocol("illegal method".into()));
    }
    let mut head = format!(
        "{} {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\nUser-Agent: helm-agent/{}\r\n",
        req.method,
        url.path_and_query,
        url.host_header(),
        env!("CARGO_PKG_VERSION")
    );
    for (k, v) in &req.headers {
        if k.is_empty() || k.contains([':', '\r', '\n', '\0', ' ']) {
            return Err(HttpError::Protocol("illegal header name".into()));
        }
        if v.contains(['\r', '\n', '\0']) {
            return Err(HttpError::Protocol("illegal header value".into()));
        }
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    if !req.body.is_empty() || req.method == "POST" || req.method == "PUT" {
        head.push_str(&format!("Content-Length: {}\r\n", req.body.len()));
    }
    head.push_str("\r\n");
    Ok(head)
}

/// Parse a complete HTTP/1.x response (the connection was read to its end).
pub fn parse_response(
    raw: &[u8],
    max_body: usize,
    head_only: bool,
) -> Result<HttpResponse, HttpError> {
    let end = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or_else(|| HttpError::Protocol("incomplete response head".into()))?;
    if end > MAX_HEAD_BYTES {
        return Err(HttpError::Protocol("header section too large".into()));
    }
    let head = std::str::from_utf8(&raw[..end])
        .map_err(|_| HttpError::Protocol("non-UTF-8 header".into()))?;
    let mut lines = head.split("\r\n");
    let status_line = lines.next().unwrap_or("");
    let mut parts = status_line.splitn(3, ' ');
    if !parts.next().unwrap_or("").starts_with("HTTP/1.") {
        return Err(HttpError::Protocol(format!("bad status line `{status_line}`")));
    }
    let status: u16 = parts
        .next()
        .and_then(|s| s.parse().ok())
        .filter(|s| (100..1000).contains(s))
        .ok_or_else(|| HttpError::Protocol(format!("bad status line `{status_line}`")))?;
    let mut headers = Vec::new();
    for line in lines {
        let (k, v) = line
            .split_once(':')
            .ok_or_else(|| HttpError::Protocol("bad header line".into()))?;
        headers.push((k.trim().to_string(), v.trim().to_string()));
    }
    let header = |name: &str| {
        headers
            .iter()
            .find(|(k, _): &&(String, String)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.clone())
    };
    let rest = &raw[end + 4..];
    let no_body = head_only || status == 204 || status == 304 || (100..200).contains(&status);
    let body = if no_body {
        Vec::new()
    } else if header("transfer-encoding").is_some_and(|v| v.to_ascii_lowercase().contains("chunked"))
    {
        dechunk(rest, max_body)?
    } else if let Some(len) = header("content-length") {
        let len: usize = len
            .parse()
            .map_err(|_| HttpError::Protocol("bad content-length".into()))?;
        if len > max_body {
            return Err(HttpError::BodyTooLarge(max_body));
        }
        if rest.len() < len {
            return Err(HttpError::Protocol("body shorter than content-length".into()));
        }
        rest[..len].to_vec()
    } else {
        if rest.len() > max_body {
            return Err(HttpError::BodyTooLarge(max_body));
        }
        rest.to_vec()
    };
    Ok(HttpResponse {
        status,
        headers,
        body,
    })
}

fn dechunk(mut data: &[u8], max_body: usize) -> Result<Vec<u8>, HttpError> {
    let bad = || HttpError::Protocol("bad chunked body".into());
    let mut body = Vec::new();
    loop {
        let eol = data.windows(2).position(|w| w == b"\r\n").ok_or_else(bad)?;
        let line = std::str::from_utf8(&data[..eol]).map_err(|_| bad())?;
        let size = usize::from_str_radix(line.split(';').next().unwrap_or("").trim(), 16)
            .map_err(|_| bad())?;
        data = &data[eol + 2..];
        if size == 0 {
            return Ok(body); // trailers ignored
        }
        if body.len().saturating_add(size) > max_body {
            return Err(HttpError::BodyTooLarge(max_body));
        }
        if data.len() < size + 2 || &data[size..size + 2] != b"\r\n" {
            return Err(bad());
        }
        body.extend_from_slice(&data[..size]);
        data = &data[size + 2..];
    }
}

impl HttpTransport for TlsHttpTransport {
    fn send(&self, req: &HttpRequest) -> Result<HttpResponse, HttpError> {
        let url = Url::parse(&req.url)?;
        if url.scheme == "http" {
            return self.plain.send(req);
        }
        let head = request_head(req, &url)?;
        let name = ServerName::try_from(url.host.clone())
            .map_err(|_| HttpError::InvalidUrl(format!("bad TLS server name {}", url.host)))?;
        let conn = rustls::ClientConnection::new(self.tls.clone(), name)
            .map_err(|e| HttpError::Connect(format!("TLS: {e}")))?;
        let tcp = self.connect(&url)?;
        let mut tls = rustls::StreamOwned::new(conn, tcp);
        tls.write_all(head.as_bytes()).map_err(tls_err)?;
        tls.write_all(&req.body).map_err(tls_err)?;
        tls.flush().map_err(tls_err)?;
        let limit = self.max_body + MAX_HEAD_BYTES + 1;
        let mut raw = Vec::new();
        let mut buf = [0u8; 16 * 1024];
        loop {
            match tls.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    raw.extend_from_slice(&buf[..n]);
                    if raw.len() > limit {
                        return Err(HttpError::BodyTooLarge(self.max_body));
                    }
                }
                // Many servers close without close_notify; what arrived is complete enough
                // for the parser to judge (it checks content-length / chunk framing).
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof && !raw.is_empty() => {
                    break;
                }
                Err(e) => return Err(tls_err(e)),
            }
        }
        parse_response(&raw, self.max_body, req.method == "HEAD")
    }
}

fn tls_err(e: std::io::Error) -> HttpError {
    if e.get_ref().is_some_and(|inner| inner.is::<rustls::Error>()) {
        HttpError::Connect(format!("TLS: {e}"))
    } else {
        io_err(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn parses_length_chunked_and_eof_bodies() {
        let r = parse_response(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nabcEXTRA", 100, false)
            .unwrap();
        assert_eq!((r.status, r.body.as_slice()), (200, &b"abc"[..]));
        let r = parse_response(
            b"HTTP/1.1 201 Created\r\nTransfer-Encoding: chunked\r\n\r\n4\r\nWiki\r\n5;x\r\npedia\r\n0\r\n\r\n",
            100,
            false,
        )
        .unwrap();
        assert_eq!(r.body, b"Wikipedia");
        assert_eq!(r.header("transfer-encoding"), Some("chunked"));
        let r = parse_response(b"HTTP/1.0 404 Not Found\r\n\r\nuntil-eof", 100, false).unwrap();
        assert_eq!((r.status, r.body.as_slice()), (404, &b"until-eof"[..]));
        assert_eq!(
            parse_response(b"HTTP/1.1 200 OK\r\nContent-Length: 300\r\n\r\n", 100, false),
            Err(HttpError::BodyTooLarge(100))
        );
        assert!(parse_response(b"HTTP/1.1 200 OK\r\nContent-Length: 9\r\n\r\nab", 100, false).is_err());
        assert!(parse_response(b"garbage\r\n\r\n", 100, false).is_err());
        assert!(parse_response(b"HTTP/1.1 200 OK\r\n", 100, false).is_err());
        let r = parse_response(b"HTTP/1.1 204 No Content\r\n\r\n", 100, false).unwrap();
        assert!(r.body.is_empty());
    }

    #[test]
    fn request_head_rejects_injection() {
        let url = Url::parse("https://example.com:8443/api/states?x=1").unwrap();
        let head = request_head(
            &HttpRequest::post("https://example.com:8443/api/states?x=1", b"{}".to_vec())
                .header("Authorization", "Bearer t"),
            &url,
        )
        .unwrap();
        assert!(head.starts_with("POST /api/states?x=1 HTTP/1.1\r\nHost: example.com:8443\r\n"));
        assert!(head.contains("Authorization: Bearer t\r\n"));
        assert!(head.ends_with("Content-Length: 2\r\n\r\n"));
        let evil = HttpRequest::get("https://example.com/").header("X", "a\r\nInjected: 1");
        assert!(request_head(&evil, &url).is_err());
        let evil = HttpRequest::get("https://example.com/").header("X: y", "z");
        assert!(request_head(&evil, &url).is_err());
    }

    /// Full HTTPS round trip against a local rustls server with a self-signed certificate
    /// for `localhost` (no network beyond 127.0.0.1).
    #[test]
    fn https_round_trip_with_a_local_server() {
        let cert = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
        let cert_der = cert.cert.der().clone();
        let key = rustls::pki_types::PrivateKeyDer::Pkcs8(
            rustls::pki_types::PrivatePkcs8KeyDer::from(cert.key_pair.serialize_der()),
        );
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let server_cfg = Arc::new(
            rustls::ServerConfig::builder_with_provider(provider)
                .with_safe_default_protocol_versions()
                .unwrap()
                .with_no_client_auth()
                .with_single_cert(vec![cert_der.clone()], key)
                .unwrap(),
        );
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (tcp, _) = listener.accept().unwrap();
            let conn = rustls::ServerConnection::new(server_cfg).unwrap();
            let mut tls = rustls::StreamOwned::new(conn, tcp);
            let mut req = Vec::new();
            let mut buf = [0u8; 1024];
            while !req.windows(4).any(|w| w == b"\r\n\r\n") {
                let n = tls.read(&mut buf).unwrap();
                req.extend_from_slice(&buf[..n]);
            }
            tls.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello")
                .unwrap();
            tls.conn.send_close_notify();
            let _ = tls.flush();
            String::from_utf8(req).unwrap()
        });
        let mut roots = rustls::RootCertStore::empty();
        roots.add(cert_der).unwrap();
        let t = TlsHttpTransport::with_roots(roots);
        let resp = t
            .send(
                &HttpRequest::get(format!("https://localhost:{port}/api/"))
                    .header("Authorization", "Bearer abc"),
            )
            .unwrap();
        assert_eq!((resp.status, resp.body.as_slice()), (200, &b"hello"[..]));
        let seen = server.join().unwrap();
        assert!(seen.starts_with("GET /api/ HTTP/1.1\r\n"), "{seen}");
        assert!(seen.contains("Authorization: Bearer abc\r\n"));

        // The public-roots transport does not trust the self-signed certificate.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let cfg = {
            let provider = Arc::new(rustls::crypto::ring::default_provider());
            let cert = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
            let key = rustls::pki_types::PrivateKeyDer::Pkcs8(
                rustls::pki_types::PrivatePkcs8KeyDer::from(cert.key_pair.serialize_der()),
            );
            Arc::new(
                rustls::ServerConfig::builder_with_provider(provider)
                    .with_safe_default_protocol_versions()
                    .unwrap()
                    .with_no_client_auth()
                    .with_single_cert(vec![cert.cert.der().clone()], key)
                    .unwrap(),
            )
        };
        let server = std::thread::spawn(move || {
            let (tcp, _) = listener.accept().unwrap();
            let conn = rustls::ServerConnection::new(cfg).unwrap();
            let mut tls = rustls::StreamOwned::new(conn, tcp);
            let mut buf = [0u8; 64];
            let _ = tls.read(&mut buf);
        });
        let err = TlsHttpTransport::new()
            .send(&HttpRequest::get(format!("https://localhost:{port}/")))
            .unwrap_err();
        assert!(matches!(err, HttpError::Connect(_)), "{err:?}");
        let _ = server.join();
        // Plain http:// still goes through the std client (refused port -> Connect error).
        assert!(matches!(
            TlsHttpTransport::new().send(&HttpRequest::get("http://127.0.0.1:1/")),
            Err(HttpError::Connect(_) | HttpError::Timeout)
        ));
    }
}
