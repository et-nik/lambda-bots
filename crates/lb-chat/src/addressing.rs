//! Whether a chat line speaks to a bot: its name as players shorten it (`Атлас` for `DUT9 ATLASA`), in either
//! script, or the bots as a whole; and what the line is: noise, a question, a greeting, a repeat, a touchy one.
//! Characters are read by hand: there is no regex here.

use crate::profanity;

/// Words that name nobody in particular, even when they are part of a nickname.
const COMMON: [&str; 27] = [
    "bot", "bots", "bota", "boty", "player", "igrok", "noob", "nub", "pro", "the", "gg", "lol", "hl", "mr", "king",
    "best", "crowbar", "gauss", "egon", "glock", "python", "shotgun", "crossbow", "rpg", "satchel", "snark", "hornet",
];

/// Everyday words, in Latin letters, that a nickname may hold but a line rarely means as a name.
const PLAIN: [&str; 64] = [
    "what", "that", "this", "with", "your", "from", "have", "just", "like", "they", "them", "then", "than", "when",
    "where", "there", "here", "sorry", "please", "thanks", "good", "nice", "well", "very", "some", "come", "know",
    "only", "about", "back", "want", "need", "make", "more", "much", "time", "game", "play", "kill", "dead", "love",
    "life", "mother", "friend", "team", "hello", "gamer", "killer", "master", "boss", "dark", "death", "privet",
    "poka", "tozhe", "prosto", "tolko", "kogda", "seychas", "pochemu", "skolko", "ochen", "mozhno", "nado",
];

/// What follows `bot` in a word for a bot: `боты`, `ботом`, `ботяра`, `botina`.
const BOT_ENDINGS: [&str; 16] = [
    "", "s", "z", "y", "a", "u", "e", "om", "ov", "am", "ami", "ah", "yara", "ik", "iki", "ina",
];
/// The endings of a bot word that can be the one spoken to: `бот`, `боты`, `ботяра`.
const BOT_NOMINATIVE: [&str; 7] = ["", "s", "z", "y", "yara", "ik", "iki"];
/// "You" as the subject, as written: Cyrillic `у` is not `u`.
const YOU: [&str; 8] = ["ты", "вы", "ти", "ty", "ti", "vy", "you", "u"];
/// Words to step over between "you" and a bot word: `are you a bot`.
const ARTICLES: [&str; 3] = ["a", "an", "the"];
/// Words that call out in front of a bot word: `эй боты`.
const HEY: [&str; 4] = ["эй", "hey", "ey", "yo"];

/// Every form of "you", as written.
#[rustfmt::skip]
const SECOND_PERSON: [&str; 43] = [
    "ты", "тебя", "тебе", "тобой", "тя", "твой", "твоя", "твоё", "твое", "твои", "твоего", "твоей", "твоим", "твою",
    "вы", "вас", "вам", "вами", "ваш", "ваша", "ваше", "ваши", "вашего", "вашей", "ти", "тобі", "ty", "ti", "tebya",
    "tebe", "teb9", "teba", "tvoi", "tvoy", "tvoj", "vy", "vas", "vam", "you", "u", "ur", "your", "yours",
];

/// What a question starts with. Not `как`, `че` or `какой`: they start exclamations as often (`как повезло`).
#[rustfmt::skip]
const ASKING: [&str; 22] = [
    "кто", "что", "где", "куда", "откуда", "почему", "зачем", "сколько", "когда", "who", "what", "why", "how",
    "where", "when", "kto", "chto", "4to", "gde", "otkuda", "pochemu", "zachem",
];

/// Hellos, repeated letters squeezed.
#[rustfmt::skip]
const HELLOS: [&str; 22] = [
    "привет", "прив", "приветик", "приветики", "приветствую", "здаров", "здарова", "здорова", "здравствуйте",
    "здрасте", "здрасьте", "ку", "хай", "салам", "салют", "hi", "hello", "hey", "yo", "privet", "priv", "hallo",
];
/// Who a hello goes to: `привет всем`, `hi all`.
#[rustfmt::skip]
const EVERYONE: [&str; 16] = [
    "всем", "все", "всех", "народ", "ребят", "ребята", "пацаны", "парни", "all", "everyone", "everybody", "guys",
    "people", "ppl", "vsem", "narod",
];

/// Roots of nations and politics, a word's start: lines the bots stay out of.
#[rustfmt::skip]
const TOUCHY_ROOTS: [&str; 16] = [
    "росси", "рашк", "украин", "хохл", "беларус", "бульбаш", "москал", "киев", "путин", "зеленск", "войн", "russia",
    "ukrain", "ukran", "belarus", "putin",
];
/// Nations and politics as whole words.
const TOUCHY_WORDS: [&str; 2] = ["нато", "nato"];

/// A word of a line.
struct Word<'a> {
    /// As written, in lower case.
    raw: String,
    /// In Latin letters.
    lat: String,
    /// What follows it up to the next word.
    gap: &'a str,
}

/// The words of `text`: runs of letters and digits.
fn split(text: &str) -> Vec<Word<'_>> {
    let mut out = Vec::new();
    let mut rest = text.trim_start_matches(|c: char| !c.is_alphanumeric());
    while !rest.is_empty() {
        let end = rest.find(|c: char| !c.is_alphanumeric()).unwrap_or(rest.len());
        let (word, tail) = rest.split_at(end);
        let next = tail.find(char::is_alphanumeric).unwrap_or(tail.len());
        let raw = word.to_lowercase();
        out.push(Word {
            lat: latin(&raw),
            raw,
            gap: &tail[..next],
        });
        rest = &tail[next..];
    }
    out
}

/// Lower-case words of `text` written in Latin letters.
fn words(text: &str) -> Vec<String> {
    split(text).into_iter().map(|w| w.lat).collect()
}

/// Cyrillic transliterated to Latin; everything else as it is.
pub(crate) fn latin(word: &str) -> String {
    let mut out = String::with_capacity(word.len());
    for c in word.chars() {
        let t = match c {
            'а' => "a",
            'б' => "b",
            'в' => "v",
            'г' => "g",
            'д' => "d",
            'е' | 'ё' | 'э' => "e",
            'ж' => "zh",
            'з' => "z",
            'и' | 'і' => "i",
            'й' | 'ы' => "y",
            'к' => "k",
            'л' => "l",
            'м' => "m",
            'н' => "n",
            'о' => "o",
            'п' => "p",
            'р' => "r",
            'с' => "s",
            'т' => "t",
            'у' => "u",
            'ф' => "f",
            'х' => "h",
            'ц' => "ts",
            'ч' => "ch",
            'ш' | 'щ' => "sh",
            'ъ' | 'ь' => "",
            'ю' => "yu",
            'я' => "ya",
            _ => {
                out.push(c);
                continue;
            }
        };
        out.push_str(t);
    }
    out
}

/// `word` with each run of one character written once: `сууука` → `сука`.
pub(crate) fn squeeze(word: &str) -> String {
    let mut out = String::with_capacity(word.len());
    for c in word.chars() {
        if !out.ends_with(c) {
            out.push(c);
        }
    }
    out
}

/// The words of `name` players would call its owner by, each as written and in Latin letters: clan tags (`[TAG]`,
/// `|TAG|`, `(TAG)`) left out unless nothing else is left, three letters at least, common words skipped.
fn name_pairs(name: &str) -> Vec<(String, String)> {
    let mut bare = String::new();
    let mut depth = 0i32;
    let mut pipes = 0;
    for c in name.chars() {
        match c {
            '[' | '(' | '{' | '<' => depth += 1,
            ']' | ')' | '}' | '>' => depth = (depth - 1).max(0),
            '|' => pipes += 1,
            _ if depth == 0 && pipes % 2 == 0 => bare.push(c),
            _ => {}
        }
    }
    let pairs = |text: &str| -> Vec<(String, String)> {
        split(text)
            .into_iter()
            .map(|w| (w.raw, w.lat))
            .filter(|(_, w)| w.chars().count() >= 3 && !COMMON.contains(&w.as_str()))
            .collect()
    };
    let found = pairs(&bare);
    if found.is_empty() { pairs(name) } else { found }
}

/// The words of `name` players would call its owner by: clan tags (`[TAG]`, `|TAG|`, `(TAG)`) left out unless
/// nothing else is left, three letters at least, common words skipped.
pub fn name_words(name: &str) -> Vec<String> {
    name_pairs(name).into_iter().map(|(_, w)| w).collect()
}

fn similar(word: &str, token: &str) -> bool {
    if word == token {
        return true;
    }
    let (w, t) = (word.chars().count(), token.chars().count());
    if w.min(t) < 4 {
        return false;
    }
    let common = word.chars().zip(token.chars()).take_while(|(a, b)| a == b).count();
    common >= 4.max(w.min(t).saturating_sub(2)) || (common >= 6 && w.min(t) >= 7)
}

/// Whether `text` calls the player named `name`: one of its words, or a word that starts the same way (`Атлас`,
/// `атласу` for `ATLASA`; `klein` for `Kleiner`; `плутоныч` for `Plutonium`).
pub fn mentions(text: &str, name: &str) -> bool {
    let tokens = name_words(name);
    if tokens.is_empty() {
        return false;
    }
    let full: String = words(name).concat();
    words(text)
        .iter()
        .any(|w| (full.chars().count() >= 3 && *w == full) || tokens.iter().any(|t| similar(w, t)))
}

/// The words of `name` that a line uses for nothing but the player: four letters at least, no common, everyday or
/// swear word.
fn strict_words(name: &str) -> Vec<String> {
    name_pairs(name)
        .into_iter()
        .filter(|(raw, lat)| {
            let letters: String = raw.chars().filter(|c| c.is_alphabetic()).collect();
            letters.chars().count() >= 4
                && !PLAIN.contains(&lat.as_str())
                && !profanity::has(&letters, &[])
                && !profanity::slur(&letters, &[])
        })
        .map(|(_, lat)| lat)
        .collect()
}

/// Whether `word` of a line is `token`, a word of a nickname, the way a line declines or shortens it: `лепса` for
/// `leps`, `кошку` for `koshka`, `профиль` for `profile1`; not `легенда` for `leger`.
fn calls(word: &str, token: &str) -> bool {
    let (w, t) = (word.chars().count(), token.chars().count());
    let stem = match token.strip_suffix(['a', 'e', 'i', 'o', 'u', 'y']) {
        Some(stem) if stem.chars().count() >= 4 => stem,
        _ => token,
    };
    let common = word.chars().zip(token.chars()).take_while(|(a, b)| a == b).count();
    word == token
        || (w >= 4 && token.starts_with(word))
        || (word.starts_with(stem) && w <= t + 3)
        || (common >= 6 && w.min(t) >= 7)
}

/// Whether `text` calls the human `name`, strictly: by a word of the nickname that means nothing else (`leps` for
/// `leps`, not `mother` for `FUCK_YOU_MOTHER_player`).
pub fn names_other(text: &str, name: &str) -> bool {
    let tokens = strict_words(name);
    !tokens.is_empty() && words(text).iter().any(|w| tokens.iter().any(|t| calls(w, t)))
}

/// Whether `text` starts or ends calling the human `name` (as [`names_other`] does): `KOZA го`, `го на рельсы, koza`.
pub fn vocative(text: &str, name: &str) -> bool {
    let tokens = strict_words(name);
    let words = words(text);
    let named = |w: Option<&String>| w.is_some_and(|w| tokens.iter().any(|t| calls(w, t)));
    named(words.first()) || named(words.last())
}

/// Whether a word in Latin letters is one for a bot, and if so whether it can be the one spoken to.
fn bot_word(lat: &str) -> Option<bool> {
    let rest = lat.strip_prefix("bot")?;
    BOT_ENDINGS.contains(&rest).then(|| BOT_NOMINATIVE.contains(&rest))
}

/// How a line speaks of the bots, if at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BotsTalk {
    None,
    /// Of them, to somebody else: `ты че с ботом разговариваешь`, `eto bot?`.
    About,
    /// To them: `боты, вы где`, `ты бот?`, `эй боты`.
    To,
}

/// How `text` speaks of the bots. To them when a bot word in the nominative is the whole line, is called out
/// (first and followed by `,` `!` `?` or `ау`, last after a comma, or after `эй`), or stands next to "you"
/// (`ti bot?`, `are you a bot`); of them otherwise.
pub fn bots_talk(text: &str) -> BotsTalk {
    let words = split(text);
    let kinds: Vec<Option<bool>> = words.iter().map(|w| bot_word(&w.lat)).collect();
    if kinds.iter().all(Option::is_none) {
        return BotsTalk::None;
    }
    let called = |i: usize| kinds[i] == Some(true);
    let last = words.len() - 1;
    let first_called = called(0)
        && (words.len() == 1 || words[0].gap.contains([',', '!', '?']) || words[1].raw == "ау" || words[1].lat == "au");
    let last_called = last > 0 && called(last) && words[last - 1].gap.contains(',');
    let hey = (1..words.len()).any(|i| called(i) && HEY.contains(&words[i - 1].raw.as_str()));
    let you = |i: usize, step: isize| {
        let mut j = i.checked_add_signed(step);
        while let Some(k) = j.filter(|&k| k < words.len() && ARTICLES.contains(&words[k].raw.as_str())) {
            j = k.checked_add_signed(step);
        }
        j.is_some_and(|k| k < words.len() && YOU.contains(&words[k].raw.as_str()))
    };
    let next_to_you = (0..words.len()).any(|i| called(i) && (you(i, -1) || you(i, 1)));
    if first_called || last_called || hey || next_to_you {
        BotsTalk::To
    } else {
        BotsTalk::About
    }
}

/// Whether `text` speaks to someone as "you": `ты`, `тебя`, `вы`, `you`, `teb9`. On the words as written: Cyrillic
/// `у` (`у него`) is not `u`.
pub fn second_person(text: &str) -> bool {
    split(text).iter().any(|w| SECOND_PERSON.contains(&w.raw.as_str()))
}

/// The length of a smiley with a letter in it at `c[i..]` (`:D`, `;-PP`, `xDD`, `o_O`, `T_T`, `\o/`), or 0.
fn smiley_at(c: &[char], i: usize) -> usize {
    let at = |j: usize| c.get(j).copied().unwrap_or(' ');
    let eye = |ch: char| matches!(ch, 'o' | 'O' | '0' | 'о' | 'О');
    let run = |from: usize| from + c[from..].iter().take_while(|&&ch| ch == c[from]).count();
    let end = if matches!(at(i), ':' | ';' | '=') {
        let mouth = if at(i + 1) == '-' { i + 2 } else { i + 1 };
        if !matches!(at(mouth), 'D' | 'd' | 'P' | 'p' | 'O' | 'o' | 'З' | 'з' | 'Р' | 'р') {
            return 0;
        }
        run(mouth)
    } else if i.checked_sub(1).is_some_and(|p| c[p].is_alphabetic()) {
        return 0;
    } else if matches!(at(i), 'x' | 'X' | 'х' | 'Х') && matches!(at(i + 1), 'D' | 'd' | 'Д' | 'д') {
        run(i + 1)
    } else if (eye(at(i)) && matches!(at(i + 1), '_' | '.') && eye(at(i + 2)))
        || (matches!(at(i), 'T' | 'Т') && at(i + 1) == '_' && matches!(at(i + 2), 'T' | 'Т'))
        || (matches!(at(i), '\\' | '_') && matches!(at(i + 1), 'o' | 'O') && matches!(at(i + 2), '/' | '_'))
    {
        i + 3
    } else {
        return 0;
    };
    if at(end).is_alphabetic() { 0 } else { end - i }
}

/// `text` with its smileys blanked out: `:D`, `xD`, `o_O`, the shrug's `ツ` and the lenny face's `ʖ`.
fn unsmiled(text: &str) -> String {
    let c: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < c.len() {
        match smiley_at(&c, i) {
            0 => {
                out.push(if matches!(c[i], 'ツ' | 'ʖ') { ' ' } else { c[i] });
                i += 1;
            }
            n => {
                out.push(' ');
                i += n;
            }
        }
    }
    out
}

/// The words of `text` that have a letter: what is between spaces, punctuation and all.
fn lettered(text: &str) -> Vec<&str> {
    text.split_whitespace()
        .filter(|w| w.chars().any(char::is_alphabetic))
        .collect()
}

/// What a line is when it is nothing anyone would answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Noise {
    /// A plugin's macro the game did not fill in: `recharging @ %l`.
    Macro,
    /// Signs, digits and smileys, `?` alone too: `)))`, `100`, `????`, `¯\_(ツ)_/¯`.
    Symbols,
    /// Laughter: `ахахах`, `looool`, `хехе`, `jajaja`.
    Laugh,
    /// A map's name: `gg_cold_rock`, `dead-dust2`.
    Map,
    /// A key bind's shout: `DIIIIIEEEEE!!!!1`, `FUCK YOU!`, `WHAT?! o_0`.
    Bind,
    /// Four characters at most, without `?`: `да`, `n1ce`, `x)`.
    Short,
    /// One word without `?`: `спасибо`, `agstart`.
    OneWord,
}

impl Noise {
    /// Never worth an answer; `Short` and `OneWord` may still be a reply to a bot that spoke.
    pub fn hard(self) -> bool {
        !matches!(self, Noise::Short | Noise::OneWord)
    }
}

/// Whether `word` (letters only, lower case) is laughter: `lol`, `kek`, `xd`, `ору`, `азаз`, or `ха`, `хе`, `ja`
/// and the like over and over.
fn laughter(word: &str) -> bool {
    const LAUGHS: [&str; 9] = ["lol", "kek", "кек", "xd", "хд", "lmao", "lmfao", "rofl", "ору"];
    const LETTERS: &str = "ахеиыъэaheijxzgfu";
    let s = squeeze(word);
    // `lolol`, `азазаза`: a unit twice or more, then maybe its first letter.
    let repeats = |unit: &str, tail: &str| {
        let body = s.strip_suffix(tail).unwrap_or(&s);
        body.len() >= 2 * unit.len() && body.as_bytes().chunks(unit.len()).all(|c| c == unit.as_bytes())
    };
    LAUGHS.contains(&s.as_str())
        || repeats("lo", "l")
        || repeats("аз", "а")
        || (word.chars().count() >= 3
            && word.chars().all(|c| LETTERS.contains(c))
            && word.chars().filter(|c| matches!(c, 'х' | 'h' | 'x' | 'j')).count() >= 2
            && word.chars().any(|c| "аеиыэaeiu".contains(c)))
}

/// Whether `text` (one token, no spaces) is a map's name: letters and digits joined by `_` or `-`.
fn map_name(text: &str) -> bool {
    let parts: Vec<&str> = text.split(['_', '-']).collect();
    text.is_ascii()
        && parts.len() >= 2
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric()))
        && text.chars().any(|c| c.is_ascii_alphabetic())
}

/// Whether `bare` (smileys out) is a bind's shout: three letters or more, all Latin capitals, a `!` or a trailing
/// `~`, six words at most (`B I T C H` is one).
fn shout(bare: &str) -> bool {
    let letters: Vec<char> = bare.chars().filter(|c| c.is_alphabetic()).collect();
    let mut words = 0;
    let mut spelt = false;
    for w in lettered(bare) {
        let single = w.chars().filter(|c| c.is_alphanumeric()).count() == 1;
        if !(single && spelt) {
            words += 1;
        }
        spelt = single;
    }
    letters.len() >= 3
        && letters.iter().all(char::is_ascii_uppercase)
        && (bare.contains('!') || bare.trim_end().ends_with('~'))
        && words <= 6
}

/// What noise `text` is, if it is: nothing anyone would answer ([`Noise::hard`]), or a line too short to answer
/// unless it replies to a bot.
pub fn noise(text: &str) -> Option<Noise> {
    let text = text.trim();
    let bare = unsmiled(text);
    let words = lettered(&bare);
    let letters = |w: &str| {
        w.chars()
            .filter(|c| c.is_alphabetic())
            .flat_map(char::to_lowercase)
            .collect::<String>()
    };
    if text
        .as_bytes()
        .windows(2)
        .any(|p| p[0] == b'%' && p[1].is_ascii_alphabetic())
    {
        Some(Noise::Macro)
    } else if words.is_empty() {
        Some(Noise::Symbols)
    } else if shout(&bare) {
        Some(Noise::Bind)
    } else if words.iter().all(|w| laughter(&letters(w))) {
        Some(Noise::Laugh)
    } else if !text.contains(char::is_whitespace) && map_name(text) {
        Some(Noise::Map)
    } else if text.contains('?') {
        None
    } else if text.chars().count() <= 4 {
        Some(Noise::Short)
    } else if bare
        .split_whitespace()
        .filter(|w| w.chars().any(char::is_alphanumeric))
        .count()
        == 1
    {
        Some(Noise::OneWord)
    } else {
        None
    }
}

/// Whether `text` asks something: a `?` with two words or six letters (inside a talk, two letters will do), or a
/// question word first (`где все`, `what map`, `ты где`).
pub fn question(text: &str, in_talk: bool) -> bool {
    let bare = unsmiled(text);
    if bare.contains('?') {
        let letters = bare.chars().filter(|c| c.is_alphabetic()).count();
        return if in_talk {
            letters >= 2
        } else {
            letters >= 6 || lettered(&bare).len() >= 2
        };
    }
    let words: Vec<String> = lettered(&bare)
        .into_iter()
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase())
        .collect();
    let asks = |w: Option<&String>| w.is_some_and(|w| ASKING.contains(&w.as_str()));
    words.len() >= 2 && (asks(words.first()) || (YOU.contains(&words[0].as_str()) && asks(words.get(1))))
}

/// Whether `text` only says hello to everyone: up to three words, all hellos or `всем`, `all`, `народ`, bots.
pub fn greeting(text: &str) -> bool {
    let words = split(text);
    let known = |list: &[&str], w: &str| list.iter().any(|h| squeeze(h) == w);
    let hello = |w: &Word<'_>| known(&HELLOS, &squeeze(&w.raw));
    let everyone = |w: &Word<'_>| known(&EVERYONE, &squeeze(&w.raw)) || bot_word(&w.lat) == Some(true);
    (1..=3).contains(&words.len()) && words.iter().any(hello) && words.iter().all(|w| hello(w) || everyone(w))
}

/// What `text` says, to tell a repeat: its words in Latin letters, letters run together squeezed, sorted, once each;
/// without the words `skip` (our bots' names as [`name_words`] gives them) or words like them.
pub fn gist(text: &str, skip: &[String]) -> Vec<String> {
    let mut out: Vec<String> = words(text)
        .into_iter()
        .filter(|w| !skip.iter().any(|s| similar(w, s)))
        .map(|w| squeeze(&w))
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// Whether two [`gist`]s say the same: equal, or three quarters of their words shared when both have four or more.
/// Lines with no words are never the same.
pub fn same_gist(a: &[String], b: &[String]) -> bool {
    if a.is_empty() || b.is_empty() {
        return false;
    }
    if a.len() < 4 || b.len() < 4 {
        return a == b;
    }
    let shared = a.iter().filter(|w| b.binary_search(w).is_ok()).count();
    shared * 4 >= (a.len() + b.len() - shared) * 3
}

/// Whether `text` is a line to stay out of: swearing, a slur, nations or politics. `names`: nicknames and aliases,
/// left out first.
pub fn touchy(text: &str, names: &[String]) -> bool {
    profanity::has(text, names)
        || profanity::slur(text, names)
        || profanity::words(text, names)
            .iter()
            .any(|w| TOUCHY_WORDS.contains(&w.as_str()) || TOUCHY_ROOTS.iter().any(|r| w.starts_with(r)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_words_skip_tags_and_noise() {
        assert_eq!(name_words("DUT9 ATLASA"), vec!["dut9", "atlasa"]);
        assert_eq!(name_words("[BOT] Kleiner"), vec!["kleiner"]);
        assert_eq!(name_words("-|NoS|-"), vec!["nos"]);
        assert_eq!(name_words("Pro Player"), Vec::<String>::new());
        assert_eq!(name_words("Ковбой"), vec!["kovboy"]);
        assert_eq!(name_words("Gauss Master"), vec!["master"]);
    }

    #[test]
    fn mentions_by_shortened_and_declined_names() {
        assert!(mentions("Дитя Атласа, ты в своём уме?", "DUT9 ATLASA"));
        assert!(mentions("атлас го 1на1", "DUT9 ATLASA"));
        assert!(mentions("klein you camper", "[BOT] Kleiner"));
        assert!(mentions("кляйнеру привет", "Кляйнер"));
        assert!(mentions("nos gg", "-|NoS|-"));
        assert!(mentions("ковбою респект", "Ковбой"));
        assert!(mentions("плутоныч а как у тебя пинг 0", "Plutonium"));
        assert!(!mentions("нос чешется", "Kleiner"));
        assert!(!mentions("and then", "Andrey"));
        assert!(!mentions("gg", "-|NoS|-"));
        assert!(!mentions("player one", "Pro Player"));
        assert!(!mentions("плутовка", "Plutonium"));
        assert!(!mentions("гаусс имба", "Gauss"));
    }

    #[test]
    fn other_humans_are_named_strictly() {
        assert!(names_other("лепс, го дуэль", "leps"));
        assert!(names_other("KOZA го на рельсы", "KOZA"));
        assert!(names_other("а лепса кто убил", "leps"));
        assert!(names_other("дай кошку", "=Кошка="));
        assert!(names_other("профиль хврэ", "Profile1"));
        assert!(names_other("педик ку", "pedik228"));
        assert!(!names_other("плутониум легенда", "Leger"));
        assert!(!names_other("козел", "KOZA"));
        assert!(!names_other("fuck you mother", "FUCK_YOU_MOTHER_player"));
        assert!(!names_other("Sorry for what?", "Sorry for what?"));
        assert!(
            !names_other("фик так ты тоже же бот", "FIK"),
            "three letters are not enough"
        );
        assert!(!names_other("ну и игрок", "Player"));
        assert!(!names_other("сука", "suka228"));
        assert!(vocative("KOZA го на рельсы", "KOZA"));
        assert!(vocative("го на рельсы, коза", "KOZA"));
        assert!(!vocative("а лепс где был", "leps"));
        assert!(!vocative("ну что", "what"));
    }

    #[test]
    fn bots_spoken_to_or_of() {
        for to in [
            "ti bot?",
            "ты бот?",
            "вы боты?",
            "боты, вы где",
            "ботяра",
            "бот",
            "эй боты, го",
            "бот, ты где?",
            "боты ау",
            "are you a bot?",
            "r u bot?",
            "you bot?",
            "го на рельсы, боты",
        ] {
            assert_eq!(bots_talk(to), BotsTalk::To, "{to}");
        }
        for about in [
            "ТЫ ЧЕ С БОТОМ РАЗГОВАРИВАЕШЬ",
            "давно тут этот бот?",
            "тут все только боты",
            "не только боты",
            "фик так ты тоже же бот",
            "вас бот уебал",
            "che tam tvoi drug bot",
            "i think he is something else... i didnt see like such a bot",
            "botina opyatb proebal",
            "eto bot?",
            "eto bot)",
            "я с ботами играю?",
            "u bota aim",
            "у бота аим",
            "tupoi bot",
            "боты тупые",
        ] {
            assert_eq!(bots_talk(about), BotsTalk::About, "{about}");
        }
        for none in ["bottle", "привет всем", "fucking aimbot cheater", "ботинки", "ботан"] {
            assert_eq!(bots_talk(none), BotsTalk::None, "{none}");
        }
    }

    #[test]
    fn noise_kinds() {
        let cases: [(&str, Option<Noise>); 49] = [
            ("recharging @ %l", Some(Noise::Macro)),
            (")))", Some(Noise::Symbols)),
            ("+", Some(Noise::Symbols)),
            (".!.", Some(Noise::Symbols)),
            ("100", Some(Noise::Symbols)),
            ("100?%", Some(Noise::Symbols)),
            ("5 5 5 5 5", Some(Noise::Symbols)),
            (": ¯\\_(ツ)_/¯", Some(Noise::Symbols)),
            ("( ͡° ͜ʖ ͡°)", Some(Noise::Symbols)),
            ("XD", Some(Noise::Symbols)),
            ("????", Some(Noise::Symbols)),
            ("?", Some(Noise::Symbols)),
            ("$$$$$_$$$$$", Some(Noise::Symbols)),
            ("ахахаах", Some(Noise::Laugh)),
            ("HAHAHAHAHHAHAHHAH", Some(Noise::Laugh)),
            ("lololololo", Some(Noise::Laugh)),
            ("looooooooooooooool", Some(Noise::Laugh)),
            ("хехехе", Some(Noise::Laugh)),
            ("jajaja", Some(Noise::Laugh)),
            ("ehuehueuhe", Some(Noise::Laugh)),
            ("axax :D", Some(Noise::Laugh)),
            ("LOL :)", Some(Noise::Laugh)),
            ("hihi haha", Some(Noise::Laugh)),
            ("аъахаха", Some(Noise::Laugh)),
            ("азазаз", Some(Noise::Laugh)),
            ("afafhahaha", Some(Noise::Laugh)),
            ("gg_cold_rock", Some(Noise::Map)),
            ("agg_Cold_rock", Some(Noise::Map)),
            ("gg_octagon_v2", Some(Noise::Map)),
            ("dead-dust2", Some(Noise::Map)),
            ("1hp_crazy_rooms_beta5", Some(Noise::Map)),
            ("DIIIIIIEEEEE!!!!1", Some(Noise::Bind)),
            ("GTFO!!!)", Some(Noise::Bind)),
            ("FUCK YOU!", Some(Noise::Bind)),
            (">:O !!! RAZIBU BLYD!!!1", Some(Noise::Bind)),
            ("FFFFFFFFFUUUUUUUU~", Some(Noise::Bind)),
            ("WHAT?! o_0", Some(Noise::Bind)),
            ("[  !!! KILL THAT  B I T C H !!!", Some(Noise::Bind)),
            ("n1ce", Some(Noise::Short)),
            ("чмо", Some(Noise::Short)),
            ("x)", Some(Noise::Short)),
            ("Laramie", Some(Noise::OneWord)),
            ("СДЕЛАЕМ", Some(Noise::OneWord)),
            ("где?", None),
            ("ГДЕ КАРТА АРЕНА ?", None),
            ("ЛИЧНО ДЛЯ ТЕБЯ)", None),
            ("100% читер", None),
            ("A TO)", None),
            ("ну и в целом.", None),
        ];
        for (text, kind) in cases {
            assert_eq!(noise(text), kind, "{text}");
        }
        assert_eq!(noise("ПИТОНИУМ ГОВОРИТ )))))"), None);
        assert_eq!(noise("я так и не смог гранату между колоннами кинуть"), None);
        assert_eq!(noise("Ты свою уже приготовил?"), None);
        assert!(Noise::Bind.hard() && !Noise::Short.hard() && !Noise::OneWord.hard());
    }

    #[test]
    fn questions() {
        for q in [
            "кто лидер?",
            "почему?",
            "Опять боты одни?",
            "кто не бот ?",
            "Ты свою уже приготовил?",
            "привет, чтог такое сачель?",
            "есть здесь бхоп?",
            "Why I start in 1 of healt?",
            "Чё, живые чтоль?",
            "как ты меня видел ?",
            "what is so funny",
            "why killbox",
            "ты где",
            "Что не в курсек,",
        ] {
            assert!(question(q, false), "{q}");
        }
        for not in [
            "где?",
            "?",
            "????",
            "WHAT?! o_0",
            "hohol?",
            "как повезло",
            "kak zaebal bot",
            "че ты мне чешешь",
            "что",
            "кто-то тут кемпит",
        ] {
            assert!(!question(not, false), "{not}");
        }
        assert!(question("где?", true) && !question("?", true) && !question("о?", true));
        assert!(second_person("Ты свою уже приготовил?"));
        assert!(second_person("ti bot?"));
        assert!(second_person("plutonium y teb9 4it est?"));
        assert!(second_person("вы ждали меня дети ?"));
        assert!(!second_person("у него бот"));
        assert!(!second_person("кто лидер?"));
    }

    #[test]
    fn greetings() {
        for hi in [
            "прив всем",
            "Hi all !",
            "hi everyone",
            "привет",
            "ку",
            "здарова)",
            "приветтт народ",
            "hello bots",
        ] {
            assert!(greeting(hi), "{hi}");
        }
        for not in [
            "привет Плутон!",
            "привет, как дела у всех",
            "всем гг",
            "hi hi hi hi",
            "",
            "hello world",
        ] {
            assert!(!greeting(not), "{not}");
        }
    }

    #[test]
    fn repeats_by_gist() {
        let skip = name_words("Plutonium");
        let a = gist("plutonium, а есть здесь скин \"Стим и не ебёт?", &skip);
        let b = gist("Plutonium, а есть здесь скин «Стим и не ебёт»??", &skip);
        assert_eq!(a, b);
        assert!(same_gist(&a, &b));
        let named = gist("плутон, а есть здесь скин", &skip);
        assert!(
            same_gist(&named, &gist("а есть здесь скин", &[])),
            "a name is not the gist"
        );
        assert!(same_gist(
            &gist("DIIIIIIEEEEE!!!!1", &[]),
            &gist("DIIIIEEEEEEEE!!1", &[])
        ));
        assert!(same_gist(
            &gist("кто тут лидер сейчас", &[]),
            &gist("кто тут лидер сейчас вообще", &[])
        ));
        assert!(!same_gist(
            &gist("а есть здесь скин", &[]),
            &gist("а есть здесь бхоп", &[])
        ));
        assert!(!same_gist(&gist("???", &[]), &gist("!!!", &[])));
    }

    #[test]
    fn property_any_line_is_safe() {
        let mut rng = lb_core::rng::Pcg32::new(7, 3);
        let alphabet: Vec<char> = "aZoxD0_ :;=-%?!~[]()<>^2'’`İıßёібЕБツʖ\u{301}🔥\u{a0}ботпиздаеб"
            .chars()
            .collect();
        for _ in 0..3000 {
            let len = rng.range_i32(0, 40) as usize;
            let text: String = (0..len)
                .map(|_| alphabet[rng.range_i32(0, alphabet.len() as i32 - 1) as usize])
                .collect();
            let names = vec![text.chars().take(3).collect::<String>()];
            noise(&text);
            bots_talk(&text);
            question(&text, true);
            question(&text, false);
            greeting(&text);
            second_person(&text);
            touchy(&text, &names);
            mentions(&text, &text);
            names_other(&text, &text);
            vocative(&text, &text);
            crate::lang::detect(&text);
            profanity::cheating(&text, &names);
            profanity::scrub(&text, &names);
            let g = gist(&text, &names);
            assert!(g.windows(2).all(|p| p[0] < p[1]), "{text:?}");
            assert_eq!(same_gist(&g, &g), !g.is_empty(), "{text:?}");
            let short = crate::aliases::short(&text);
            assert!(text.trim().is_empty() || !short.is_empty(), "{text:?}");
        }
    }

    #[test]
    fn touchy_lines() {
        assert!(touchy("плутониум, мэдкид пидорас ебаный", &[]));
        assert!(touchy("чурки лучше хохолов", &[]));
        assert!(touchy("а путин то", &[]));
        assert!(touchy("aaah belarus , the asslickers of russia?", &[]));
        assert!(touchy("все в НАТО", &[]));
        assert!(!touchy("ну и карта", &[]));
        assert!(!touchy("двойной фраг", &[]));
        assert!(!touchy("hohol.ua зашёл", &["hohol.ua".into()]));
    }
}
