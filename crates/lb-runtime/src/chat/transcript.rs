//! `logs/chat.<date>.log` (UTC days, a week kept): every request to the chat model and its answer as they went over
//! the wire, the JSON bodies written as YAML so the prompts read as text, and what came of each.

use std::fmt::Write as _;
use std::fs::{File, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};

use lb_llm::Exchange;

/// Days of transcripts kept.
const KEEP: usize = 7;
/// The transcript's day files are `chat.<date>.log`.
const PREFIX: &str = "chat";

pub struct Transcript {
    dir: PathBuf,
    day: String,
    file: Option<File>,
}

/// Today (UTC) as `2026-10-05`, and the time as `21:14:03`.
pub fn now() -> (String, String) {
    let s = crate::roster::stamp();
    (
        format!("{}-{}-{}", &s[0..4], &s[4..6], &s[6..8]),
        format!("{}:{}:{}", &s[9..11], &s[11..13], &s[13..15]),
    )
}

/// A JSON body as YAML, multi-line strings as blocks; anything else as it came.
fn as_yaml(body: &str) -> String {
    match serde_json::from_str::<serde_json::Value>(body) {
        Ok(v) => lb_config::yaml::to_string(&v).unwrap_or_else(|_| format!("{body}\n")),
        Err(_) => format!("{body}\n"),
    }
}

/// The file of `day` in `dir`, `<prefix>.<day>.log`.
pub fn file_of(dir: &Path, prefix: &str, day: &str) -> PathBuf {
    dir.join(format!("{prefix}.{day}.log"))
}

/// The day of a day's file `<prefix>.YYYY-MM-DD.log`; `None` for any other name.
pub fn day_of<'a>(name: &'a str, prefix: &str) -> Option<&'a str> {
    let day = name.strip_prefix(prefix)?.strip_prefix('.')?.strip_suffix(".log")?;
    let date = day.len() == 10
        && day.bytes().enumerate().all(|(i, b)| match i {
            4 | 7 => b == b'-',
            _ => b.is_ascii_digit(),
        });
    date.then_some(day)
}

/// Today's file in `dir`.
pub fn today(dir: &Path) -> PathBuf {
    file_of(dir, PREFIX, &now().0)
}

impl Transcript {
    pub fn new(dir: PathBuf) -> Transcript {
        Transcript {
            dir,
            day: String::new(),
            file: None,
        }
    }

    /// Opens the file of `day`, the older ones beyond [`KEEP`] removed.
    fn open(&mut self, day: &str) -> Option<&mut File> {
        if self.day != day || self.file.is_none() {
            let _ = std::fs::create_dir_all(&self.dir);
            let path = file_of(&self.dir, PREFIX, day);
            self.file = match OpenOptions::new().create(true).append(true).open(&path) {
                Ok(f) => Some(f),
                Err(e) => {
                    tracing::warn!("chat transcript {}: {e}", path.display());
                    None
                }
            };
            self.day = day.to_string();
            self.prune();
        }
        self.file.as_mut()
    }

    fn prune(&self) {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return;
        };
        let mut days: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| day_of(n, PREFIX).is_some())
            })
            .collect();
        days.sort();
        for old in days.iter().rev().skip(KEEP) {
            let _ = std::fs::remove_file(old);
        }
    }

    /// One exchange: `title` (whose line and why), the request and the answer, and what came of it.
    pub fn write(&mut self, title: &str, wire: &Exchange, outcome: &str) {
        let (day, time) = now();
        let mut text = format!("===== {day} {time} UTC · {title}\n>>> POST {}\n", wire.url);
        text.push_str(&as_yaml(&wire.request));
        let ms = wire.elapsed.as_millis();
        match (wire.status, &wire.response) {
            (Some(status), Some(body)) => {
                let _ = writeln!(text, "<<< {status} in {ms} ms");
                text.push_str(&as_yaml(body));
            }
            _ => {
                let _ = writeln!(text, "<<< no answer after {ms} ms");
            }
        }
        let _ = writeln!(text, "=== {outcome}\n");
        if let Some(file) = self.open(&day)
            && let Err(e) = file.write_all(text.as_bytes())
        {
            tracing::warn!("chat transcript: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn exchanges_read_as_text_and_old_days_go() {
        let dir = std::env::temp_dir().join(format!("lb-transcript-prune-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for day in 1..=9 {
            std::fs::write(dir.join(format!("chat.2020-01-0{day}.log")), "old").unwrap();
        }
        let other = dir.join("chat.backup.log");
        std::fs::write(&other, "kept").unwrap();
        let mut t = Transcript::new(dir.clone());
        let wire = Exchange {
            url: "https://api.example.com/v1/chat/completions".into(),
            request: r#"{"model":"m","messages":[{"role":"user","content":"строка 1\nстрока 2"}]}"#.into(),
            status: Some(200),
            response: Some(r#"{"choices":[{"message":{"content":"gg"}}]}"#.into()),
            elapsed: Duration::from_millis(840),
        };
        t.write("Kleiner · Gordon to the bot: привет", &wire, "line: gg");
        let lost = Exchange {
            status: None,
            response: None,
            ..wire.clone()
        };
        t.write("Kleiner · Gordon joined", &lost, "failed: timed out");
        let text = std::fs::read_to_string(today(&dir)).unwrap();
        assert!(
            text.contains("UTC · Kleiner · Gordon to the bot: привет\n>>> POST https://api.example.com"),
            "{text}"
        );
        assert!(text.contains("- content: |-\n    строка 1\n    строка 2"), "{text}");
        assert!(text.contains("<<< 200 in 840 ms"), "{text}");
        assert!(text.contains("=== line: gg"), "{text}");
        assert!(
            text.contains("<<< no answer after 840 ms\n=== failed: timed out"),
            "{text}"
        );
        assert!(other.exists(), "only day files are pruned");
        let files = std::fs::read_dir(&dir).unwrap().count();
        assert_eq!(files, KEEP + 1, "a week of transcripts is kept");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
