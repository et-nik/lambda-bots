//! `lb` commands to the server next to the editor, over its telemetry command channel (UDP on loopback, telemetry
//! port + 1, signed with the channel's secret: `lb-telemetry::commands`).

use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use lb_config::main_config::MainConfig;
use lb_telemetry::commands::{encode_hex, sign};
use serde::Serialize;

pub struct Commands {
    /// Where the server listens, or why commands cannot be sent.
    to: Result<SocketAddr, String>,
    secret: Vec<u8>,
    session: u64,
    nonce: AtomicU64,
}

/// What the page shows about sending commands.
#[derive(Clone, Debug, Serialize)]
pub struct Status {
    pub available: bool,
    pub detail: String,
}

impl Commands {
    /// The channel as `<install>/config/lambdabots.yaml` sets it up, with `secret` and `port` (the telemetry port)
    /// from the command line winning: cvars set in `server.cfg` do not reach the file.
    pub fn new(install: &Path, secret: Option<String>, port: Option<u16>) -> Commands {
        let path = install.join("config/lambdabots.yaml");
        let config = std::fs::read_to_string(&path)
            .ok()
            .map(|text| MainConfig::parse(&text, &path.display().to_string()));
        let telemetry = match &config {
            Some(Ok(c)) => Some(c.telemetry.clone()),
            _ => None,
        };
        let port = port.or(telemetry.as_ref().map(|t| t.port)).unwrap_or(27070);
        let unauthenticated = telemetry.as_ref().is_some_and(|t| t.allow_unauthenticated_loopback);
        let secret = secret
            .filter(|s| !s.is_empty())
            .or(telemetry.as_ref().map(|t| t.secret.clone()).filter(|s| !s.is_empty()));
        let to = match (&secret, &config) {
            (Some(_), _) => Ok(SocketAddr::from((Ipv4Addr::LOCALHOST, port.wrapping_add(1)))),
            (None, _) if unauthenticated => Ok(SocketAddr::from((Ipv4Addr::LOCALHOST, port.wrapping_add(1)))),
            (None, Some(Err(e))) => Err(format!("{e}: pass the channel's secret with --secret")),
            (None, _) => Err(format!(
                "no command channel secret: set telemetry.secret (and telemetry.enabled) in {}, or pass --secret",
                path.display()
            )),
        };
        let mut session = [0u8; 8];
        getrandom::fill(&mut session).expect("the OS gives random bytes");
        Commands {
            to,
            secret: secret.unwrap_or_default().into_bytes(),
            session: u64::from_le_bytes(session) >> 1,
            nonce: AtomicU64::new(0),
        }
    }

    pub fn status(&self) -> Status {
        match &self.to {
            Ok(addr) => Status {
                available: true,
                detail: format!("to the server's command channel at {addr}"),
            },
            Err(e) => Status {
                available: false,
                detail: e.clone(),
            },
        }
    }

    /// Sends `lb <args>`. The server answers into its telemetry, not here: whether it ran shows in its console.
    pub fn send(&self, args: &str) -> Result<SocketAddr, String> {
        let to = self.to.clone()?;
        let nonce = self.nonce.fetch_add(1, Ordering::Relaxed) + 1;
        let ts_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as u64);
        let mac = if self.secret.is_empty() {
            String::new()
        } else {
            encode_hex(&sign(&self.secret, self.session, nonce, ts_ms, args))
        };
        let message = serde_json::json!({
            "v": 2,
            "cmd": "lb",
            "args": args,
            "sid": self.session,
            "nonce": nonce,
            "ts_ms": ts_ms,
            "mac": mac,
        });
        let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).map_err(|e| e.to_string())?;
        socket
            .send_to(message.to_string().as_bytes(), to)
            .map_err(|e| format!("{to}: {e}"))?;
        Ok(to)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lb_telemetry::commands::CommandChannel;

    #[test]
    fn a_command_is_signed_as_the_server_checks_it() {
        let install = std::env::temp_dir().join(format!("lb-editor-cmd-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&install);
        let unset = Commands::new(&install, None, None);
        assert!(!unset.status().available && unset.status().detail.contains("--secret"));
        let mut server = CommandChannel::open("127.0.0.1", 0, "s3cret", false).expect("a channel");
        let port = server.local_port().expect("bound");
        let editor = Commands::new(&install, Some("s3cret".into()), Some(port - 1));
        assert!(editor.status().available);
        editor.send("overlay reload").expect("sent");
        editor.send("status").expect("sent");
        let mut got = Vec::new();
        for _ in 0..50 {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64;
            server.poll(now, &mut got);
            if got.len() >= 2 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let args: Vec<String> = got
            .into_iter()
            .map(|r| r.map(|c| c.args).map_err(|e| e.1.to_string()).unwrap())
            .collect();
        assert_eq!(args, ["overlay reload", "status"]);
    }
}
