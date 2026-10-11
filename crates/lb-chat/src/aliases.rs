//! Short names players go by (`Атлас` or `Атласыч` for `ATLAS Gamer`, `Бобр` for `xX_Bobr_Xx`): the prompt shows
//! players by them, and a bot's line says the main one instead of the full nickname. A player without one goes by
//! the nickname without its tags ([`short`]).

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
}
