//! `config/chat/phrases.yaml`: ready lines for game moments, by language, for every bot and for single bots.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::ConfigError;
use crate::chat_bots::check_names;
use crate::main_config::language_code;
use crate::yaml;

pub const KIND: &str = "chat-phrases";
pub const MAJOR: u32 = 1;
/// Longest phrase in characters, placeholders unfilled.
pub const PHRASE_MAX: usize = 60;
/// Most phrases of one moment in one language.
pub const PHRASES_MAX: usize = 64;
/// The placeholder every moment takes: the map's name.
pub const MAP_FILL: &str = "map";

/// A moment a bot may meet with a ready phrase. `{name}` in its phrases is always the other player of the moment.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Moment {
    /// A player the bots do not know joins or greets everybody.
    Greet,
    /// The bot's own streak of 10 frags or more, humans among them.
    Streak,
    /// A human's streak of 10 frags or more.
    StreakOther,
    /// The bot's own multikill, humans among the killed.
    Multikill,
    /// A human's multikill.
    MultikillOther,
    /// The bot kills the one who killed it 3 times or more in a row.
    Revenge,
    /// A human kills the bot the 3rd time in a row, then the 5th, the 7th…
    Nemesis,
    /// A human kills the bot with the crowbar, not in GunGame.
    Crowbarred,
    /// The bot kills a human with the crowbar, not in GunGame.
    CrowbarKill,
    /// The bot blows itself up.
    OwnBlast,
    /// A human blows themselves up.
    OwnBlastOther,
    /// A human kills the bot while it types.
    KilledTyping,
    /// The bot reaches GunGame's last level.
    LastLevel,
    /// The match ends and the bot won.
    Win,
    /// The match ends and a human won.
    Gg,
}

impl Moment {
    pub const ALL: [Moment; 15] = [
        Moment::Greet,
        Moment::Streak,
        Moment::StreakOther,
        Moment::Multikill,
        Moment::MultikillOther,
        Moment::Revenge,
        Moment::Nemesis,
        Moment::Crowbarred,
        Moment::CrowbarKill,
        Moment::OwnBlast,
        Moment::OwnBlastOther,
        Moment::KilledTyping,
        Moment::LastLevel,
        Moment::Win,
        Moment::Gg,
    ];

    /// The moment's key in `phrases.yaml`.
    pub fn key(self) -> &'static str {
        match self {
            Moment::Greet => "greet",
            Moment::Streak => "streak",
            Moment::StreakOther => "streak_other",
            Moment::Multikill => "multikill",
            Moment::MultikillOther => "multikill_other",
            Moment::Revenge => "revenge",
            Moment::Nemesis => "nemesis",
            Moment::Crowbarred => "crowbarred",
            Moment::CrowbarKill => "crowbar_kill",
            Moment::OwnBlast => "own_blast",
            Moment::OwnBlastOther => "own_blast_other",
            Moment::KilledTyping => "killed_typing",
            Moment::LastLevel => "last_level",
            Moment::Win => "win",
            Moment::Gg => "gg",
        }
    }

    /// The moment of a `phrases.yaml` key.
    pub fn parse(key: &str) -> Option<Moment> {
        Moment::ALL.into_iter().find(|m| m.key() == key)
    }

    /// The placeholders the moment's phrases take besides [`MAP_FILL`]: `name` (the other player), `count` (a
    /// number), `weapon` (as a phrase: "сачелем", "with a satchel").
    pub fn fills(self) -> &'static [&'static str] {
        match self {
            Moment::Greet
            | Moment::Revenge
            | Moment::Crowbarred
            | Moment::CrowbarKill
            | Moment::KilledTyping
            | Moment::Gg => &["name"],
            Moment::Streak | Moment::Multikill => &["count"],
            Moment::StreakOther | Moment::MultikillOther | Moment::Nemesis => &["name", "count"],
            Moment::OwnBlast => &["weapon"],
            Moment::OwnBlastOther => &["name", "weapon"],
            Moment::LastLevel | Moment::Win => &[],
        }
    }

    /// The moment's phrases may use the placeholder `fill`.
    pub fn allows(self, fill: &str) -> bool {
        fill == MAP_FILL || self.fills().contains(&fill)
    }
}

/// Phrases by language (`ru`, `en`), then by moment.
pub type Phrasebook = BTreeMap<String, BTreeMap<Moment, Vec<String>>>;

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ChatPhrasesFile {
    pub schema: String,
    /// Every bot's phrases.
    #[serde(default)]
    pub phrases: Phrasebook,
    #[serde(default)]
    pub bots: Vec<BotPhrases>,
}

/// One bot's own phrases.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct BotPhrases {
    /// The personality's name as `lb list` shows it, or the bot's nickname in the game.
    pub name: String,
    /// The bot's phrases replace every bot's for the moments it lists, instead of joining them.
    #[serde(default)]
    pub replace: bool,
    #[serde(default)]
    pub phrases: Phrasebook,
}

impl ChatPhrasesFile {
    pub fn parse(text: &str, path: &str) -> Result<ChatPhrasesFile, ConfigError> {
        let f: ChatPhrasesFile = yaml::from_str(text, path)?;
        yaml::check_schema(&f.schema, KIND, MAJOR, path)?;
        check_book(&f.phrases, "phrases", path)?;
        check_names(f.bots.iter().map(|b| b.name.as_str()), path)?;
        for (i, b) in f.bots.iter().enumerate() {
            check_book(&b.phrases, &format!("bots[{i}].phrases"), path)?;
        }
        Ok(f)
    }

    /// The own phrases of the bot with this personality name or nickname, in any case.
    pub fn bot(&self, name: &str) -> Option<&BotPhrases> {
        let name = name.to_lowercase();
        self.bots.iter().find(|b| b.name.to_lowercase() == name)
    }
}

fn check_book(book: &Phrasebook, at: &str, path: &str) -> Result<(), ConfigError> {
    let bad = |field: String, message: String| ConfigError::Invalid {
        path: path.to_string(),
        field,
        message,
    };
    for (lang, moments) in book {
        if !language_code(lang) {
            return Err(bad(
                format!("{at}.{lang}"),
                "expected a language code such as ru or en".into(),
            ));
        }
        for (&moment, list) in moments {
            let at = format!("{at}.{lang}.{}", moment.key());
            if list.len() > PHRASES_MAX {
                return Err(bad(at, format!("at most {PHRASES_MAX} phrases")));
            }
            for (i, phrase) in list.iter().enumerate() {
                check_phrase(phrase, moment).map_err(|message| bad(format!("{at}[{i}]"), message))?;
            }
        }
    }
    Ok(())
}

fn check_phrase(phrase: &str, moment: Moment) -> Result<(), String> {
    let p = phrase.trim();
    if !(1..=PHRASE_MAX).contains(&p.chars().count()) {
        return Err(format!("must be 1..={PHRASE_MAX} characters"));
    }
    if p.chars().any(|c| c.is_control() || c == '"' || c == '%') {
        return Err("must have no `\"`, `%` or control characters".into());
    }
    if p.starts_with(['/', '!', '@', '.', '#']) {
        return Err("must not start with `/`, `!`, `@`, `.` or `#`: the bot would cut it off".into());
    }
    for fill in placeholders(p)? {
        if !moment.allows(fill) {
            let takes: Vec<String> = moment
                .fills()
                .iter()
                .chain([&MAP_FILL])
                .map(|f| format!("{{{f}}}"))
                .collect();
            return Err(format!(
                "`{{{fill}}}` is not a placeholder of {}, which takes {}",
                moment.key(),
                takes.join(" ")
            ));
        }
    }
    Ok(())
}

/// A part of a phrase: plain text, or the name of a placeholder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Piece<'a> {
    Text(&'a str),
    Fill(&'a str),
}

/// `text` cut at its placeholders: `{name}, ну ты даёшь` is `[Fill("name"), Text(", ну ты даёшь")]`. Braces come
/// only in pairs, around a placeholder.
pub fn pieces(text: &str) -> Result<Vec<Piece<'_>>, String> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find(['{', '}']) {
        if rest[at..].starts_with('}') {
            return Err("has a `}` without its `{`".into());
        }
        let inside = &rest[at + 1..];
        let Some(end) = inside.find(['{', '}']).filter(|&i| inside[i..].starts_with('}')) else {
            return Err("has a `{` without its `}`".into());
        };
        if at > 0 {
            out.push(Piece::Text(&rest[..at]));
        }
        out.push(Piece::Fill(&inside[..end]));
        rest = &inside[end + 1..];
    }
    if !rest.is_empty() {
        out.push(Piece::Text(rest));
    }
    Ok(out)
}

/// The names of the placeholders in `text`, in order.
pub fn placeholders(text: &str) -> Result<Vec<&str>, String> {
    Ok(pieces(text)?
        .into_iter()
        .filter_map(|p| match p {
            Piece::Fill(fill) => Some(fill),
            Piece::Text(_) => None,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEAD: &str = "schema: lambdabots/chat-phrases@1\n";

    #[test]
    fn moments_are_map_keys() {
        let book: Phrasebook = yaml::from_str(
            "ru:\n  greet: [привет]\n  crowbar_kill: [\"{name}, бывает\"]\nen: {}\n",
            "t",
        )
        .unwrap();
        assert_eq!(book["ru"][&Moment::Greet], ["привет"]);
        assert_eq!(book["ru"][&Moment::CrowbarKill], ["{name}, бывает"]);
        assert!(book["en"].is_empty());
        let e = yaml::from_str::<Phrasebook>("ru:\n  greeting: [привет]\n", "t").unwrap_err();
        assert!(e.to_string().contains("greeting"), "{e}");
        assert!(yaml::from_str::<Phrasebook>("ru:\n  Greet: [привет]\n", "t").is_err());
        assert!(
            yaml::from_str::<Phrasebook>("ru:\n  greet: [a]\n  greet: [b]\n", "t").is_err(),
            "a moment twice"
        );
    }

    #[test]
    fn the_moment_table() {
        for (i, m) in Moment::ALL.into_iter().enumerate() {
            assert_eq!(m as usize, i, "ALL in declaration order");
            assert_eq!(Moment::parse(m.key()), Some(m));
            assert_eq!(
                yaml::from_str::<Moment>(m.key(), "t").unwrap(),
                m,
                "serde names are the keys"
            );
            assert!(m.allows(MAP_FILL) && !m.fills().contains(&MAP_FILL));
            assert!(m.fills().iter().all(|f| ["name", "count", "weapon"].contains(f)));
        }
        assert_eq!(Moment::parse("rage_quit"), None);
        assert!(Moment::Nemesis.allows("count") && !Moment::Revenge.allows("count"));
        assert!(Moment::OwnBlast.allows("weapon") && !Moment::OwnBlast.allows("name"));
        assert!(Moment::Win.fills().is_empty() && Moment::LastLevel.fills().is_empty());
    }

    #[test]
    fn placeholders_are_found() {
        assert_eq!(
            pieces("{name}, ну ты даёшь").unwrap(),
            [Piece::Fill("name"), Piece::Text(", ну ты даёшь")]
        );
        assert_eq!(
            pieces("уже {count} на {map}").unwrap(),
            [
                Piece::Text("уже "),
                Piece::Fill("count"),
                Piece::Text(" на "),
                Piece::Fill("map")
            ]
        );
        assert_eq!(
            pieces("{name}{count}").unwrap(),
            [Piece::Fill("name"), Piece::Fill("count")]
        );
        assert_eq!(pieces("просто так").unwrap(), [Piece::Text("просто так")]);
        assert!(pieces("").unwrap().is_empty());
        assert_eq!(placeholders("{name}: {count}-й раз").unwrap(), ["name", "count"]);
        assert_eq!(placeholders("{}").unwrap(), [""]);
        for bad in ["{name", "name}", "{na{me}", "{name}}", "}{", "а {name} {"] {
            assert!(pieces(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn parses_a_file() {
        let text = format!(
            "{HEAD}phrases:\n  ru:\n    greet: [\"привет, {{name}}\", здарова]\n    win: [\"gg, {{map}} моя\"]\n  en:\n    gg: [\"gg {{name}}\"]\nbots:\n  - name: \"[B] Plutonium\"\n    replace: true\n    phrases:\n      ru:\n        win: [изи]\n        gg: []\n  - name: Ёжик\n"
        );
        let f = ChatPhrasesFile::parse(&text, "phrases.yaml").unwrap();
        assert_eq!(f.phrases["ru"][&Moment::Greet], ["привет, {name}", "здарова"]);
        let bot = f.bot("[b] plutonium").unwrap();
        assert!(bot.replace);
        assert!(bot.phrases["ru"][&Moment::Gg].is_empty());
        let other = f.bot("ЁЖИК").unwrap();
        assert!(!other.replace && other.phrases.is_empty());
        assert!(f.bot("Kleiner").is_none());
        let empty = ChatPhrasesFile::parse(HEAD, "p").unwrap();
        assert!(empty.phrases.is_empty() && empty.bots.is_empty());
    }

    #[test]
    fn rejects_bad_phrases() {
        let err = |list: &str| {
            ChatPhrasesFile::parse(&format!("{HEAD}phrases:\n  ru:\n    streak:\n{list}"), "p")
                .unwrap_err()
                .to_string()
        };
        let long = format!("      - {}\n", "я".repeat(PHRASE_MAX + 1));
        for list in [
            "      - \"\"\n",
            "      - \"   \"\n",
            long.as_str(),
            "      - 'скажи \"привет\"'\n",
            "      - 100% моё\n",
            "      - \"раз\\tдва\"\n",
            "      - /top\n",
            "      - \"!изи\"\n",
            "      - \"@all\"\n",
            "      - ...ну\n",
            "      - \"#1\"\n",
            "      - \"уже {count\"\n",
        ] {
            assert!(err(list).contains("phrases.ru.streak[0]"), "{list}");
        }
        let fits = format!(
            "{HEAD}phrases:\n  ru:\n    streak:\n      - \"  {}  \"\n",
            "я".repeat(PHRASE_MAX)
        );
        assert!(ChatPhrasesFile::parse(&fits, "p").is_ok());
        let e = err("      - ok\n      - \"{name} молодец\"\n");
        assert!(
            e.contains("phrases.ru.streak[1]")
                && e.contains("`{name}`")
                && e.contains("streak, which takes {count} {map}"),
            "{e}"
        );
        let many = "      - x\n".repeat(PHRASES_MAX + 1);
        assert!(err(&many).contains("phrases.ru.streak"));
        let max = "      - x\n".repeat(PHRASES_MAX);
        assert!(ChatPhrasesFile::parse(&format!("{HEAD}phrases:\n  ru:\n    streak:\n{max}"), "p").is_ok());
    }

    #[test]
    fn rejects_bad_files() {
        let err = |body: &str| {
            ChatPhrasesFile::parse(&format!("{HEAD}{body}"), "p")
                .unwrap_err()
                .to_string()
        };
        assert!(err("phrases:\n  русский:\n    win: [gg]\n").contains("phrases.русский"));
        assert!(err("phrases:\n  r:\n    win: [gg]\n").contains("phrases.r"));
        assert!(err("phrases:\n  ru:\n    rage_quit: [пока]\n").contains("rage_quit"));
        assert!(err("bots:\n  - name: \"a;b\"\n").contains("bots[0].name"));
        let dup = err("bots:\n  - name: Ёжик\n  - name: ёжик\n");
        assert!(dup.contains("bots[1].name"), "{dup}");
        let e = err("bots:\n  - name: a\n    phrases:\n      en:\n        win: [\"gg {name}\"]\n");
        assert!(
            e.contains("bots[0].phrases.en.win[0]") && e.contains("which takes {map}"),
            "{e}"
        );
        assert!(err("bots:\n  - name: a\n    ru:\n      win: [gg]\n").contains("ru"));
        assert!(err("phrases: {}\nreplace: true\n").contains("replace"));
        assert!(ChatPhrasesFile::parse("schema: lambdabots/chat-phrases@2\n", "p").is_err());
    }
}

#[cfg(test)]
mod shipped {
    use super::*;

    #[test]
    fn shipped_phrases_cover_every_moment() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/config/chat/phrases.yaml");
        let text = std::fs::read_to_string(path).expect("data/config/chat/phrases.yaml");
        let f = ChatPhrasesFile::parse(&text, path).unwrap();
        for lang in ["ru", "en"] {
            let book = f.phrases.get(lang).unwrap_or_else(|| panic!("no `{lang}` phrases"));
            for m in Moment::ALL {
                let n = book.get(&m).map_or(0, Vec::len);
                assert!(n >= 8, "{lang}.{}: {n} phrases, at least 8 wanted", m.key());
            }
        }
        let banned = ["чит", "мяс", "жертв", "нуб", "cheat", "noob", "aimbot"];
        let books = std::iter::once(&f.phrases).chain(f.bots.iter().map(|b| &b.phrases));
        for (lang, moments) in books.flatten() {
            for (m, list) in moments {
                for p in list {
                    let lower = p.to_lowercase();
                    assert!(!banned.iter().any(|b| lower.contains(b)), "{lang}.{}: {p}", m.key());
                }
            }
        }
    }
}
