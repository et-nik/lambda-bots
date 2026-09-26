//! Golden fixtures: user messages captured on a real server (`lb debug capture`) must decode the same way
//! as recorded in the snapshots. A decoder change that alters any result shows up as a snapshot diff.

use std::path::Path;

use lb_core::msg::{MsgArg, UserMsg};
use lb_game::messages::decode;
use smallvec::SmallVec;

fn arg(v: &serde_json::Value) -> MsgArg {
    let tag = v[0].as_str().expect("tag");
    let int = || v[1].as_i64().expect("int") as i32;
    let float = || v[1].as_f64().expect("float") as f32;
    match tag {
        "B" => MsgArg::Byte(int()),
        "C" => MsgArg::Char(int()),
        "S" => MsgArg::Short(int()),
        "L" => MsgArg::Long(int()),
        "A" => MsgArg::Angle(float()),
        "O" => MsgArg::Coord(float()),
        "T" => MsgArg::String(v[1].as_str().expect("string").as_bytes().to_vec()),
        "E" => MsgArg::Entity(int()),
        other => panic!("unknown tag {other}"),
    }
}

fn decode_file(path: &Path) -> String {
    let text = std::fs::read_to_string(path).expect("fixture");
    let mut out = String::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let v: serde_json::Value = serde_json::from_str(line).expect("json line");
        let name = v["name"].as_str().expect("name");
        let args: SmallVec<[MsgArg; 8]> = v["args"].as_array().expect("args").iter().map(arg).collect();
        let msg = UserMsg {
            msg_id: 0,
            dest: v["dest"].as_u64().unwrap_or(0) as u8,
            target_slot: v["target"].as_u64().unwrap_or(0) as u8,
            origin: None,
            truncated: false,
            from_msg_manager: false,
            args,
        };
        out.push_str(&format!(
            "{name} {line_args} => {:?}\n",
            decode(name.as_bytes(), &msg),
            line_args = v["args"]
        ));
    }
    out
}

#[test]
fn hldm_messages_decode_as_recorded() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .expect("fixtures dir")
        .flatten()
        .map(|e| e.path())
        .collect();
    files.retain(|p| p.extension().is_some_and(|e| e == "jsonl"));
    files.sort();
    assert!(!files.is_empty(), "no fixtures in {}", dir.display());
    for f in files {
        let name = f.file_stem().unwrap().to_string_lossy().to_string();
        insta::assert_snapshot!(name, decode_file(&f));
    }
}
