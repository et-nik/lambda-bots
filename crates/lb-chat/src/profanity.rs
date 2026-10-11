//! Swearing, slurs and talk of cheats in a chat line: what a bot's line may not say, what the memory hides, which
//! lines the bots stay out of. Words are read the way players disguise them (`пiзда` with a Latin `i`, `XУЙ`,
//! `сууука`), and nicknames and aliases are left out first: `FUCK_THIS_GAME_noob` is a name, not swearing.
//! Characters are read by hand: there is no regex here.

use std::cmp::Reverse;

use crate::addressing::{latin, squeeze};
use crate::aliases::find_ci;
use crate::lang::cyrillic;

/// Swearing anywhere in a word: `нихуя`, `спиздил`, `motherfucker`.
#[rustfmt::skip]
const SWEAR_INSIDE: [&str; 17] = [
    "хуй", "хуе", "хуи", "хуя", "хую", "пизд", "пзд", "пидор", "пидар", "пидер", "пидр", "fuck", "shit", "bitch",
    "whore", "faggot", "nigg",
];
/// Swearing a word starts with: `мудак`, `шлюха`, `отсоси`, `zaebal`.
#[rustfmt::skip]
const SWEAR_START: [&str; 44] = [
    "муда", "мудил", "мудоз", "гандон", "гондон", "шлюх", "залуп", "сукин", "сучар", "блядск", "отсос", "отсас",
    "страпон", "клитор", "fck", "fuk", "cunt", "slut", "bastard", "twat", "wank", "pussy", "pizd", "xuy", "xui", "huy",
    "eban", "ebal", "ebat", "zaeb", "proeb", "uebal", "nahui", "nahuy", "mudak", "orospu", "siktir", "sikeyim",
    "asslick", "cocks", "pidor", "pidar", "pidr", "gandon",
];
/// Swearing as a whole word: `бля`, `сука`, `соси`, `stfu`.
#[rustfmt::skip]
const SWEAR_WORDS: [&str; 44] = [
    "бля", "блять", "блядь", "бляд", "блядина", "сука", "суки", "суку", "сукой", "сучка", "сучке", "сучку", "сюка",
    "соси", "пососи", "сосать", "сасать", "сосал", "сосала", "сосет", "сосешь", "сосут", "dick", "dicks",
    "dickhead", "cock", "ass", "asses", "asshole", "stfu", "gtfo", "hui", "huy", "blya", "blyat", "blyad", "blyd",
    "suka", "amk", "sosi", "soset", "sosat", "sosal", "sosesh",
];
/// What may stand in front of `еб` in a swear word: `заебал`, `долбоеб`. Not a bare `в` or `с` (`вебка`, `себя`).
#[rustfmt::skip]
const EB_PREFIXES: [&str; 21] = [
    "", "за", "на", "у", "вы", "от", "отъ", "про", "пере", "подъ", "съ", "сь", "раз", "разъ", "до", "по", "при", "въ",
    "взъ", "долбо", "долба",
];
/// What may follow `еб` in a swear word, the word's end besides.
const EB_NEXT: &str = "ауилнызоеяшчтькщю";
/// Words that hold a root and are clean: `страхуй`, `спидран`, `психуй`, `жидкий`, `хачапури`, `pedikur`.
#[rustfmt::skip]
const CLEAN: [&str; 12] = [
    "страху", "спидр", "скипидар", "ебол", "псих", "педикюр", "жидк", "нигери", "хачап", "pedikur", "pedikyur",
    "pedikjur",
];

/// Slurs anywhere in a word: `суперпидр`, `pidorasy`.
#[rustfmt::skip]
const SLUR_INSIDE: [&str; 9] = ["пидор", "пидар", "пидер", "пидр", "pidor", "pidar", "pidr", "nigg", "faggot"];
/// Slurs a word starts with: `хохлы`, `чурка`. Never `петух`: bots go by it (`>I<apeHbIu_neTyX`), and lines about
/// them say `по петуху`.
#[rustfmt::skip]
const SLUR_START: [&str; 16] = [
    "педик", "пидал", "чурк", "чурок", "хохол", "хохл", "укроп", "жид", "черножоп", "нигер", "ниггер", "хачик",
    "хачь", "hohol", "xoxol", "kike",
];
/// Slurs as whole words: `fag`, `хачи`.
#[rustfmt::skip]
const SLUR_WORDS: [&str; 12] = [
    "fag", "fags", "homo", "homos", "хач", "хача", "хачи", "хачей", "хачам", "хачами", "хачах", "хачом",
];
/// Slurs in Latin letters a word of a nickname starts with: `Piderok`, `pedik777`. For nicknames alone ([`nick`]):
/// a line keeps such a word as the name of the player it calls ([`crate::addressing::names_other`]).
const NICK_SLUR_START: [&str; 2] = ["pider", "pedik"];

/// Cheat words, whole, in Latin letters with digits read as letters (`4it` is `chit`): `чит`, `читы`, `вх`, `wh`.
#[rustfmt::skip]
const CHEAT_WORDS: [&str; 16] = [
    "chit", "chity", "chita", "chitu", "chitom", "chitov", "chitam", "chitami", "chitah", "chite", "vh", "wh", "hack",
    "hacks", "hax", "speedhack",
];
/// Cheat words by their start: `читер`, `читоры`, `cheating`, `aimbots`, `аимщик`.
#[rustfmt::skip]
const CHEAT_START: [&str; 8] = [
    "chiter", "chitak", "chitor", "cheat", "aimbot", "aimshik", "wallhack", "hacker",
];

/// Where whole `names` stand in `text`, case-insensitively: (start, end) byte offsets in the order they stand. Of
/// overlapping names the longest wins.
fn name_spans(text: &str, names: &[String]) -> Vec<(usize, usize)> {
    let mut names: Vec<&str> = names.iter().map(|n| n.trim()).filter(|n| !n.is_empty()).collect();
    names.sort_by_key(|n| Reverse(n.chars().count()));
    let word = |c: Option<char>| c.is_some_and(char::is_alphanumeric);
    let mut found: Vec<(usize, usize)> = Vec::new();
    for name in names {
        let mut from = 0;
        while let Some((start, end)) = find_ci(text, name, from) {
            from = start + text[start..].chars().next().map_or(1, char::len_utf8);
            let whole = !word(text[..start].chars().next_back()) && !word(text[end..].chars().next());
            if whole && !found.iter().any(|&(s, e)| s < end && start < e) {
                found.push((start, end));
            }
        }
    }
    found.sort_unstable();
    found
}

/// `text` with each whole one of the `names` in it (nicknames and aliases) written as a space.
pub(crate) fn without_names(text: &str, names: &[String]) -> String {
    let mut kept = String::with_capacity(text.len());
    let mut at = 0;
    for (start, end) in name_spans(text, names) {
        kept.push_str(&text[at..start]);
        kept.push(' ');
        at = end;
    }
    kept.push_str(&text[at..]);
    kept
}

/// The words of `text`, as byte ranges: letters, digits and apostrophes inside them.
fn spans(text: &str) -> Vec<(usize, usize)> {
    let apostrophe = |c: char| matches!(c, '\'' | '’' | 'ʼ' | '`');
    let mut out = Vec::new();
    let mut start = None;
    for (i, c) in text.char_indices().chain([(text.len(), ' ')]) {
        match start {
            None if c.is_alphanumeric() => start = Some(i),
            Some(s) if !c.is_alphanumeric() && !apostrophe(c) => {
                let word = text[s..i].trim_end_matches(apostrophe);
                out.push((s, s + word.len()));
                start = None;
            }
            _ => {}
        }
    }
    out
}

/// `word` as the lists spell it: lower case, `ё` as `е`, Ukrainian letters as Russian ones, and in a word with
/// Cyrillic, Latin and digit look-alikes as Cyrillic (`пiзда`, `XУЙ`, `3аебал`) and apostrophes as `ъ` (`в'єбав`).
/// Elsewhere apostrophes are dropped.
fn normal(word: &str) -> String {
    let lower = word.to_lowercase();
    let russian = lower.chars().any(cyrillic);
    lower
        .chars()
        .filter_map(|c| {
            Some(match c {
                'ё' | 'є' => 'е',
                'і' | 'ї' => 'и',
                'ґ' => 'г',
                '\'' | '’' | 'ʼ' | '`' if russian => 'ъ',
                '\'' | '’' | 'ʼ' | '`' => return None,
                _ if russian => look_alike(c),
                _ => c,
            })
        })
        .collect()
}

/// The Cyrillic letter a Latin letter or digit stands for in a Cyrillic word.
fn look_alike(c: char) -> char {
    match c {
        'a' => 'а',
        'c' => 'с',
        'e' => 'е',
        'i' => 'и',
        'k' => 'к',
        'm' => 'м',
        'o' | '0' => 'о',
        'p' => 'р',
        't' => 'т',
        'x' => 'х',
        'y' => 'у',
        'b' => 'в',
        'h' => 'н',
        '3' => 'з',
        '6' => 'б',
        _ => c,
    }
}

/// The words of `text` without the `names` in it (nicknames and aliases), as the lists spell them; three or more
/// letters in a row spelt apart (`S H I T`) make one word.
pub(crate) fn words(text: &str, names: &[String]) -> Vec<String> {
    let kept = without_names(text, names);
    let mut out = Vec::new();
    let mut spelt = Vec::new();
    for (s, e) in spans(&kept) {
        let word = &kept[s..e];
        if word.chars().count() == 1 {
            spelt.push(word);
        } else {
            unspell(&mut spelt, &mut out);
            out.push(normal(word));
        }
    }
    unspell(&mut spelt, &mut out);
    out
}

/// Letters spelt apart into `out`: each, and all of them as one word when there are three or more.
fn unspell(spelt: &mut Vec<&str>, out: &mut Vec<String>) {
    if spelt.len() >= 3 {
        out.push(normal(&spelt.concat()));
    }
    out.extend(spelt.drain(..).map(normal));
}

/// Whether `word` (as the lists spell it) or its squeezed form passes `test`; never a clean word.
fn either(word: &str, test: fn(&str) -> bool) -> bool {
    !CLEAN.iter().any(|c| word.contains(c)) && (test(word) || test(&squeeze(word)))
}

/// `еб` after a prefix and before a vowel, `л`, `н` … or the end: `заебал`, `долбоеб`, `ебля`; not `небо`, `хлеб`.
fn eb(word: &str) -> bool {
    EB_PREFIXES.iter().any(|p| {
        word.strip_prefix(p)
            .and_then(|r| r.strip_prefix("еб"))
            .is_some_and(|r| r.chars().next().is_none_or(|c| EB_NEXT.contains(c)))
    })
}

/// Whether a word (as the lists spell it) swears; `мсука`, one stray letter and `сука`, too.
fn swearing(word: &str) -> bool {
    let stray_suka = word.chars().count() == 5 && ["сука", "сюка"].iter().any(|s| word.ends_with(s));
    SWEAR_INSIDE.iter().any(|r| word.contains(r))
        || SWEAR_START.iter().any(|r| word.starts_with(r))
        || SWEAR_WORDS.contains(&word)
        || eb(word)
        || stray_suka
}

/// Whether a word (as the lists spell it) is a slur.
fn slurring(word: &str) -> bool {
    SLUR_INSIDE.iter().any(|r| word.contains(r))
        || SLUR_START.iter().any(|r| word.starts_with(r))
        || SLUR_WORDS.contains(&word)
}

/// Whether `text` swears (`бля`, `заебал`, `fuck`). `names`: nicknames and aliases, left out first.
pub fn has(text: &str, names: &[String]) -> bool {
    words(text, names).iter().any(|w| either(w, swearing))
}

/// Whether `text` holds a slur: nations, races, orientation (`хохлы`, `чурка`, `пидор`, `faggot`). `names`:
/// nicknames and aliases, left out first.
pub fn slur(text: &str, names: &[String]) -> bool {
    words(text, names).iter().any(|w| either(w, slurring))
}

/// `word` with the digits players write for letters read as those letters: `pid0r` is `pidor`, `h0h0l` is `hohol`.
fn leet(word: &str) -> String {
    word.chars()
        .map(|c| match c {
            '0' => 'o',
            '1' => 'i',
            '3' => 'e',
            '4' => 'a',
            _ => c,
        })
        .collect()
}

/// Whether a word (as the lists spell it) starts with a slur only nicknames are read for ([`NICK_SLUR_START`]).
fn nick_slurring(word: &str) -> bool {
    NICK_SLUR_START.iter().any(|r| word.starts_with(r))
}

/// The parts of `word` its capitals start (`xXPiderokXx`: `x`, `X`, `Piderok`, `Xx`): a capital after a small
/// letter or a digit starts one (`1337Pedik`: `1337`, `Pedik`), and so does a capital between a capital and a small
/// letter (`HLDMKing`: `HLDM`, `King`).
fn camel(word: &str) -> Vec<&str> {
    let chars: Vec<(usize, char)> = word.char_indices().collect();
    let mut parts = Vec::new();
    let mut start = 0;
    for (i, &(at, c)) in chars.iter().enumerate().skip(1) {
        let before = chars[i - 1].1;
        let after = chars.get(i + 1).map(|&(_, c)| c);
        if c.is_uppercase()
            && (before.is_lowercase()
                || before.is_ascii_digit()
                || (before.is_uppercase() && after.is_some_and(char::is_lowercase)))
        {
            parts.push(&word[start..at]);
            start = at;
        }
    }
    parts.push(&word[start..]);
    parts
}

/// Whether a nickname swears or holds a slur, so that no phrase says it. Each word of it, and each part of a word its
/// capitals start (`MegaPedik`, `5Pedik`), is read: it swears or is a slur as it is, without its digits (`suka1337`)
/// or with digits read as letters (`Pid0r_99`); or it starts with a Latin slur lines are not read for
/// (`xX_Piderok_Xx`), as it is or with digits read as letters, never without them (`5piderman` stays).
pub fn nick(name: &str) -> bool {
    let parts = spans(name)
        .into_iter()
        .flat_map(|(s, e)| camel(&name[s..e]))
        .map(normal);
    words(name, &[]).into_iter().chain(parts).any(|word| {
        let listed = |w: &str| either(w, swearing) || either(w, slurring);
        let digitless: String = word.chars().filter(|c| !c.is_ascii_digit()).collect();
        let lettered = leet(&word);
        [word.as_str(), lettered.as_str()]
            .into_iter()
            .any(|w| listed(w) || either(w, nick_slurring))
            || listed(&digitless)
    })
}

/// `word` in Latin letters with digits read as letters, the way cheat words are compared: `4it` → `chit`.
fn spoken(word: &str) -> String {
    let lat = latin(word);
    if lat.chars().any(char::is_alphabetic) {
        lat.replace('4', "ch").replace('9', "ya")
    } else {
        lat
    }
}

/// Whether `text` speaks of cheats: `читер`, `читы`, `4it`, `aimbot`, `wh`, `aim bot`; not `читаю`, `значит`,
/// `what`. `names`: nicknames and aliases, left out first.
pub fn cheating(text: &str, names: &[String]) -> bool {
    let words: Vec<String> = words(text, names).iter().map(|w| spoken(w)).collect();
    words
        .iter()
        .any(|w| CHEAT_WORDS.contains(&w.as_str()) || CHEAT_START.iter().any(|r| w.starts_with(r)))
        || words.windows(2).any(|p| p[0] == "aim" && p[1].starts_with("bot"))
}

/// `text` with every swear word and slur written as `…`; the `names` in it (nicknames and aliases) stay whole.
pub fn scrub(text: &str, names: &[String]) -> String {
    let kept = name_spans(text, names);
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for (start, end) in spans(text) {
        let named = kept.iter().any(|&(s, e)| s < end && start < e);
        let word = normal(&text[start..end]);
        if !named && (either(&word, swearing) || either(&word, slurring)) {
            out.push_str(&text[at..start]);
            out.push('…');
            at = end;
        }
    }
    out.push_str(&text[at..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn none() -> Vec<String> {
        Vec::new()
    }

    #[test]
    fn swearing_is_found_however_written() {
        for line in [
            "опять пiзда моему счёту",
            "Гордон опять пизданулся с крыши",
            "Аликс зашла, щас будет ебля?",
            "щас опять fuck полетит в чат)",
            "shittie карта, говорю же, лол",
            "заебал",
            "наебнулся",
            "долбоеб",
            "спиздил",
            "ДА ТУТ НИХУЯ НЕ ВИДНО)",
            "ахуеть",
            "сьебался уже с карты?",
            "пздц карта",
            "ЫСУКАААААААААААА",
            "yсука",
            "сюка, опять с рельсов снял",
            "СУУУУКАААААААА",
            "бляяяяяяя",
            "ЩО ЗА ПIЗДЕЦЬ!!!",
            "ДA HA XУЮ BEРTEЛ TAKУЮ KAРTУ",
            "ne pizdi, ja tut byl",
            "nu ty ebanko",
            "zaebali eti boty",
            "fuuuckkk",
            "ohfuck nooo",
            "cocksukerr",
            "пиздец",
            "сууука",
            "нахуй",
            "съебал",
            "блять",
            "blyat",
            "fuck you",
            "bullshit",
            "motherfucker",
            "ну ти й в'єбав, ніхуя собі",
            "3аебал",
            "выебали",
            "суперпидр",
            "ПЗДИЛ",
            ": С У К А !",
            "S H I T  HAPPENS!!!",
            "отсасывай",
            "клиторище",
            "пусть идёт сосать",
        ] {
            assert!(has(line, &none()), "{line}");
        }
    }

    #[test]
    fn clean_words_stay_clean() {
        for line in [
            "спидраны на этой карте норм",
            "ребята",
            "небо",
            "хлеб",
            "корабля",
            "рубля",
            "употребляет",
            "оскорбление",
            "страхуй",
            "застрахуйся",
            "сукно",
            "психолог",
            "психуй",
            "себя",
            "себе",
            "Глеб",
            "погреб",
            "мудрый",
            "бляха",
            "барсук",
            "сучок",
            "скипидар",
            "заколебал",
            "вебка",
            "хулиган",
            "class",
            "pass",
            "assist",
            "cockpit",
            "peacock",
            "мандарин",
            "damn",
            "учебник",
            "требовать",
            "дебил",
            "ебола",
            "son of a gun",
            "Не читаешь что?",
            "значит",
            "считать",
            "петуха снял ломом, чисто",
            "хулиганы тут",
            "убедил",
            "пребывать",
            "хачу",
            "сосиска",
            "сосна",
            "насос",
            "я и в шоке",
            "a b c d",
        ] {
            assert!(!has(line, &none()), "{line}");
            assert!(!slur(line, &none()), "{line}");
        }
    }

    #[test]
    fn names_are_not_swearing() {
        let names = vec!["_FUCK_".to_string(), "FUCK_THIS_GAME_noob".into()];
        assert!(!has("_FUCK_ опять тут", &names));
        assert!(has("fuck, _FUCK_", &names));
        assert!(!has("gg FUCK_THIS_GAME_noob", &names));
        assert!(has("_fuck_x", &names), "not a whole name");
        assert!(!slur("xoxol_007 зашёл", &["xoxol_007".into()]));
        let bots = vec!["Kleiner".to_string(), "Plutonium".into(), "Shit_Happens".into()];
        assert!(
            !has("kleiner, plutonium, shit_happens, привет", &bots),
            "a shorter name first"
        );
        assert!(has("kleiner, plutonium, бля", &bots));
        assert_eq!(without_names("kleiner и plutonium", &bots), "  и  ");
    }

    #[test]
    fn nicknames_are_read_harder_than_lines() {
        for name in ["pedik777", "xX_Piderok_Xx", "Pid0r_99", "suka1337", "h0h0l_x"] {
            assert!(nick(name), "{name}");
        }
        for name in ["Spiderman", "pedikur_pro", "Nordwind", "Ass4ssin"] {
            assert!(!nick(name), "{name}");
        }
        assert!(
            !slur("ну привет, Piderok", &none()) && !has("suka1337 опять тут", &none()),
            "a line may call such a player"
        );
    }

    #[test]
    fn nicknames_are_read_by_the_parts_their_capitals_start() {
        assert_eq!(camel("xXPiderokXx"), ["x", "X", "Piderok", "Xx"]);
        assert_eq!(camel("HLDMKing"), ["HLDM", "King"]);
        assert_eq!(camel("СуперГерой"), ["Супер", "Герой"]);
        assert_eq!(camel("gauss"), ["gauss"]);
        assert_eq!(camel("Pro1337Pedik"), ["Pro1337", "Pedik"]);
        assert_eq!(camel("5piderman"), ["5piderman"]);
        for name in [
            "MegaPedik",
            "xXPiderokXx",
            "SuperPider",
            "MrHoholX",
            "SukaBlyat123",
            "СуперПедик",
            "5Pedik",
            "1337Pedik",
            "Pro1337Pedik",
        ] {
            assert!(nick(name), "{name}");
        }
        for name in [
            "5piderman",
            "3pider",
            "SpiderMan",
            "ClassicGamer",
            "xXSpiderXx",
            "Player2Pedro",
        ] {
            assert!(!nick(name), "{name}");
        }
        let words = [
            "Dark", "Silent", "Rocket", "Night", "Iron", "Crazy", "Happy", "Shadow", "Classic", "Grass", "Glass",
            "Bass", "Spider", "Sniper", "Gamer", "Hunter", "Wolf", "Knight", "Panda", "Hopper", "Master", "Pass",
            "Peace", "Gauss", "Bunny", "Lucky", "Mister", "Captain", "Little", "Big", "Snow", "Fox", "King", "Hero",
        ];
        for a in words {
            assert!(!nick(&format!("Mr{a}")), "Mr{a}");
            for b in words {
                for name in [format!("{a}{b}"), format!("xX{a}{b}Xx"), format!("{a}{b}2000")] {
                    assert!(!nick(&name), "{name}");
                }
            }
        }
    }

    #[test]
    fn slurs() {
        for line in [
            "хохлы и чурки опять тут",
            "hohol is back",
            "пидор",
            "хохол",
            "faggot",
            "hoholy vezde, pidorasy",
            "homos everywhere",
            "жиды",
            "хачи понаехали",
        ] {
            assert!(slur(line, &none()), "{line}");
        }
        for line in [
            "петуха снял ломом, чисто",
            "жидкий стул",
            "хачапури",
            "сукно",
            "чурчхела",
        ] {
            assert!(!slur(line, &none()), "{line}");
        }
    }

    #[test]
    fn cheats_by_whole_words() {
        for line in [
            "nice wh",
            "ну и аимщик",
            "у него читы",
            "опять читоры тут",
            "is that an aim bot",
            "4it vklu4il?",
            "ya ne 4iter, prosto vezu4ij",
            "читер",
            "читак",
            "aimbot",
            "cheater",
            "вх",
            "это уже читерство какое-то",
            "10 подряд? читы включил?",
            "три за секунду, точно чит",
            "это не чит, это скилл",
        ] {
            assert!(cheating(line, &none()), "{line}");
        }
        for line in [
            "значит, ещё раунд",
            "считать фраги лень",
            "учитесь стрелять из гаусса",
            "медик опять не вылечит",
            "он кричит, а я играю",
            "серия скоро кончится",
            "аим сломался, ломом добил",
            "Не читаешь что?",
            "читаю) это Атлас тут",
            "читаешь",
            "прочитал",
            "читай",
            "what is that",
            "whose turn",
            "why",
            "вход",
        ] {
            assert!(!cheating(line, &none()), "{line}");
        }
        assert!(!cheating("gg Читер228", &["Читер228".into()]));
    }

    #[test]
    fn scrubbed_words_become_an_ellipsis() {
        assert_eq!(scrub("кричит «ебля?»", &none()), "кричит «…?»");
        assert_eq!(scrub("ну ты и пиздец, хохол", &none()), "ну ты и …, …");
        assert_eq!(
            scrub("FUCK_THIS_GAME_noob fuck", &["FUCK_THIS_GAME_noob".into()]),
            "FUCK_THIS_GAME_noob …"
        );
        assert_eq!(scrub("gg wp", &none()), "gg wp");
    }
}
