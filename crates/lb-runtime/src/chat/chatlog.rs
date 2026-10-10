//! `logs/chatlog.<date>.log` (UTC days, `chat.chatlog_days` of them kept): the game chat as the players saw it, our
//! bots' lines marked, players coming and going, each map's lines under its name. Written on the main thread and
//! never read back: one `write_all` per line on an unbuffered file, so a crash loses no line, and the map's header
//! comes first whenever the file is opened.

use std::borrow::Cow;
use std::fs::{File, OpenOptions};
use std::io::Write as _;
use std::path::PathBuf;

use lb_chat::sanitize;

use super::transcript::{day_of, file_of, now};

/// The log's day files are `chatlog.<date>.log`.
const PREFIX: &str = "chatlog";

pub struct ChatLog {
    dir: PathBuf,
    /// The UTC day of `file`.
    day: String,
    file: Option<File>,
    /// The map the lines are on, and whether its header is written.
    map: String,
    headed: bool,
    /// The UTC day opening or writing the file failed: nothing more is tried that day.
    failed: Option<String>,
    /// The UTC day the old day files last went.
    pruned: Option<String>,
}

/// A line of the log as it goes after the time: a space for a player, `»` for our bot, `+` for a player coming in,
/// `-` for one leaving, then the name.
enum Line<'a> {
    Say {
        name: &'a str,
        bot: bool,
        team: bool,
        text: &'a str,
    },
    Join(&'a str),
    Leave(&'a str),
}

impl std::fmt::Display for Line<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            Line::Say { name, bot, team, text } => {
                let text = one_line(text);
                let text = if bot { Cow::from(text.as_str()) } else { masked(&text) };
                let gutter = if bot { '»' } else { ' ' };
                let team = if team { " (team)" } else { "" };
                write!(f, "{gutter} {}{team}: {text}", name_of(name))
            }
            Line::Join(name) => write!(f, "+ {}", name_of(name)),
            Line::Leave(name) => write!(f, "- {}", name_of(name)),
        }
    }
}

impl ChatLog {
    pub fn new(dir: PathBuf) -> ChatLog {
        ChatLog {
            dir,
            day: String::new(),
            file: None,
            map: String::new(),
            headed: false,
            failed: None,
            pruned: None,
        }
    }

    /// A map starts: its header goes before its first line.
    pub fn map(&mut self, map: &str) {
        self.map = one_line(map);
        self.headed = false;
    }

    /// Removes the day files more than `keep_days` days old, at most once a UTC day, whether the log is written or
    /// not.
    pub fn prune_daily(&mut self, keep_days: u32) {
        self.prune_once(&now().0, keep_days);
    }

    /// Lets the file go and forgets a failure: the next line opens the file again, under the map's header.
    pub fn close(&mut self) {
        self.file = None;
        self.failed = None;
    }

    /// A line of the chat: `bot` for our bot's, `team` for `say_team`.
    pub fn say(&mut self, keep_days: u32, name: &str, bot: bool, team: bool, text: &str) {
        self.write(keep_days, Line::Say { name, bot, team, text });
    }

    /// A player came in.
    pub fn join(&mut self, keep_days: u32, name: &str) {
        self.write(keep_days, Line::Join(name));
    }

    /// A player left.
    pub fn leave(&mut self, keep_days: u32, name: &str) {
        self.write(keep_days, Line::Leave(name));
    }

    /// Today's file.
    pub fn today(&self) -> PathBuf {
        file_of(&self.dir, PREFIX, &now().0)
    }

    /// `lb chat status`: today's file and the days kept, or why the chat is not written.
    pub fn status(log: Option<&ChatLog>, on: bool, keep_days: u32) -> String {
        match log {
            None => "chatlog: not written in a replay".into(),
            Some(_) if !on => "chatlog: off (chat.chatlog)".into(),
            Some(log) => format!(
                "chatlog: {} ({keep_days} {} kept)",
                log.today().display(),
                if keep_days == 1 { "day" } else { "days" }
            ),
        }
    }

    fn write(&mut self, keep_days: u32, line: Line<'_>) {
        let (day, time) = now();
        self.write_at(&day, &time, keep_days, line);
    }

    /// Writes `line` at `time` of the UTC `day`. A failure is told once, and nothing is tried again before the next
    /// day or [`ChatLog::close`].
    fn write_at(&mut self, day: &str, time: &str, keep_days: u32, line: Line<'_>) {
        if self.failed.as_deref() == Some(day) {
            return;
        }
        if let Err(e) = self.append(day, time, keep_days, &line) {
            tracing::warn!(
                "chat log {}: {e}; tried again on the next UTC day or after `lb config reload`",
                file_of(&self.dir, PREFIX, day).display()
            );
            self.file = None;
            self.failed = Some(day.to_string());
        }
    }

    /// The line and, when due, the map's header before it, in one write.
    fn append(&mut self, day: &str, time: &str, keep_days: u32, line: &Line<'_>) -> std::io::Result<()> {
        let file = match self.file.take() {
            Some(file) if self.day == day => file,
            _ => self.open(day, keep_days)?,
        };
        let header = if self.headed || self.map.is_empty() {
            String::new()
        } else {
            format!("---- {} ----\n", self.map)
        };
        self.file
            .insert(file)
            .write_all(format!("{header}{time} {line}\n").as_bytes())?;
        self.headed = true;
        Ok(())
    }

    /// The file of `day`, its first line under a header; the day files more than `keep_days` days older go.
    fn open(&mut self, day: &str, keep_days: u32) -> std::io::Result<File> {
        std::fs::create_dir_all(&self.dir)?;
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(file_of(&self.dir, PREFIX, day))?;
        self.day = day.to_string();
        self.headed = false;
        self.prune(day, keep_days);
        Ok(file)
    }

    /// [`ChatLog::prune`] unless the day files already went on `today`.
    fn prune_once(&mut self, today: &str, keep_days: u32) {
        if self.pruned.as_deref() != Some(today) {
            self.prune(today, keep_days);
        }
    }

    /// Removes the day files dated more than `keep_days` days before `today`; no other file is touched.
    fn prune(&mut self, today: &str, keep_days: u32) {
        self.pruned = Some(today.to_string());
        let Some(today) = days_from_civil(today) else {
            return;
        };
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let day = name.to_str().and_then(|n| day_of(n, PREFIX)).and_then(days_from_civil);
            if day.is_some_and(|d| today - d > i64::from(keep_days)) {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
}

/// `s` on one line: control characters as spaces, the unseen ones dropped, every run of spaces, line and paragraph
/// separators among them, as one space, none at the ends.
pub fn one_line(s: &str) -> String {
    let shown: String = s
        .chars()
        .filter(|&c| !unseen(c))
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    shown.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Zero-width characters, direction marks and overrides, the byte order mark: they show nothing, or turn the text
/// around.
fn unseen(c: char) -> bool {
    matches!(c, '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2069}' | '\u{FEFF}')
}

/// A player's line with its password hidden: after a login or registration command ([`sanitize::secret`]) all
/// becomes `***` (`/login ***`).
pub fn masked(text: &str) -> Cow<'_, str> {
    match text.split_whitespace().next() {
        Some(first) if sanitize::secret(text) => Cow::Owned(format!("{first} ***")),
        _ => Cow::Borrowed(text),
    }
}

/// A name on one line; `?` for none.
fn name_of(name: &str) -> String {
    let name = one_line(name);
    if name.is_empty() { "?".into() } else { name }
}

/// Days from 1970-01-01 to a `YYYY-MM-DD` date (H. Hinnant's `days_from_civil`).
fn days_from_civil(date: &str) -> Option<i64> {
    let mut parts = date.split('-').map(|p| p.parse::<i64>().ok());
    let (year, month, day) = (parts.next()??, parts.next()??, parts.next()??);
    if parts.next().is_some() || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(era * 146_097 + doe - 719_468)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    const DAY: &str = "2026-10-10";

    /// A directory of the test's own.
    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lb-chatlog-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn read(dir: &Path, day: &str) -> String {
        std::fs::read_to_string(file_of(dir, PREFIX, day)).unwrap()
    }

    fn player<'a>(name: &'a str, text: &'a str) -> Line<'a> {
        Line::Say {
            name,
            bot: false,
            team: false,
            text,
        }
    }

    #[test]
    fn lines_read_as_the_chat_went() {
        let dir = dir("format");
        let mut log = ChatLog::new(dir.clone());
        log.map("gg_cold_rock");
        log.write_at(DAY, "21:14:03", 30, Line::Join("Gordon"));
        log.write_at(DAY, "21:14:10", 30, player("Gordon", "всем привет"));
        let bot = Line::Say {
            name: "Plutonium",
            bot: true,
            team: false,
            text: "привет, Gordon",
        };
        log.write_at(DAY, "21:14:13", 30, bot);
        let team = Line::Say {
            name: "Alyx",
            bot: false,
            team: true,
            text: "го на рельсы",
        };
        log.write_at(DAY, "21:15:02", 30, team);
        log.write_at(DAY, "21:15:40", 30, player("Gordon", "/login secret"));
        log.write_at(DAY, "21:31:55", 30, Line::Leave("Gordon"));
        assert_eq!(
            read(&dir, DAY),
            "---- gg_cold_rock ----\n\
             21:14:03 + Gordon\n\
             21:14:10   Gordon: всем привет\n\
             21:14:13 » Plutonium: привет, Gordon\n\
             21:15:02   Alyx (team): го на рельсы\n\
             21:15:40   Gordon: /login ***\n\
             21:31:55 - Gordon\n"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_clock_stamps_each_line_with_the_utc_time() {
        let dir = dir("clock");
        let mut log = ChatLog::new(dir.clone());
        log.map("crossfire");
        log.join(30, "Gordon");
        log.say(30, "Gordon", false, false, "hi");
        log.say(30, "Plutonium", true, true, "hello");
        log.leave(30, "Gordon");
        let today = log.today();
        let name = today.file_name().and_then(|n| n.to_str()).unwrap();
        assert!(day_of(name, PREFIX).is_some(), "{name}");
        // A line written past midnight goes to the next day's file: read them all.
        let mut files: Vec<PathBuf> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.path()).collect();
        files.sort();
        let text: String = files.iter().map(|f| std::fs::read_to_string(f).unwrap()).collect();
        let lines: Vec<&str> = text.lines().filter(|l| *l != "---- crossfire ----").collect();
        let ends = [" + Gordon", "   Gordon: hi", " » Plutonium (team): hello", " - Gordon"];
        assert_eq!(lines.len(), ends.len(), "{text}");
        for (line, end) in lines.iter().zip(ends) {
            let (time, rest) = line.split_at(8);
            let hms = time.bytes().enumerate().all(|(i, b)| match i {
                2 | 5 => b == b':',
                _ => b.is_ascii_digit(),
            });
            assert!(hms && rest == end, "{line}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn names_and_lines_come_out_on_one_clean_line() {
        assert_eq!(one_line(" a\tb\r\nc\u{0085}d "), "a b c d");
        assert_eq!(one_line("x\u{2028}y\u{2029}z"), "x y z");
        assert_eq!(one_line("pl\u{200B}ay\u{200D}er\u{FEFF}"), "player");
        assert_eq!(one_line("\u{202E}tset\u{202C} \u{2066}ok\u{2069} \u{200F}"), "tset ok");
        assert_eq!(one_line("\u{2060}"), "");
        assert_eq!(Line::Join("").to_string(), "+ ?");
        assert_eq!(Line::Leave(" \u{200B} ").to_string(), "- ?");
        assert_eq!(player("\u{0007}", "ok").to_string(), "  ?: ok");
    }

    #[test]
    fn a_header_leads_each_map_with_lines_and_each_file_opened() {
        let dir = dir("headers");
        let mut log = ChatLog::new(dir.clone());
        log.map("crossfire");
        log.map("gg_cold_rock");
        log.write_at(DAY, "10:00:00", 30, player("Gordon", "раз"));
        log.write_at(DAY, "10:00:05", 30, player("Gordon", "два"));
        log.close();
        log.write_at(DAY, "10:00:10", 30, player("Gordon", "три"));
        log.map("");
        log.write_at(DAY, "10:00:15", 30, player("Gordon", "четыре"));
        assert_eq!(
            read(&dir, DAY),
            "---- gg_cold_rock ----\n\
             10:00:00   Gordon: раз\n\
             10:00:05   Gordon: два\n\
             ---- gg_cold_rock ----\n\
             10:00:10   Gordon: три\n\
             10:00:15   Gordon: четыре\n"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn utc_midnight_opens_the_next_days_file_under_the_header() {
        let dir = dir("midnight");
        let mut log = ChatLog::new(dir.clone());
        log.map("stalkyard");
        log.write_at("2026-10-09", "23:59:58", 30, player("Gordon", "до полуночи"));
        log.write_at("2026-10-10", "00:00:01", 30, player("Gordon", "после"));
        assert_eq!(
            read(&dir, "2026-10-09"),
            "---- stalkyard ----\n23:59:58   Gordon: до полуночи\n"
        );
        assert_eq!(
            read(&dir, "2026-10-10"),
            "---- stalkyard ----\n00:00:01   Gordon: после\n"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn day_files_go_by_the_date_in_their_name() {
        let dir = dir("prune");
        let old = [
            "chatlog.2025-01-01.log",
            "chatlog.2026-09-09.log",
            "chatlog.2026-09-10.log",
            "chatlog.2026-10-11.log",
            "chatlog.2026-13-01.log",
            "chatlog.backup.log",
            "chat.2020-01-01.log",
            "lambdabots.2020-01-01.log",
        ];
        for name in old {
            std::fs::write(dir.join(name), "old\n").unwrap();
        }
        let mut log = ChatLog::new(dir.clone());
        log.write_at(DAY, "12:00:00", 30, player("Gordon", "hi"));
        let mut left: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter_map(|e| e.file_name().into_string().ok())
            .collect();
        left.sort();
        assert_eq!(
            left,
            [
                "chat.2020-01-01.log",
                "chatlog.2026-09-10.log",
                "chatlog.2026-10-10.log",
                "chatlog.2026-10-11.log",
                "chatlog.2026-13-01.log",
                "chatlog.backup.log",
                "lambdabots.2020-01-01.log",
            ]
        );
        assert_eq!(
            day_of("chatlog.2026-10-10.log", "chat"),
            None,
            "the transcripts' pruning leaves the chat log alone"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn old_day_files_go_once_a_day_without_a_line_written() {
        let dir = dir("daily");
        let files = |names: &[&str]| {
            for name in names {
                std::fs::write(dir.join(name), "old\n").unwrap();
            }
        };
        let left = || {
            let mut left: Vec<String> = std::fs::read_dir(&dir)
                .unwrap()
                .flatten()
                .filter_map(|e| e.file_name().into_string().ok())
                .collect();
            left.sort();
            left
        };
        files(&["chatlog.2026-09-01.log", "chatlog.2026-09-10.log", "notes.txt"]);
        let mut log = ChatLog::new(dir.clone());
        log.prune_once(DAY, 30);
        assert_eq!(left(), ["chatlog.2026-09-10.log", "notes.txt"], "and no file opened");
        files(&["chatlog.2026-09-02.log"]);
        log.prune_once(DAY, 30);
        assert_eq!(
            left(),
            ["chatlog.2026-09-02.log", "chatlog.2026-09-10.log", "notes.txt"],
            "once a day"
        );
        log.prune_once("2026-10-11", 30);
        assert_eq!(left(), ["notes.txt"]);

        files(&["chatlog.2000-01-01.log"]);
        ChatLog::new(dir.clone()).prune_daily(30);
        assert_eq!(left(), ["notes.txt"], "today by the clock");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn dates_count_in_days() {
        assert_eq!(days_from_civil("1970-01-01"), Some(0));
        assert_eq!(days_from_civil("2000-03-01"), Some(11_017));
        let between = |a, b| Some(days_from_civil(a)? - days_from_civil(b)?);
        assert_eq!(between("2024-03-01", "2024-02-28"), Some(2));
        assert_eq!(between("2026-10-10", "2026-09-10"), Some(30));
        assert_eq!(days_from_civil("2026-13-01"), None);
        assert_eq!(days_from_civil("2026-10"), None);
    }

    #[test]
    fn a_password_after_a_login_command_is_masked() {
        assert_eq!(masked("/login secret"), "/login ***");
        assert_eq!(masked("!reg a b"), "!reg ***");
        assert_eq!(masked(".PW hunter2"), ".PW ***");
        assert_eq!(masked("auth 1234"), "auth ***");
        assert_eq!(
            masked("I can't login, what is the password"),
            "I can't login, what is the password"
        );
        assert_eq!(masked("/login"), "/login", "nothing after the command to hide");
        assert_eq!(masked("//login secret"), "//login secret");
        assert_eq!(masked("pass hunter2"), "pass ***");
        assert_eq!(masked("pass the gauss"), "pass the gauss", "chat, not a password");
        assert!(matches!(masked("gg all"), Cow::Borrowed(_)));
        let tab = Line::Say {
            name: "Gordon",
            bot: false,
            team: true,
            text: "/register\tsecret secret",
        };
        assert_eq!(tab.to_string(), "  Gordon (team): /register ***");
        let bot = Line::Say {
            name: "Plutonium",
            bot: true,
            team: false,
            text: "pass the gauss",
        };
        assert_eq!(
            bot.to_string(),
            "» Plutonium: pass the gauss",
            "our bots' lines are never masked"
        );
    }

    #[test]
    fn a_failure_is_not_tried_again_before_the_next_day() {
        let dir = dir("failure");
        let logs = dir.join("logs");
        std::fs::write(&logs, "a file where the directory should be").unwrap();
        let mut log = ChatLog::new(logs.clone());
        log.map("crossfire");
        log.write_at(DAY, "10:00:00", 30, player("Gordon", "раз"));
        assert_eq!(log.failed.as_deref(), Some(DAY));
        std::fs::remove_file(&logs).unwrap();
        log.write_at(DAY, "10:00:05", 30, player("Gordon", "два"));
        assert!(!logs.exists(), "not tried again the same day");
        log.write_at("2026-10-11", "00:00:01", 30, player("Gordon", "три"));
        assert_eq!(
            read(&logs, "2026-10-11"),
            "---- crossfire ----\n00:00:01   Gordon: три\n"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn close_tries_a_failed_directory_again() {
        let dir = dir("reopen");
        let logs = dir.join("logs");
        std::fs::write(&logs, "a file where the directory should be").unwrap();
        let mut log = ChatLog::new(logs.clone());
        log.map("crossfire");
        log.write_at(DAY, "10:00:00", 30, player("Gordon", "раз"));
        std::fs::remove_file(&logs).unwrap();
        log.close();
        log.write_at(DAY, "10:00:05", 30, player("Gordon", "два"));
        assert_eq!(read(&logs, DAY), "---- crossfire ----\n10:00:05   Gordon: два\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn status_tells_where_the_chat_goes_or_why_not() {
        let log = ChatLog::new(PathBuf::from("logs"));
        assert_eq!(ChatLog::status(None, true, 30), "chatlog: not written in a replay");
        assert_eq!(ChatLog::status(Some(&log), false, 30), "chatlog: off (chat.chatlog)");
        let on = ChatLog::status(Some(&log), true, 30);
        assert!(
            on.starts_with("chatlog: logs") && on.ends_with(".log (30 days kept)"),
            "{on}"
        );
        let one = ChatLog::status(Some(&log), true, 1);
        assert!(one.ends_with(".log (1 day kept)"), "{one}");
    }
}
