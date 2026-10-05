//! Short names players go by (`Атлас` or `Атласыч` for `ATLAS Gamer`, `Ник` for `ET^NiK`): the prompt shows players
//! by them, and a bot's line says the main one instead of the full nickname.

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Aliases {
    /// (nickname, its aliases, the main one first), the longest nickname first.
    names: Vec<(String, Vec<String>)>,
}

/// Where `needle` starts in `hay` at char boundary `from` or later, ignoring case: (start, end) byte offsets.
fn find_ci(hay: &str, needle: &str, from: usize) -> Option<(usize, usize)> {
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

    /// `line` with every whole nickname that has aliases written as the main one.
    pub fn apply(&self, line: &str) -> String {
        let mut out = line.to_string();
        for (nick, aliases) in &self.names {
            let alias = &aliases[0];
            let mut from = 0;
            while let Some((start, end)) = find_ci(&out, nick, from) {
                let before = out[..start].chars().next_back();
                let after = out[end..].chars().next();
                let word = |c: Option<char>| c.is_some_and(char::is_alphanumeric);
                if word(before) || word(after) {
                    from = end;
                    continue;
                }
                out.replace_range(start..end, alias);
                from = start + alias.len();
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn aliases() -> Aliases {
        let mut a = Aliases::default();
        a.insert("ATLAS Gamer", &["Атлас", "Атласыч"]);
        a.insert("ET^NiK", &["Ник"]);
        a.insert("Bob", &["Бобик"]);
        a.insert("Bobby", &["Бобби"]);
        a
    }

    #[test]
    fn lookup_ignores_case_and_noise() {
        let a = aliases();
        assert_eq!(a.of("atlas gamer"), Some("Атлас"));
        assert_eq!(a.of(" ET^NiK "), Some("Ник"));
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
            a.apply("ATLAS Gamer, сам ты читер"),
            "Атлас, сам ты читер",
            "the main alias"
        );
        assert_eq!(
            a.apply("ну atlas gamer и et^nik опять вдвоём"),
            "ну Атлас и Ник опять вдвоём"
        );
        assert_eq!(a.apply("Bobby и Bob"), "Бобби и Бобик", "the longest nickname first");
        assert_eq!(a.apply("Bobcat"), "Bobcat", "inside a word stays");
        assert_eq!(a.apply("gg ET^NiK)))"), "gg Ник)))");
        assert_eq!(a.apply("без ников"), "без ников");
    }
}
