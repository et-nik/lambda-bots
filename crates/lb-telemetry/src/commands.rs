//! Authenticated command channel (UDP, loopback by default).
//!
//! Message: `{"v":2,"cmd":"lb","args":"add","sid":1,"nonce":5,"ts_ms":1700000000000,"mac":"<hex>"}`
//! with `mac = HMAC-SHA256(secret, "lbcmd1\n" + sid + "\n" + nonce + "\n" + ts_ms + "\n" + args)`.
//! Nonces must strictly increase per session and timestamps must be within ±30 s. Without a
//! secret the channel accepts nothing unless unauthenticated loopback use was explicitly allowed.

use std::net::{SocketAddr, UdpSocket};

use hmac::{Hmac, KeyInit, Mac};
use serde::Deserialize;
use sha2::Sha256;

#[derive(Clone, Debug, Deserialize)]
struct Wire {
    v: u32,
    cmd: String,
    args: String,
    #[serde(default)]
    sid: u64,
    #[serde(default)]
    nonce: u64,
    #[serde(default)]
    ts_ms: u64,
    #[serde(default)]
    mac: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandRequest {
    pub args: String,
    pub from: SocketAddr,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum VerifyError {
    #[error("malformed message")]
    Malformed,
    #[error("unsupported protocol version")]
    Version,
    #[error("only `lb` commands are accepted")]
    NotLb,
    #[error("command channel disabled: no secret configured")]
    Disabled,
    #[error("bad signature")]
    Signature,
    #[error("replayed or out-of-order nonce")]
    Replay,
    #[error("timestamp outside the allowed window")]
    Clock,
}

pub struct CommandChannel {
    socket: Option<UdpSocket>,
    secret: Vec<u8>,
    allow_unauth_loopback: bool,
    last_nonce: Vec<(u64, u64)>,
    pub rejected: u64,
}

pub const MAX_CLOCK_SKEW_MS: u64 = 30_000;

impl CommandChannel {
    pub fn disabled() -> CommandChannel {
        CommandChannel {
            socket: None,
            secret: Vec::new(),
            allow_unauth_loopback: false,
            last_nonce: Vec::new(),
            rejected: 0,
        }
    }

    pub fn open(bind: &str, port: u16, secret: &str, allow_unauth_loopback: bool) -> std::io::Result<CommandChannel> {
        let socket = UdpSocket::bind(format!("{bind}:{port}"))?;
        socket.set_nonblocking(true)?;
        Ok(CommandChannel {
            socket: Some(socket),
            secret: secret.as_bytes().to_vec(),
            allow_unauth_loopback,
            last_nonce: Vec::new(),
            rejected: 0,
        })
    }

    /// The port it listens on (the one the OS picked when opened on port 0).
    pub fn local_port(&self) -> Option<u16> {
        Some(self.socket.as_ref()?.local_addr().ok()?.port())
    }

    /// Drains pending datagrams; returns verified commands and verification failures.
    pub fn poll(&mut self, now_ms: u64, out: &mut Vec<Result<CommandRequest, (SocketAddr, VerifyError)>>) {
        let Some(socket) = self.socket.as_ref() else { return };
        let mut buf = [0u8; 2048];
        let mut received = Vec::new();
        for _ in 0..16 {
            match socket.recv_from(&mut buf) {
                Ok((n, from)) => received.push((buf[..n].to_vec(), from)),
                Err(_) => break,
            }
        }
        for (bytes, from) in received {
            match self.verify(&bytes, from, now_ms) {
                Ok(args) => out.push(Ok(CommandRequest { args, from })),
                Err(e) => {
                    self.rejected += 1;
                    out.push(Err((from, e)));
                }
            }
        }
    }

    pub fn verify(&mut self, bytes: &[u8], from: SocketAddr, now_ms: u64) -> Result<String, VerifyError> {
        let wire: Wire = serde_json::from_slice(bytes).map_err(|_| VerifyError::Malformed)?;
        if wire.v != crate::PROTOCOL_VERSION {
            return Err(VerifyError::Version);
        }
        if wire.cmd != "lb" {
            return Err(VerifyError::NotLb);
        }
        if self.secret.is_empty() {
            return if self.allow_unauth_loopback && from.ip().is_loopback() {
                Ok(wire.args)
            } else {
                Err(VerifyError::Disabled)
            };
        }
        let expected = sign(&self.secret, wire.sid, wire.nonce, wire.ts_ms, &wire.args);
        let got = decode_hex(&wire.mac).ok_or(VerifyError::Signature)?;
        if !constant_time_eq(&expected, &got) {
            return Err(VerifyError::Signature);
        }
        if now_ms.abs_diff(wire.ts_ms) > MAX_CLOCK_SKEW_MS {
            return Err(VerifyError::Clock);
        }
        match self.last_nonce.iter_mut().find(|(sid, _)| *sid == wire.sid) {
            Some((_, last)) if wire.nonce <= *last => return Err(VerifyError::Replay),
            Some((_, last)) => *last = wire.nonce,
            None => self.last_nonce.push((wire.sid, wire.nonce)),
        }
        Ok(wire.args)
    }
}

pub fn sign(secret: &[u8], sid: u64, nonce: u64, ts_ms: u64, args: &str) -> Vec<u8> {
    let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(secret).expect("HMAC accepts any key length");
    mac.update(format!("lbcmd1\n{sid}\n{nonce}\n{ts_ms}\n{args}").as_bytes());
    mac.finalize().into_bytes().to_vec()
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    use subtle::ConstantTimeEq;
    a.len() == b.len() && bool::from(a.ct_eq(b))
}

pub fn encode_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn decode_hex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wire(secret: &[u8], sid: u64, nonce: u64, ts: u64, args: &str) -> Vec<u8> {
        let mac = encode_hex(&sign(secret, sid, nonce, ts, args));
        format!(r#"{{"v":2,"cmd":"lb","args":"{args}","sid":{sid},"nonce":{nonce},"ts_ms":{ts},"mac":"{mac}"}}"#)
            .into_bytes()
    }

    fn channel(secret: &str, unauth: bool) -> CommandChannel {
        CommandChannel {
            socket: None,
            secret: secret.as_bytes().to_vec(),
            allow_unauth_loopback: unauth,
            last_nonce: Vec::new(),
            rejected: 0,
        }
    }

    const LOCAL: &str = "127.0.0.1:5000";

    #[test]
    fn accepts_signed_and_rejects_replay_and_forgery() {
        let mut c = channel("s3cret", false);
        let from: SocketAddr = LOCAL.parse().unwrap();
        assert_eq!(
            c.verify(&wire(b"s3cret", 1, 1, 1000, "add"), from, 1000),
            Ok("add".into())
        );
        assert_eq!(
            c.verify(&wire(b"s3cret", 1, 1, 1000, "add"), from, 1000),
            Err(VerifyError::Replay)
        );
        assert_eq!(
            c.verify(&wire(b"wrong", 1, 2, 1000, "add"), from, 1000),
            Err(VerifyError::Signature)
        );
        assert_eq!(
            c.verify(&wire(b"s3cret", 1, 3, 1000, "add"), from, 1000 + 31_000),
            Err(VerifyError::Clock)
        );
    }

    #[test]
    fn unsigned_rejected_without_secret() {
        let unsigned = br#"{"v":2,"cmd":"lb","args":"add"}"#;
        let from: SocketAddr = LOCAL.parse().unwrap();
        assert_eq!(channel("", false).verify(unsigned, from, 0), Err(VerifyError::Disabled));
        assert_eq!(channel("", true).verify(unsigned, from, 0), Ok("add".into()));
        let remote: SocketAddr = "10.0.0.5:5000".parse().unwrap();
        assert_eq!(
            channel("", true).verify(unsigned, remote, 0),
            Err(VerifyError::Disabled)
        );
        assert_eq!(
            channel("k", false).verify(unsigned, from, 0),
            Err(VerifyError::Signature)
        );
    }
}
