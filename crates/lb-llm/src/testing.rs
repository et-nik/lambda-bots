//! A loopback HTTP server for tests: answers requests one at a time with canned answers and keeps what it was asked.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct Canned {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: String,
    pub chunked: bool,
    /// How long to wait before answering.
    pub delay: Duration,
}

impl Canned {
    pub fn json(status: u16, body: &str) -> Canned {
        Canned {
            status,
            headers: vec![("content-type".into(), "application/json".into())],
            body: body.into(),
            chunked: false,
            delay: Duration::ZERO,
        }
    }

    pub fn header(mut self, name: &str, value: &str) -> Canned {
        self.headers.push((name.into(), value.into()));
        self
    }

    pub fn chunked(mut self) -> Canned {
        self.chunked = true;
        self
    }

    pub fn delay(mut self, delay: Duration) -> Canned {
        self.delay = delay;
        self
    }
}

#[derive(Clone, Debug)]
pub struct Recorded {
    pub method: String,
    pub path: String,
    /// Names in lower case.
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl Recorded {
    pub fn header(&self, name: &str) -> Option<&str> {
        let name = name.to_ascii_lowercase();
        self.headers.iter().find(|(n, _)| *n == name).map(|(_, v)| v.as_str())
    }
}

pub struct MockServer {
    addr: SocketAddr,
    requests: Arc<Mutex<Vec<Recorded>>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl MockServer {
    /// Answers the n-th request with `answers[n]`, and every request past the list with its last answer.
    pub fn start(answers: Vec<Canned>) -> MockServer {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind a loopback port");
        let addr = listener.local_addr().expect("local address");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let (requests, stop) = (requests.clone(), stop.clone());
            std::thread::Builder::new()
                .name("lb-llm-mock".into())
                .spawn(move || {
                    for (n, stream) in listener.incoming().enumerate() {
                        if stop.load(Ordering::SeqCst) {
                            break;
                        }
                        let (Ok(stream), Some(answer)) = (stream, answers.get(n).or(answers.last())) else {
                            continue;
                        };
                        serve(stream, answer, &requests);
                    }
                })
                .expect("spawn the mock server")
        };
        MockServer {
            addr,
            requests,
            stop,
            thread: Some(thread),
        }
    }

    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    pub fn requests(&self) -> Vec<Recorded> {
        self.requests.lock().map(|r| r.clone()).unwrap_or_default()
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(self.addr);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn serve(stream: TcpStream, answer: &Canned, requests: &Mutex<Vec<Recorded>>) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut reader = BufReader::new(stream);
    let Some(request) = read_request(&mut reader) else {
        return;
    };
    if let Ok(mut r) = requests.lock() {
        r.push(request);
    }
    std::thread::sleep(answer.delay);
    let mut out = format!("HTTP/1.1 {} Canned\r\nconnection: close\r\n", answer.status).into_bytes();
    for (name, value) in &answer.headers {
        out.extend_from_slice(format!("{name}: {value}\r\n").as_bytes());
    }
    let body = answer.body.as_bytes();
    if answer.chunked {
        out.extend_from_slice(b"transfer-encoding: chunked\r\n\r\n");
        for chunk in body.chunks(body.len().div_ceil(3).max(1)) {
            out.extend_from_slice(format!("{:x}\r\n", chunk.len()).as_bytes());
            out.extend_from_slice(chunk);
            out.extend_from_slice(b"\r\n");
        }
        out.extend_from_slice(b"0\r\n\r\n");
    } else {
        out.extend_from_slice(format!("content-length: {}\r\n\r\n", body.len()).as_bytes());
        out.extend_from_slice(body);
    }
    let _ = reader.get_mut().write_all(&out);
}

fn read_request(reader: &mut BufReader<TcpStream>) -> Option<Recorded> {
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;
    let mut parts = line.split_whitespace();
    let (method, path) = (parts.next()?.to_string(), parts.next()?.to_string());
    let mut headers = Vec::new();
    loop {
        line.clear();
        reader.read_line(&mut line).ok()?;
        let header = line.trim_end();
        if header.is_empty() {
            break;
        }
        let (name, value) = header.split_once(':')?;
        headers.push((name.trim().to_ascii_lowercase(), value.trim().to_string()));
    }
    let length = headers
        .iter()
        .find(|(n, _)| n == "content-length")
        .and_then(|(_, v)| v.parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = vec![0; length];
    reader.read_exact(&mut body).ok()?;
    Some(Recorded {
        method,
        path,
        headers,
        body: String::from_utf8_lossy(&body).into_owned(),
    })
}
