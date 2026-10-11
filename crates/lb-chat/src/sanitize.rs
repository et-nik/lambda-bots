//! Text in and out of the chat: players' lines as the game shows them, the model's answer as one line, and what a
//! bot may send with `say`.

/// What a plugin's chat command starts with (`/top15`, `!level`, `@admin`).
const COMMAND_PREFIXES: [char; 3] = ['/', '!', '@'];
/// First words of a line that a password follows: login and registration commands.
const SECRET_WORDS: [&str; 8] = ["login", "reg", "register", "password", "pass", "pw", "setpw", "auth"];

/// Bytes a line may take: `Host_Say` builds `"\x02<name>: <text>\n"` in 128 bytes (`say_team` adds `(TEAM) `) and
/// cuts the text to fit.
pub fn say_budget(netname: &str, team: bool) -> usize {
    let used = 1 + netname.len() + 2 + 2 + if team { 7 } else { 0 };
    128usize.saturating_sub(used)
}

/// A player's line as `Host_Say` shows it: the command's arguments without the quotes around them.
pub fn player_line(args: &str) -> String {
    let t = args.trim();
    let t = t.strip_prefix('"').and_then(|t| t.strip_suffix('"')).unwrap_or(t);
    t.chars()
        .filter(|c| !c.is_control())
        .collect::<String>()
        .trim()
        .to_string()
}

/// A plugin's chat command rather than chat: a command prefix, or a first word in `blocked`.
pub fn is_command(text: &str, blocked: &[String]) -> bool {
    let t = text.trim_start();
    t.starts_with(COMMAND_PREFIXES) || first_word_blocked(t, blocked)
}

fn first_word_blocked(text: &str, blocked: &[String]) -> bool {
    let first = text.split_whitespace().next().unwrap_or("");
    let first = first.trim_end_matches(|c: char| c.is_ascii_punctuation());
    !first.is_empty() && blocked.iter().any(|b| first.eq_ignore_ascii_case(b.trim()))
}

/// A login or registration command with a password after it, which is no chat: the first word, in lower case and
/// without one leading `/`, `!` or `.`, is one of `SECRET_WORDS` and something follows it. After a bare `pass` or
/// `pw`, without one of these in front, exactly one word does, so `pass the gauss` stays chat.
pub fn secret(text: &str) -> bool {
    let mut words = text.split_whitespace();
    let first = words.next().unwrap_or_default().to_lowercase();
    let command = first.strip_prefix(['/', '!', '.']).unwrap_or(&first);
    let prefixed = command.len() != first.len();
    let after = words.count();
    match command {
        "pass" | "pw" if !prefixed => after == 1,
        _ => after > 0 && SECRET_WORDS.contains(&command),
    }
}

/// The model's answer as one chat line: the first line, without quotes around it or the bot's own name in front.
/// `None` for silence: `-`, nothing, punctuation only, or a stage direction like `(молчит)`.
pub fn clean_reply(text: &str, bot_name: &str) -> Option<String> {
    let line = text.lines().map(str::trim).find(|l| !l.is_empty())?;
    let mut line = unquote(line).trim();
    for prefix in [bot_name.trim(), "Ты", "You", "Я", "Me"] {
        if let Some(rest) = strip_ci(line, prefix).and_then(|r| r.trim_start().strip_prefix(':')) {
            line = unquote(rest.trim()).trim();
        }
    }
    let wrapped = |open: char, close: char| line.starts_with(open) && line.ends_with(close) && line.len() > 1;
    if wrapped('(', ')') || wrapped('*', '*') || wrapped('[', ']') {
        return None;
    }
    if !line.chars().any(char::is_alphanumeric) && !line.contains([')', '(', '+', '?']) {
        return None;
    }
    if line.chars().all(|c| matches!(c, '-' | '—' | '–' | '.' | '…' | ' ')) {
        return None;
    }
    Some(line.split_whitespace().collect::<Vec<_>>().join(" "))
}

fn unquote(s: &str) -> &str {
    for (open, close) in [('"', '"'), ('«', '»'), ('“', '”'), ('\'', '\''), ('„', '“')] {
        if let Some(inner) = s.strip_prefix(open).and_then(|r| r.strip_suffix(close))
            && !inner.contains(close)
        {
            return inner;
        }
    }
    s
}

fn strip_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    if prefix.is_empty() {
        return None;
    }
    let head = s.get(..prefix.len())?;
    head.to_lowercase()
        .eq(&prefix.to_lowercase())
        .then(|| &s[prefix.len()..])
}

/// Characters the game's fonts show: printable ASCII, Latin-1 letters, Cyrillic and common punctuation.
fn shown(c: char) -> bool {
    matches!(c, ' '..='~' | '\u{a0}'..='\u{ff}' | '\u{400}'..='\u{4ff}' | '\u{2010}'..='\u{2027}' | '№')
}

/// What a bot sends with `say`: characters the game shows, no `"` or `%`, no command in front or blocked first
/// word, at most `budget` bytes cut on a character (and, when possible, word) boundary. `ascii_needed`: the game
/// DLL drops a line without a printable ASCII character (the SDK's `Host_Say` before 2023), so pure Cyrillic gets a
/// `)`. `None` when nothing is left to say.
pub fn fit_say(text: &str, budget: usize, blocked: &[String], ascii_needed: bool) -> Option<String> {
    let cleaned: String = text
        .chars()
        .map(|c| if c == '"' { '\'' } else { c })
        .filter(|&c| c != '%' && shown(c))
        .collect();
    let mut line = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let start = line
        .find(|c: char| !(COMMAND_PREFIXES.contains(&c) || c == '.' || c == '#' || c == ' '))
        .unwrap_or(line.len());
    line.drain(..start);
    if line.is_empty() || first_word_blocked(&line, blocked) {
        return None;
    }
    if line.len() > budget {
        let mut cut = budget;
        while !line.is_char_boundary(cut) {
            cut -= 1;
        }
        line.truncate(cut);
        if let Some(space) = line.rfind(' ')
            && space * 10 >= budget * 6
        {
            line.truncate(space);
        }
        line = line.trim_end().to_string();
    }
    if ascii_needed && !line.chars().any(|c| c.is_ascii_graphic()) {
        while line.len() + 1 > budget {
            line.pop()?;
        }
        line.push(')');
    }
    (!line.trim().is_empty()).then_some(line)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blocked() -> Vec<String> {
        ["rtv", "nominate"].into_iter().map(String::from).collect()
    }

    #[test]
    fn budgets_match_host_say() {
        assert_eq!(say_budget("Player", false), 117);
        assert_eq!(say_budget("Player", true), 110);
        assert_eq!(say_budget(&"x".repeat(31), false), 92);
    }

    #[test]
    fn player_lines_and_commands() {
        assert_eq!(player_line("\"привет всем\""), "привет всем");
        assert_eq!(player_line("  gg wp "), "gg wp");
        assert!(is_command("/top15", &blocked()));
        assert!(is_command("rtv", &blocked()));
        assert!(is_command("RTV!", &blocked()));
        assert!(!is_command("rtv это зло", &[]));
        assert!(!is_command("ну ты и кемпер", &blocked()));
    }

    #[test]
    fn login_lines_carry_a_password() {
        for line in [
            "/login hunter2",
            ".login hunter2",
            "login hunter2",
            "!REG hunter2 hunter2",
            "Register hunter2",
            "password hunter2",
            "pass hunter2",
            ".PW hunter2",
            "setpw hunter2",
            "  auth\t1234 ",
            "/pw a b",
            "!pass a b",
            ".pass a b",
            ".pw a b",
        ] {
            assert!(secret(line), "{line}");
        }
        for line in [
            "login",
            "/login",
            "pass",
            "pass the gauss",
            "pw is not easy here",
            "I can't login, what is the password",
            "loginhunter2 now",
            "//login hunter2",
            "gg",
            "",
        ] {
            assert!(!secret(line), "{line}");
        }
    }

    #[test]
    fn replies_become_one_line_or_silence() {
        assert_eq!(clean_reply("\"гг\"\nА ещё…", "Bob").as_deref(), Some("гг"));
        assert_eq!(clean_reply("Bob: ну и ладно", "Bob").as_deref(), Some("ну и ладно"));
        assert_eq!(clean_reply("bob:  «изи»", "Bob").as_deref(), Some("изи"));
        assert_eq!(clean_reply("-", "Bob"), None);
        assert_eq!(clean_reply("  ", "Bob"), None);
        assert_eq!(clean_reply("(молчит)", "Bob"), None);
        assert_eq!(clean_reply("...", "Bob"), None);
        assert_eq!(clean_reply(")))", "Bob").as_deref(), Some(")))"));
        assert_eq!(clean_reply("ага,   бот я ))", "Bob").as_deref(), Some("ага, бот я ))"));
    }

    #[test]
    fn say_lines_fit_the_game() {
        assert_eq!(fit_say("100% \"изи\"", 100, &[], false).as_deref(), Some("100 'изи'"));
        assert_eq!(fit_say("/kill me", 100, &[], false).as_deref(), Some("kill me"));
        assert_eq!(fit_say("!! rtv pls", 100, &blocked(), false), None);
        assert_eq!(fit_say("gg 🔥🔥", 100, &[], false).as_deref(), Some("gg"));
        assert_eq!(fit_say("\u{1}\u{2}гг\u{7}", 100, &[], false).as_deref(), Some("гг"));
        assert_eq!(fit_say("гг", 100, &[], true).as_deref(), Some("гг)"));
        assert_eq!(fit_say("гг!", 100, &[], true).as_deref(), Some("гг!"));
        assert_eq!(fit_say("🔥", 100, &[], false), None);
    }

    #[test]
    fn cuts_stay_on_characters_and_words() {
        let long = "очень длинная фраза которая никак не помещается в строку чата целиком".repeat(3);
        for budget in 10..120 {
            let line = fit_say(&long, budget, &[], false).unwrap();
            assert!(line.len() <= budget, "{budget}: {line}");
            assert!(!line.ends_with(' '));
        }
        let line = fit_say(&long, 60, &[], false).unwrap();
        assert!(long.starts_with(&line) && long[line.len()..].starts_with(' '), "{line}");
        let tight = fit_say("ааааа", 5, &[], true).unwrap();
        assert!(tight.len() <= 5 && tight.ends_with(')'), "{tight}");
    }

    #[test]
    fn property_any_input_is_safe() {
        let mut rng = lb_core::rng::Pcg32::new(5, 9);
        let alphabet: Vec<char> = "aZ09 /!@.#%\"\u{1}\u{7f}жЖ…—🔥\u{a0}é\n\t\\;".chars().collect();
        for _ in 0..2000 {
            let len = rng.range_i32(0, 80) as usize;
            let text: String = (0..len)
                .map(|_| alphabet[rng.range_i32(0, alphabet.len() as i32 - 1) as usize])
                .collect();
            let budget = rng.range_i32(1, 120) as usize;
            let ascii = rng.chance(50.0);
            if let Some(line) = fit_say(&text, budget, &blocked(), ascii) {
                assert!(line.len() <= budget, "{text:?} {budget}");
                assert!(line.chars().all(|c| !c.is_control() && c != '"' && c != '%'));
                assert!(!line.starts_with(['/', '!', '@', '.', '#', ' ']));
                assert!(!ascii || line.chars().any(|c| c.is_ascii_graphic()));
                assert!(!line.trim().is_empty());
            }
        }
    }
}
