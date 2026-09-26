//! `lb debug capture <seconds>`: writes every captured user message as one JSON line to
//! `logs/capture-<map>-<unix>.jsonl`. The files are the source of the decoder golden fixtures in lb-game.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use lb_core::msg::{MsgArg, UserMsg};
use lb_core::time::SimTime;

pub struct MessageCapture {
    pub path: PathBuf,
    pub until: SimTime,
    pub written: u64,
    out: BufWriter<File>,
}

impl MessageCapture {
    pub fn start(dir: &Path, map: &str, until: SimTime) -> std::io::Result<MessageCapture> {
        std::fs::create_dir_all(dir)?;
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let path = dir.join(format!("capture-{map}-{stamp}.jsonl"));
        let out = BufWriter::new(File::create(&path)?);
        Ok(MessageCapture {
            path,
            until,
            written: 0,
            out,
        })
    }

    pub fn write(&mut self, name: &[u8], msg: &UserMsg) {
        let args: Vec<serde_json::Value> = msg.args.iter().map(arg_json).collect();
        let line = serde_json::json!({
            "name": String::from_utf8_lossy(name),
            "dest": msg.dest,
            "target": msg.target_slot,
            "origin": msg.origin.map(|o| [o.x, o.y, o.z]),
            "args": args,
        });
        if writeln!(self.out, "{line}").is_ok() {
            self.written += 1;
        }
    }

    pub fn finish(mut self) -> (PathBuf, u64) {
        let _ = self.out.flush();
        (self.path, self.written)
    }
}

/// `[tag, value]`: B byte, C char, S short, L long, A angle, O coord, T string, E entity.
fn arg_json(arg: &MsgArg) -> serde_json::Value {
    match arg {
        MsgArg::Byte(v) => serde_json::json!(["B", v]),
        MsgArg::Char(v) => serde_json::json!(["C", v]),
        MsgArg::Short(v) => serde_json::json!(["S", v]),
        MsgArg::Long(v) => serde_json::json!(["L", v]),
        MsgArg::Angle(v) => serde_json::json!(["A", v]),
        MsgArg::Coord(v) => serde_json::json!(["O", v]),
        MsgArg::String(v) => serde_json::json!(["T", String::from_utf8_lossy(v)]),
        MsgArg::Entity(v) => serde_json::json!(["E", v]),
    }
}
