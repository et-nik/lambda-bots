//! Non-blocking UDP sender with per-type and global rate limits. Losing packets is fine.

use std::net::{SocketAddr, UdpSocket};

use serde::Serialize;

pub struct TelemetrySink {
    socket: Option<UdpSocket>,
    dest: Option<SocketAddr>,
    session: u64,
    seq: u64,
    epoch: u32,
    budget_bytes: f64,
    max_bytes_per_sec: f64,
    pub dropped: u64,
    pub sent: u64,
}

#[derive(Serialize)]
struct Envelope<'a, T: Serialize> {
    v: u32,
    t: &'a str,
    sid: u64,
    seq: u64,
    ep: u32,
    ts: f64,
    #[serde(flatten)]
    body: &'a T,
}

impl TelemetrySink {
    pub fn disabled() -> TelemetrySink {
        TelemetrySink {
            socket: None,
            dest: None,
            session: 0,
            seq: 0,
            epoch: 0,
            budget_bytes: 0.0,
            max_bytes_per_sec: 0.0,
            dropped: 0,
            sent: 0,
        }
    }

    pub fn open(dest: &str, port: u16, session: u64, max_kbps: u32) -> std::io::Result<TelemetrySink> {
        let socket = UdpSocket::bind("0.0.0.0:0")?;
        socket.set_nonblocking(true)?;
        let dest: SocketAddr = format!("{dest}:{port}")
            .parse()
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("{e}")))?;
        let max = max_kbps as f64 * 1024.0 / 8.0;
        Ok(TelemetrySink {
            socket: Some(socket),
            dest: Some(dest),
            session,
            seq: 0,
            epoch: 0,
            budget_bytes: max,
            max_bytes_per_sec: max,
            dropped: 0,
            sent: 0,
        })
    }

    pub fn is_enabled(&self) -> bool {
        self.socket.is_some()
    }

    pub fn set_epoch(&mut self, epoch: u32) {
        self.epoch = epoch;
    }

    pub fn begin_frame(&mut self, dt: f64) {
        self.budget_bytes = (self.budget_bytes + self.max_bytes_per_sec * dt).min(self.max_bytes_per_sec);
    }

    /// Serializes and sends one message; messages that exceed the byte budget are dropped.
    pub fn send<T: Serialize>(&mut self, kind: &str, sim_time: f64, body: &T) {
        let (Some(socket), Some(dest)) = (&self.socket, self.dest) else {
            return;
        };
        self.seq += 1;
        let env = Envelope {
            v: crate::PROTOCOL_VERSION,
            t: kind,
            sid: self.session,
            seq: self.seq,
            ep: self.epoch,
            ts: sim_time,
            body,
        };
        let Ok(bytes) = serde_json::to_vec(&env) else { return };
        if bytes.len() as f64 > self.budget_bytes || bytes.len() > 60_000 {
            self.dropped += 1;
            return;
        }
        self.budget_bytes -= bytes.len() as f64;
        match socket.send_to(&bytes, dest) {
            Ok(_) => self.sent += 1,
            Err(_) => self.dropped += 1,
        }
    }
}
