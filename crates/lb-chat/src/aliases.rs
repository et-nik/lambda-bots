//! Short names players go by (`Атлас` or `Атласыч` for `ATLAS Gamer`, `Бобр` for `xX_Bobr_Xx`): the prompt shows
//! players by them, and a bot's line says the main one instead of the full nickname. They are the admin's
//! (`players.yaml`), else those the model gave the nickname ([`request`]); a player without any goes by the nickname
//! without its tags ([`short`]).

use std::collections::BTreeMap;
use std::fmt::Write;

use crate::lang::{self, Lang};
use crate::memory::same_nickname;
use crate::profanity;
use crate::prompt::{self, Rendered};

/// Nicknames one request for names covers.
pub const ASKED_MAX: usize = 12;
/// Names of a nickname in one language.
const NAMES_MAX: usize = 3;
/// Longest name in characters.
const NAME_MAX: usize = 24;

const NAMES_RU: &str = "\
Ты подбираешь игрокам сервера Half-Life простые имена для чата — как звали бы их свои: коротко, легко набрать. \
Убери клан-теги, цифры и значки, склей буквы, разбитые пробелами; латиницу, похожую на кириллицу, читай по-русски; \
из слов бери главное, можно перевести. Без оскорблений и насмешек. 1–3 имени, главное первым; нет хорошего — [].
Примеры (ru | en): Lazy Pharaoh → фараон | Pharaoh; Moss → Мосс | Moss; Monitor7 → Монитор, мон | Monitor, mon; \
[N] C o B A → сова | Sova.";

const NAMES_EN: &str = "\
You pick simple names for the players of a Half-Life server to be called by in the chat, as their friends would \
call them: short and easy to type. Drop clan tags, digits and symbols, join letters spaced apart; take the main word, \
it may be translated. No insults or mockery. 1–3 names, the main one first; none fits: [].
Examples: Lazy Pharaoh → Pharaoh; Moss → Moss; Monitor7 → Monitor, mon; [N] C o B A → Coba.";

/// The languages a server's names are asked in: its own (`chat.language`, by the primary subtag) and English.
pub fn languages(server: &str) -> Vec<String> {
    let own = lang::primary(server).to_ascii_lowercase();
    let mut codes = vec![if own.is_empty() { "en".to_string() } else { own }];
    if codes[0] != "en" {
        codes.push("en".into());
    }
    codes
}

/// The request for names of `nicks` in the languages `codes` ([`languages`]), numbered: the answer goes by number.
pub fn request(nicks: &[String], codes: &[String]) -> Rendered {
    let ru = codes.first().is_some_and(|c| Lang::of(c) == Lang::Ru);
    let mut user = String::from(if ru { "Ники:\n" } else { "Nicknames:\n" });
    for (i, nick) in nicks.iter().enumerate() {
        let _ = writeln!(user, "{}. {}", i + 1, nick.trim());
    }
    let langs: Vec<String> = codes
        .iter()
        .map(|code| match (ru, code.as_str()) {
            (true, "ru") => "\"ru\": [имена по-русски]".to_string(),
            (true, _) => format!("\"{code}\": [имена латиницей]"),
            (false, _) => format!("\"{code}\": [names in {}]", prompt::language_name(code)),
        })
        .collect();
    let shape = format!("{{\"1\": {{{}}}, ...}}", langs.join(", "));
    let _ = if ru {
        write!(user, "Ответ — только JSON: {shape}")
    } else {
        write!(user, "Answer with a JSON object only: {shape}")
    };
    Rendered {
        system_static: (if ru { NAMES_RU } else { NAMES_EN }).to_string(),
        system: String::new(),
        user,
        max_tokens: Some(60 + 40 * nicks.len() as u32),
    }
}

/// The names in the model's answer to a [`request`] for `asked` nicknames in the languages `codes`, in the order of
/// the nicknames: nothing for one the answer leaves out; else its names in each language, the main one first, none
/// where the answer has none that fits ([`fit`]). Language codes are read in any case.
pub fn parse_answer(text: &str, asked: usize, codes: &[String]) -> Vec<Option<BTreeMap<String, Vec<String>>>> {
    let mut out = vec![None; asked];
    for (number, value) in prompt::json_object(text).unwrap_or_default() {
        let Some(slot) = number
            .trim()
            .parse::<usize>()
            .ok()
            .and_then(|n| n.checked_sub(1))
            .and_then(|i| out.get_mut(i))
        else {
            continue;
        };
        let Some(by_lang) = value.as_object() else {
            continue;
        };
        let names_in = |code: &str| {
            by_lang
                .iter()
                .find(|(c, _)| c.trim().eq_ignore_ascii_case(code))
                .and_then(|(_, v)| v.as_array())
                .map_or_else(Vec::new, |names| fitting(names.iter().filter_map(|n| n.as_str())))
        };
        *slot = Some(codes.iter().map(|code| (code.clone(), names_in(code))).collect());
    }
    out
}

/// The names that fit ([`fit`]), each once whatever the case, [`NAMES_MAX`] at most.
fn fitting<'a>(names: impl Iterator<Item = &'a str>) -> Vec<String> {
    let mut kept: Vec<String> = Vec::new();
    for name in names.filter_map(fit) {
        if kept.len() < NAMES_MAX && !kept.iter().any(|k| k.to_lowercase() == name.to_lowercase()) {
            kept.push(name);
        }
    }
    kept
}

/// A name the model gave, trimmed, when a bot may say it: up to [`NAME_MAX`] characters with a letter, no quotes,
/// brackets, `%`, `;` or control characters, no swearing or slur ([`profanity::nick`]).
fn fit(name: &str) -> Option<String> {
    let name = name.trim();
    let plain = !name.chars().any(|c| c.is_control() || "\"%;{}[]<>".contains(c));
    let ok =
        plain && name.chars().count() <= NAME_MAX && name.chars().any(char::is_alphabetic) && !profanity::nick(name);
    ok.then(|| name.to_string())
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Aliases {
    /// (nickname, its aliases, the main one first), the longest nickname first.
    names: Vec<(String, Vec<String>)>,
}

/// Where `needle` starts in `hay` at char boundary `from` or later, ignoring case: (start, end) byte offsets.
pub(crate) fn find_ci(hay: &str, needle: &str, from: usize) -> Option<(usize, usize)> {
    let needle: Vec<char> = needle.chars().flat_map(char::to_lowercase).collect();
    if needle.is_empty() {
        return None;
    }
    for (start, _) in hay.char_indices().filter(|(i, _)| *i >= from) {
        let mut rest = hay[start..]
            .char_indices()
            .flat_map(|(i, c)| c.to_lowercase().map(move |l| (i, l)));
        let mut end = None;
        for (n, want) in needle.iter().enumerate() {
            match rest.next() {
                Some((i, got)) if got == *want => {
                    if n + 1 == needle.len() {
                        end = Some(start + i + hay[start + i..].chars().next().map_or(0, char::len_utf8));
                    }
                }
                _ => break,
            }
        }
        if let Some(end) = end {
            return Some((start, end));
        }
    }
    None
}

impl Aliases {
    /// The aliases of `nick`, the main one first; empty ones and the nickname itself are left out.
    pub fn insert<S: AsRef<str>>(&mut self, nick: &str, aliases: &[S]) {
        let nick = nick.trim();
        let mut kept: Vec<String> = Vec::new();
        for alias in aliases.iter().map(|a| a.as_ref().trim()) {
            let seen = |k: &String| k.to_lowercase() == alias.to_lowercase();
            if !alias.is_empty() && alias.to_lowercase() != nick.to_lowercase() && !kept.iter().any(seen) {
                kept.push(alias.to_string());
            }
        }
        if nick.is_empty() || kept.is_empty() || self.of(nick).is_some() {
            return;
        }
        self.names.push((nick.to_string(), kept));
        self.names.sort_by_key(|(n, _)| std::cmp::Reverse(n.chars().count()));
    }

    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    /// Every alias of `nick`, the main one first, case-insensitively.
    pub fn all(&self, nick: &str) -> &[String] {
        let nick = nick.trim().to_lowercase();
        self.names
            .iter()
            .find(|(n, _)| n.to_lowercase() == nick)
            .map_or(&[], |(_, a)| a.as_slice())
    }

    /// Whether `alias` is already that of a nickname other than `nick`, case-insensitively; the engine's `(1)` copy of
    /// `nick` is no other ([`same_nickname`]).
    pub fn taken(&self, alias: &str, nick: &str) -> bool {
        let alias = alias.trim().to_lowercase();
        self.names
            .iter()
            .any(|(n, all)| !same_nickname(n, nick) && all.iter().any(|a| a.to_lowercase() == alias))
    }

    /// The main alias of `nick`.
    pub fn of(&self, nick: &str) -> Option<&str> {
        self.all(nick).first().map(String::as_str)
    }

    /// `nick` as the bots call the player.
    pub fn call<'a>(&'a self, nick: &'a str) -> &'a str {
        self.of(nick).unwrap_or(nick)
    }

    /// Every nickname with aliases and every alias: names a line may hold, to leave out of its checks.
    pub fn words(&self) -> Vec<String> {
        self.names
            .iter()
            .flat_map(|(nick, aliases)| std::iter::once(nick).chain(aliases))
            .cloned()
            .collect()
    }

    /// `line` with every whole nickname that has aliases written as the main one: of overlapping nicknames the longest
    /// wins, and an alias written is never taken for a nickname in turn.
    pub fn apply(&self, line: &str) -> String {
        let mut found: Vec<(usize, usize, &str)> = Vec::new();
        for (nick, aliases) in &self.names {
            let mut from = 0;
            while let Some((start, end)) = find_ci(line, nick, from) {
                from = end;
                let before = line[..start].chars().next_back();
                let after = line[end..].chars().next();
                let word = |c: Option<char>| c.is_some_and(char::is_alphanumeric);
                if !word(before) && !word(after) && !found.iter().any(|&(s, e, _)| s < end && start < e) {
                    found.push((start, end, aliases[0].as_str()));
                }
            }
        }
        found.sort_unstable_by_key(|&(start, _, _)| start);
        let mut out = String::with_capacity(line.len());
        let mut at = 0;
        for (start, end, alias) in found {
            out.push_str(&line[at..start]);
            out.push_str(alias);
            at = end;
        }
        out.push_str(&line[at..]);
        out
    }
}

/// The bracket that closes `open`.
fn closing(open: char) -> Option<char> {
    match open {
        '[' => Some(']'),
        '(' => Some(')'),
        '{' => Some('}'),
        '<' => Some('>'),
        _ => None,
    }
}

/// The bracket that opens `close`.
fn opening(close: char) -> Option<char> {
    match close {
        ']' => Some('['),
        ')' => Some('('),
        '}' => Some('{'),
        '>' => Some('<'),
        _ => None,
    }
}

/// `s` without the bracketed group it starts with (`[B] x` → ` x`), and the group's length in characters.
fn lead_group(s: &str) -> Option<(&str, usize)> {
    let close = closing(s.chars().next()?)?;
    let end = s.find(close)? + 1;
    Some((&s[end..], s[..end].chars().count()))
}

/// `s` without the bracketed group it ends with (`x (1)` → `x `), and the group's length in characters.
fn tail_group(s: &str) -> Option<(&str, usize)> {
    let open = opening(s.chars().next_back()?)?;
    let start = s.rfind(open)?;
    Some((&s[..start], s[start..].chars().count()))
}

/// `s` without what is neither a letter nor a digit at its edges, but for a bracket whose pair stays inside:
/// `[Long Clan Tag]Xy` keeps its `[`, `(x)` is `x`.
fn trim_edges(s: &str) -> &str {
    let plain = |c: char| !c.is_alphanumeric();
    let (start, end) = (
        s.len() - s.trim_start_matches(plain).len(),
        s.trim_end_matches(plain).len(),
    );
    if start >= end {
        return "";
    }
    let inner = &s[start..end];
    let start = s[..start]
        .char_indices()
        .find(|&(_, c)| closing(c).is_some_and(|close| inner.contains(close)))
        .map_or(start, |(i, _)| i);
    let end = s[end..]
        .char_indices()
        .rfind(|&(_, c)| opening(c).is_some_and(|open| inner.contains(open)))
        .map_or(end, |(i, c)| end + i + c.len_utf8());
    &s[start..end]
}

/// `nick` without clan tags and decoration, for a line that names a player who has no alias: `[N] C o B A` →
/// `C o B A`, `=Белка=` → `Белка`, `^5Krot` → `Krot`. A bracketed group at an edge goes when two letters or
/// digits are left without it, one longer than ten characters only when three letters are (`Calm guy (the quiet
/// one)` → `Calm guy`). The nickname itself when fewer than two letters or digits would be left.
pub fn short(nick: &str) -> String {
    let alnum = |s: &str| s.chars().filter(|c| c.is_alphanumeric()).count();
    let letters = |s: &str| s.chars().filter(|c| c.is_alphabetic()).count();
    let goes = |&(rest, len): &(&str, usize)| (len <= 10 && alnum(rest) >= 2) || letters(rest) >= 3;
    let mut s = nick.trim();
    while let Some((rest, _)) = [lead_group(s), tail_group(s)].into_iter().flatten().find(goes) {
        s = rest.trim();
    }
    if let Some(rest) = s.strip_prefix('^')
        && rest.starts_with(|c: char| c.is_ascii_digit())
    {
        s = &rest[1..];
    }
    let s = trim_edges(s);
    if alnum(s) >= 2 { s } else { nick.trim() }.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn aliases() -> Aliases {
        let mut a = Aliases::default();
        a.insert("ATLAS Gamer", &["Атлас", "Атласыч"]);
        a.insert("xX_Bobr_Xx", &["Бобр"]);
        a.insert("Bob", &["Бобик"]);
        a.insert("Bobby", &["Бобби"]);
        a
    }

    #[test]
    fn lookup_ignores_case_and_noise() {
        let a = aliases();
        assert_eq!(a.of("atlas gamer"), Some("Атлас"));
        assert_eq!(a.of(" xX_Bobr_Xx "), Some("Бобр"));
        assert_eq!(a.of("112S"), None);
        assert_eq!(a.call("112S"), "112S");
        assert_eq!(a.all("atlas gamer"), ["Атлас", "Атласыч"]);
        assert!(a.all("112S").is_empty());
        let mut b = Aliases::default();
        b.insert("Kleiner", &["kleiner", " "]);
        b.insert("", &["x"]);
        assert!(b.is_empty());
        b.insert("Gina", &["Джина", "джина", "Gina", "Джинка"]);
        assert_eq!(b.all("Gina"), ["Джина", "Джинка"], "no repeats, no nickname");
    }

    #[test]
    fn lines_say_aliases_for_whole_nicknames() {
        let a = aliases();
        assert_eq!(
            a.apply("ATLAS Gamer, сам ты кемпер"),
            "Атлас, сам ты кемпер",
            "the main alias"
        );
        assert_eq!(
            a.apply("ну atlas gamer и xx_bobr_xx опять вдвоём"),
            "ну Атлас и Бобр опять вдвоём"
        );
        assert_eq!(a.apply("Bobby и Bob"), "Бобби и Бобик", "the longest nickname first");
        assert_eq!(a.apply("Bobcat"), "Bobcat", "inside a word stays");
        assert_eq!(a.apply("gg xX_Bobr_Xx)))"), "gg Бобр)))");
        assert_eq!(a.apply("без ников"), "без ников");
        let mut b = aliases();
        b.insert("Gamer", &["Геймер"]);
        b.insert("Атлас", &["Атлашка"]);
        assert_eq!(
            b.apply("ATLAS Gamer и Gamer, Атлас"),
            "Атлас и Геймер, Атлашка",
            "the longer nickname wins, an alias written stays"
        );
    }

    #[test]
    fn every_name_to_leave_out() {
        let a = aliases();
        let words = a.words();
        for name in [
            "ATLAS Gamer",
            "Атлас",
            "Атласыч",
            "xX_Bobr_Xx",
            "Бобр",
            "Bobby",
            "Бобби",
        ] {
            assert!(words.iter().any(|w| w == name), "{name}");
        }
        assert_eq!(words.len(), 9);
        assert!(Aliases::default().words().is_empty());
    }

    #[test]
    fn short_names_drop_tags_and_decoration() {
        for (nick, short_name) in [
            ("[N] C o B A", "C o B A"),
            ("=Белка=", "Белка"),
            ("[rk-t] black-mesa", "black-mesa"),
            ("3e6pa :>>>", "3e6pa"),
            ("-|NoS|-", "NoS"),
            ("^5Krot", "Krot"),
            ("(1)player", "player"),
            ("GATOS LOCOSSS", "GATOS LOCOSSS"),
            ("0-ByJIKaH-0", "0-ByJIKaH-0"),
            ("[mesa]_tolik", "tolik"),
            ("Gordon (2)", "Gordon"),
            ("[TAG][X] Gordon", "Gordon"),
            ("[ABC]", "ABC"),
            ("[X]", "[X]"),
            ("~*~", "~*~"),
            ("[VeryLongClanTag]Warrior", "Warrior"),
            ("Calm guy (the quiet one)", "Calm guy"),
            ("[Very Long Clan Tag]Xy", "[Very Long Clan Tag]Xy"),
            ("Xy (the very quiet one)", "Xy (the very quiet one)"),
            ("==[Tag]=Name", "[Tag]=Name"),
            ("(x)abc(", "abc"),
        ] {
            assert_eq!(short(nick), short_name, "{nick}");
        }
    }

    fn codes(all: &[&str]) -> Vec<String> {
        all.iter().map(|c| c.to_string()).collect()
    }

    #[test]
    fn names_are_asked_in_the_server_s_language_and_english() {
        assert_eq!(languages("ru"), ["ru", "en"]);
        assert_eq!(languages("ru-RU"), ["ru", "en"]);
        assert_eq!(languages("en"), ["en"]);
        assert_eq!(languages(""), ["en"]);
        assert_eq!(languages("pt-BR"), ["pt", "en"]);
        let nicks = ["[N] C o B A".to_string(), "Lazy Pharaoh".to_string()];
        let r = request(&nicks, &codes(&["ru", "en"]));
        assert!(r.system_static.starts_with("Ты подбираешь"), "{}", r.system_static);
        assert!(
            r.user.starts_with("Ники:\n1. [N] C o B A\n2. Lazy Pharaoh\n"),
            "{}",
            r.user
        );
        assert!(
            r.user
                .ends_with("{\"1\": {\"ru\": [имена по-русски], \"en\": [имена латиницей]}, ...}"),
            "{}",
            r.user
        );
        assert_eq!(r.max_tokens, Some(140));
        let r = request(&nicks[..1], &codes(&["en"]));
        assert!(r.system_static.starts_with("You pick"), "{}", r.system_static);
        assert!(r.user.starts_with("Nicknames:\n1. [N] C o B A\n"), "{}", r.user);
        assert!(
            r.user.ends_with("{\"1\": {\"en\": [names in English]}, ...}"),
            "{}",
            r.user
        );
        let r = request(&nicks[..1], &codes(&["uk", "en"]));
        assert!(r.user.contains("\"uk\": [names in Ukrainian]"), "{}", r.user);
    }

    #[test]
    fn the_answer_gives_names_by_number() {
        let both = codes(&["ru", "en"]);
        let names = |ru: &[&str], en: &[&str]| {
            Some(BTreeMap::from([
                ("en".to_string(), en.iter().map(|n| n.to_string()).collect()),
                ("ru".to_string(), ru.iter().map(|n| n.to_string()).collect()),
            ]))
        };
        let text = "```json\n{\"1\": {\"ru\": [\"сова\", \"Совушка\"], \"en\": [\"Sova\"]}, \"3\": {\"RU\": [], \
                    \"en\": [\"Moss\"]}, \"9\": {\"ru\": [\"x\"]}, \"a\": {}}\n```";
        assert_eq!(
            parse_answer(text, 3, &both),
            [names(&["сова", "Совушка"], &["Sova"]), None, names(&[], &["Moss"])],
            "the second left out, a number beyond and a key no number ignored"
        );
        assert_eq!(
            parse_answer("{\"1\": {\"ru\": [\"Мосс\"]}}", 1, &both),
            [names(&["Мосс"], &[])],
            "a language left out has no names"
        );
        assert_eq!(parse_answer("не знаю", 2, &both), [None, None]);
        assert_eq!(parse_answer("{\"1\": [\"Мосс\"]}", 1, &both), [None], "not by language");
    }

    #[test]
    fn names_a_bot_may_not_say_are_dropped() {
        let text = "{\"1\": {\"en\": [\" Moss \", \"moss\", \"\", \"1337\", \"Mo;ss\", \"[M]oss\", \"Mo\\nss\", \
                    \"Mossy the green one of the swamp\", \"suka\", \"Mossy\", \"Mo\", \"M\"]}}";
        assert_eq!(
            parse_answer(text, 1, &codes(&["en"])),
            [Some(BTreeMap::from([(
                "en".to_string(),
                vec!["Moss".to_string(), "Mossy".into(), "Mo".into()]
            )]))],
            "trimmed, once, with a letter, no `;` or brackets or new lines, short, no swearing, three at most"
        );
    }

    #[test]
    fn a_name_another_nickname_has_is_taken() {
        let a = aliases();
        assert!(a.taken("атлас", "Gordon"));
        assert!(!a.taken("Атлас", "ATLAS Gamer"), "its own");
        assert!(
            !a.taken("Атлас", "(1)ATLAS Gamer"),
            "the ghost of a reconnect is the same player"
        );
        assert!(!a.taken("Гордон", "Gordon"));
    }
}
