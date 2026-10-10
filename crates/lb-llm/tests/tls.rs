//! HTTPS end to end on loopback: rustls with ring on both sides, the server's certificate trusted only through
//! `ca_file` (`fixtures/` holds a test CA and a `localhost` certificate valid until 2126).

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use lb_llm::{Client, ErrorClass, Kind, Prompt, Settings};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

/// Serves one HTTPS request with a chat completion; returns the port.
fn serve_once() -> u16 {
    let certs = CertificateDer::pem_file_iter(fixture("localhost.pem"))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    let key = PrivateKeyDer::from_pem_file(fixture("localhost.key")).unwrap();
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        let Ok((tcp, _)) = listener.accept() else {
            return;
        };
        let conn = rustls::ServerConnection::new(Arc::new(config)).unwrap();
        let mut reader = BufReader::new(rustls::StreamOwned::new(conn, tcp));
        let mut length = 0;
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).unwrap_or(0) == 0 {
                return;
            }
            if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                length = v.trim().parse().unwrap();
            }
            if line.trim_end().is_empty() {
                break;
            }
        }
        let mut body = vec![0; length];
        reader.read_exact(&mut body).unwrap();
        let answer = r#"{"content":[{"type":"text","text":"gg"}],"stop_reason":"end_turn","usage":{"input_tokens":3,"output_tokens":1}}"#;
        let stream = reader.get_mut();
        let _ = write!(
            stream,
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\nconnection: close\r\ncontent-length: {}\r\n\r\n{answer}",
            answer.len()
        );
        stream.conn.send_close_notify();
        let _ = stream.flush();
    });
    port
}

fn settings(port: u16, ca_file: Option<PathBuf>) -> Settings {
    Settings {
        kind: Kind::Anthropic,
        base_url: format!("https://localhost:{port}"),
        model: "test".into(),
        key: Some("k".into()),
        headers: Vec::new(),
        max_tokens: 16,
        temperature: None,
        extra_body: None,
        timeout: Duration::from_secs(10),
        ca_file,
    }
}

#[test]
fn https_with_a_trusted_bundle() {
    let port = serve_once();
    let client = Client::new(settings(port, Some(fixture("ca.pem")))).unwrap();
    let done = client
        .complete(&Prompt {
            user: "hi".into(),
            ..Prompt::default()
        })
        .unwrap();
    assert_eq!(done.text, "gg");
}

#[test]
fn https_without_the_bundle_is_refused() {
    let port = serve_once();
    let client = Client::new(settings(port, None)).unwrap();
    let e = client.complete(&Prompt::default()).unwrap_err();
    assert_eq!(e.class(), ErrorClass::Backoff(None), "{e}");
}

#[test]
fn a_bundle_without_certificates_is_a_config_error() {
    let e = Client::new(settings(1, Some(fixture("localhost.key")))).err().unwrap();
    assert_eq!(e.class(), ErrorClass::Fatal, "{e}");
}
