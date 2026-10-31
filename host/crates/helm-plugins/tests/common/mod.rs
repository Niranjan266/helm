//! Local fake HTTP server for tests (127.0.0.1, ephemeral port).
#![allow(dead_code)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

#[derive(Debug, Clone)]
pub struct Recorded {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Recorded {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

pub enum Reply {
    /// Status, extra headers, body (Content-Length added).
    Normal(u16, Vec<(String, String)>, Vec<u8>),
    /// Bytes written verbatim.
    Raw(Vec<u8>),
}

pub fn json(status: u16, body: &str) -> Reply {
    Reply::Normal(
        status,
        vec![("Content-Type".into(), "application/json".into())],
        body.as_bytes().to_vec(),
    )
}

type Handler = dyn Fn(&Recorded) -> Reply + Send + Sync;

pub struct FakeServer {
    pub addr: SocketAddr,
    requests: Arc<Mutex<Vec<Recorded>>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl FakeServer {
    pub fn start(handler: impl Fn(&Recorded) -> Reply + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let handler: Arc<Handler> = Arc::new(handler);
        let thread = {
            let requests = requests.clone();
            let stop = stop.clone();
            std::thread::spawn(move || {
                for conn in listener.incoming() {
                    if stop.load(Ordering::SeqCst) {
                        break;
                    }
                    let Ok(conn) = conn else { continue };
                    serve(conn, &*handler, &requests);
                }
            })
        };
        Self {
            addr,
            requests,
            stop,
            thread: Some(thread),
        }
    }

    pub fn url(&self, path: &str) -> String {
        format!("http://{}{}", self.addr, path)
    }

    pub fn base(&self) -> String {
        format!("http://{}", self.addr)
    }

    pub fn requests(&self) -> Vec<Recorded> {
        self.requests.lock().unwrap().clone()
    }
}

impl Drop for FakeServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(self.addr);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn serve(conn: TcpStream, handler: &Handler, log: &Mutex<Vec<Recorded>>) -> Option<()> {
    let mut r = BufReader::new(conn.try_clone().ok()?);
    let mut line = String::new();
    r.read_line(&mut line).ok()?;
    let mut parts = line.split_whitespace();
    let method = parts.next()?.to_string();
    let path = parts.next()?.to_string();
    let mut headers = Vec::new();
    loop {
        let mut h = String::new();
        r.read_line(&mut h).ok()?;
        let h = h.trim_end();
        if h.is_empty() {
            break;
        }
        let (k, v) = h.split_once(':')?;
        headers.push((k.trim().to_string(), v.trim().to_string()));
    }
    let len: usize = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.parse().ok())
        .unwrap_or(0);
    let mut body = vec![0; len];
    r.read_exact(&mut body).ok()?;
    let rec = Recorded {
        method,
        path,
        headers,
        body,
    };
    let reply = handler(&rec);
    log.lock().unwrap().push(rec);
    let bytes = match reply {
        Reply::Raw(b) => b,
        Reply::Normal(status, hdrs, body) => {
            let mut out = format!("HTTP/1.1 {status} X\r\nContent-Length: {}\r\n", body.len());
            for (k, v) in hdrs {
                out.push_str(&format!("{k}: {v}\r\n"));
            }
            out.push_str("Connection: close\r\n\r\n");
            let mut out = out.into_bytes();
            out.extend_from_slice(&body);
            out
        }
    };
    let mut w = conn;
    let _ = w.write_all(&bytes);
    let _ = w.flush();
    Some(())
}
