//! Words the prompts are made of, in Russian for `ru` and in English for any other language; and the language a
//! player's line is written in.

use std::cmp::Ordering;

use lb_styles::persona::CHAT_MANNERS;

use crate::addressing::squeeze;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    Ru,
    En,
}

impl Lang {
    /// The prompts' language for `chat.language`: Russian for `ru` (`ru-RU` too), English for any other.
    pub fn of(code: &str) -> Lang {
        if primary(code).eq_ignore_ascii_case("ru") {
            Lang::Ru
        } else {
            Lang::En
        }
    }
}

/// A language code's primary subtag: `ru` of `ru-RU` or `ru_RU`.
pub fn primary(code: &str) -> &str {
    code.trim().split(['-', '_']).next().unwrap_or("")
}

/// How a kill reads after "X killed Y": `ломом`, `with the crossbow`. Kill-feed names are the inflictor's
/// classname without `weapon_`/`monster_`.
pub fn weapon_phrase(lang: Lang, weapon: &str) -> String {
    let (ru, en) = match weapon.to_ascii_lowercase().as_str() {
        "crowbar" => ("ломом", "with the crowbar"),
        "9mmhandgun" | "glock" => ("из глока", "with the glock"),
        "357" | "python" => ("из питона", "with the 357"),
        "9mmar" | "mp5" => ("из MP5", "with the MP5"),
        "argrenade" => ("гранатой из MP5", "with an MP5 grenade"),
        "shotgun" => ("из дробовика", "with the shotgun"),
        "crossbow" | "bolt" | "crossbow_bolt" => ("из арбалета", "with the crossbow"),
        "rpg_rocket" | "rpg" => ("из ракетницы", "with a rocket"),
        "gauss" => ("из гаусса", "with the gauss"),
        "egon" => ("из эгона", "with the egon"),
        "hornet" | "hornetgun" => ("пчёлами из хорнетгана", "with hornets"),
        "grenade" | "handgrenade" => ("гранатой", "with a grenade"),
        "satchel" => ("сачелем", "with a satchel"),
        "tripmine" => ("миной", "with a tripmine"),
        "snark" => ("снарками", "with snarks"),
        "world" | "" => return String::new(),
        other => return format!("({other})"),
    };
    match lang {
        Lang::Ru => ru.into(),
        Lang::En => en.into(),
    }
}

/// A weapon by itself: `арбалет`, `crossbow`.
pub fn weapon_name(lang: Lang, weapon: &str) -> String {
    let (ru, en) = match weapon.to_ascii_lowercase().as_str() {
        "crowbar" => ("лом", "crowbar"),
        "9mmhandgun" | "glock" => ("глок", "glock"),
        "357" | "python" => ("питон", "357"),
        "9mmar" | "mp5" => ("MP5", "MP5"),
        "shotgun" => ("дробовик", "shotgun"),
        "crossbow" => ("арбалет", "crossbow"),
        "rpg" => ("ракетница", "RPG"),
        "gauss" => ("гаусс", "gauss"),
        "egon" => ("эгон", "egon"),
        "hornetgun" => ("хорнетган", "hornetgun"),
        "handgrenade" | "grenade" => ("гранаты", "grenades"),
        "satchel" => ("сачели", "satchels"),
        "tripmine" => ("мины", "tripmines"),
        "snark" => ("снарки", "snarks"),
        other => return other.to_string(),
    };
    match lang {
        Lang::Ru => ru.into(),
        Lang::En => en.into(),
    }
}

/// Explosives a player can blow themselves up with.
pub fn is_explosive(weapon: &str) -> bool {
    matches!(
        weapon.to_ascii_lowercase().as_str(),
        "grenade" | "argrenade" | "satchel" | "tripmine" | "rpg_rocket" | "snark" | "gauss" | "hornet"
    )
}

/// How a personality without its own `chat.style` writes; [`CHAT_MANNERS`] of them.
pub fn manner(lang: Lang, manner: u8) -> &'static str {
    const RU: [&str; CHAT_MANNERS as usize] = [
        "коротко, строчными буквами, без точек",
        "коротко, строчными, часто ставит ))",
        "почти без знаков препинания, с сокращениями (спс, щас, норм, хз)",
        "спокойно и вежливо: gg, wp, nice",
        "с подколками, но беззлобно",
        "немногословно, часто одним словом",
        "эмоционально, когда злится — капсом",
        "с юмором, шутит про карту и оружие",
    ];
    const EN: [&str; CHAT_MANNERS as usize] = [
        "short, lower case, no full stops",
        "short, lower case, often ends with :)",
        "hardly any punctuation, abbreviations (ty, np, idk)",
        "calm and polite: gg, wp, nice",
        "teasing, but never mean",
        "terse, often a single word",
        "emotional, caps when angry",
        "jokey, jokes about the map and the weapons",
    ];
    let i = usize::from(manner) % RU.len();
    match lang {
        Lang::Ru => RU[i],
        Lang::En => EN[i],
    }
}

/// How well a personality plays, from its skill 0..100.
pub fn skill(lang: Lang, skill: u8) -> &'static str {
    match (lang, skill) {
        (Lang::Ru, 0..=29) => "играешь слабо, новичок",
        (Lang::Ru, 30..=59) => "играешь средне",
        (Lang::Ru, 60..=84) => "играешь хорошо",
        (Lang::Ru, _) => "играешь очень сильно",
        (Lang::En, 0..=29) => "you play weakly, a newcomer",
        (Lang::En, 30..=59) => "you play about average",
        (Lang::En, 60..=84) => "you play well",
        (Lang::En, _) => "you play very well",
    }
}

/// `12 с назад`, `3 мин назад` up to an hour and a half, `2 ч назад` up to a day, then `вчера`, `3 дня назад`;
/// `12s ago`, `3 min ago`, `2 h ago`, `yesterday`.
pub fn ago(lang: Lang, secs: f64) -> String {
    let secs = secs.max(0.0).round() as u64;
    match (lang, secs) {
        (_, 86_400..) => days_ago(lang, secs),
        (Lang::Ru, 0..=4) => "только что".into(),
        (Lang::Ru, 5..=99) => format!("{secs} с назад"),
        (Lang::Ru, 100..=5399) => format!("{} мин назад", (secs + 30) / 60),
        (Lang::Ru, _) => format!("{} ч назад", (secs + 1800) / 3600),
        (Lang::En, 0..=4) => "just now".into(),
        (Lang::En, 5..=99) => format!("{secs}s ago"),
        (Lang::En, 100..=5399) => format!("{} min ago", (secs + 30) / 60),
        (Lang::En, _) => format!("{} h ago", (secs + 1800) / 3600),
    }
}

/// `вчера`, `3 дня назад`, `today`: how long ago a player was last seen.
pub fn days_ago(lang: Lang, secs: u64) -> String {
    let days = secs / 86_400;
    match (lang, days) {
        (Lang::Ru, 0) => "сегодня".into(),
        (Lang::Ru, 1) => "вчера".into(),
        (Lang::Ru, d) => format!("{d} {} назад", plural_ru(d, "день", "дня", "дней")),
        (Lang::En, 0) => "today".into(),
        (Lang::En, 1) => "yesterday".into(),
        (Lang::En, d) => format!("{d} days ago"),
    }
}

/// The Russian plural form for `n`: `1 день`, `2 дня`, `5 дней`.
pub fn plural_ru<'a>(n: u64, one: &'a str, few: &'a str, many: &'a str) -> &'a str {
    match (n % 10, n % 100) {
        (1, r) if r != 11 => one,
        (2..=4, r) if !(12..=14).contains(&r) => few,
        _ => many,
    }
}

/// English words Russians do not write in Latin letters. Not `a`, `i`, `u`, `to`, `on`, `no`, `my`, `do` (Russian
/// in Latin letters has them too), and not what every player writes: `gg`, `lol`, `ok`, `nice`, `noob`, `hi`, `go`.
#[rustfmt::skip]
const EN: [&str; 151] = [
    "you", "your", "yours", "youre", "ur", "me", "we", "he", "she", "they", "them", "it", "its", "im", "the", "an",
    "is", "are", "am", "was", "were", "be", "been", "being", "does", "did", "dont", "didnt", "doesnt", "cant", "can",
    "will", "would", "have", "has", "had", "not", "yes", "yeah", "yep", "nope", "okay", "and", "or", "but", "so",
    "too", "of", "in", "at", "for", "with", "from", "about", "this", "that", "what", "why", "how", "hows", "where",
    "who", "when", "there", "here", "all", "everyone", "everybody", "like", "just", "very", "now", "please", "pls",
    "plz", "thanks", "thank", "thx", "sorry", "man", "bro", "dude", "good", "bad", "great", "best", "love", "know",
    "think", "said", "say", "see", "look", "play", "come", "lets", "get", "got", "stop", "die", "damn", "cry", "bye",
    "loser", "lucky", "easy", "chance", "little", "dead", "funny", "cheat", "cheater", "map", "face", "angry",
    "understand", "again", "only", "one", "some", "any", "want", "need", "make", "take", "kill", "time", "game",
    "team", "win", "lose", "lost", "really", "never", "always", "still", "back", "if", "then", "than", "because",
    "way", "fine", "dog", "shots", "suck", "sucks", "bitch", "whore", "dick", "stfu",
];
/// Russian in Latin letters.
#[rustfmt::skip]
const RU: [&str; 124] = [
    "privet", "priv", "poka", "kak", "kto", "chto", "4to", "che", "cho", "chego", "gde", "kuda", "otkuda", "pochemu",
    "zachem", "kogda", "skolko", "ladno", "spasibo", "norm", "blin", "davai", "davay", "eto", "etot", "tut", "tam",
    "nu", "da", "net", "ne", "vot", "ty", "ti", "tebya", "tebe", "tvoi", "tvoy", "menya", "mne", "mnoy", "vy", "vas",
    "vam", "uzhe", "uje", "eshe", "esche", "tozhe", "toje", "tipa", "prosto", "tak", "takoi", "takoy", "takaya",
    "vse", "vsem", "idi", "sogl", "krasava", "krasivo", "aga", "blya", "blyat", "suka", "pizdec", "nahui", "nahuy",
    "hui", "xui", "huy", "zaebal", "pidor", "debil", "tupoi", "tupoy", "loh", "lox", "loxa", "shutka", "esli",
    "netu", "ponjal", "ponyal", "ponjatno", "ponyatno", "smotri", "posmotri", "posidi", "zatknis", "estestvenno",
    "opyat", "opyatb", "shas", "ku", "za", "na", "po", "ot", "iz", "bez", "dlya", "budet", "est", "igra", "lomom",
    "lan", "xoxol", "hohol", "pituh", "chel", "nado", "mozhno", "mojno", "horosho", "xorosho", "potomu", "konechno",
    "seychas", "zdes", "ochen", "bota", "boty",
];
/// Turkish, in Latin letters without Turkish ones (`guzel` for `güzel`).
#[rustfmt::skip]
const TR: [&str; 34] = [
    "orospu", "cocug", "cocuk", "amk", "siktir", "sikeyim", "sikerim", "anani", "gotunden", "naber", "tamam", "evet",
    "hayir", "degil", "abi", "kanka", "hadi", "gel", "gol", "guzel", "oyun", "yok", "ben", "sen", "bir", "cok",
    "iyi", "neden", "nerede", "merhaba", "selam", "kardes", "daha", "olmadi",
];

/// Whether a character is Cyrillic.
pub(crate) fn cyrillic(c: char) -> bool {
    ('\u{400}'..='\u{4ff}').contains(&c)
}

/// The language a word (lower case) votes for: Cyrillic or Russian in Latin letters (a digit for a letter, `teb9`; a
/// soft sign written `'`; `-ij`, `-sja`), English (`fucking`, `don't`), Turkish (`güzel`, `goool`).
fn vote(word: &str) -> Option<&'static str> {
    const CONTRACTIONS: [&str; 7] = ["n't", "'s", "'m", "'re", "'ll", "'ve", "'d"];
    if word.chars().any(cyrillic) {
        return Some("ru");
    }
    if word.contains('\'') {
        return Some(if CONTRACTIONS.iter().any(|c| word.ends_with(c)) {
            "en"
        } else {
            "ru"
        });
    }
    let turkish: String = word
        .chars()
        .map(|c| match c {
            'ı' => 'i',
            'ş' => 's',
            'ğ' => 'g',
            'ü' => 'u',
            'ö' => 'o',
            'ç' => 'c',
            _ => c,
        })
        .collect();
    let letters = word.chars().filter(char::is_ascii_alphabetic).count();
    let vowel = word.contains(['a', 'e', 'i', 'o', 'u', 'y']);
    let stretched = word.as_bytes().windows(3).any(|w| w[0] == w[1] && w[1] == w[2]);
    if TR.contains(&turkish.as_str()) || (stretched && TR.contains(&squeeze(&turkish).as_str())) {
        Some("tr")
    } else if EN.contains(&word) || word.starts_with("fuck") || word.starts_with("shit") {
        Some("en")
    } else if RU.contains(&word)
        || (letters >= 2 && vowel && word.contains(['3', '4', '6', '9']))
        || (word.len() >= 5 && ["ij", "yj", "sja", "sya"].iter().any(|e| word.ends_with(e)))
    {
        Some("ru")
    } else {
        None
    }
}

/// The language a line's letters tell: Turkish `ı ş ğ`, German `ä ö ü ß`, Polish `ą ę ł`, Spanish `ñ ¿`,
/// Portuguese `ã õ ç`.
fn by_letters(text: &str) -> Option<&'static str> {
    text.chars().find_map(|c| match c {
        'ı' | 'İ' | 'ş' | 'Ş' | 'ğ' | 'Ğ' => Some("tr"),
        'ä' | 'ö' | 'ü' | 'ß' | 'Ä' | 'Ö' | 'Ü' | 'ẞ' => Some("de"),
        'ą' | 'ę' | 'ł' | 'ś' | 'ź' | 'ż' | 'ć' | 'ń' | 'Ą' | 'Ę' | 'Ł' | 'Ś' | 'Ź' | 'Ż' | 'Ć' | 'Ń' => {
            Some("pl")
        }
        'ñ' | 'Ñ' | '¿' | '¡' => Some("es"),
        'ã' | 'õ' | 'ç' | 'Ã' | 'Õ' | 'Ç' => Some("pt"),
        _ => None,
    })
}

/// The language `text` is written in, when it shows: `ru` for Cyrillic or Russian in Latin letters (`privet`,
/// `teb9`, `slovarnij`), `en`, `tr`, and `de`, `pl`, `es`, `pt` by their letters, unless the words are plainly
/// English (`6İ6A FUCK YOU`, typed on a Turkish keyboard). `None` for a line of no language (`gg`, `hi`, `)))`,
/// `A TO)`); the player's other lines may tell.
pub fn detect(text: &str) -> Option<&'static str> {
    if text.chars().filter(|&c| cyrillic(c) && c.is_alphabetic()).count() >= 3 {
        return Some("ru");
    }
    let (mut en, mut ru, mut tr) = (0, 0, 0);
    let lower = text.replace('İ', "i").to_lowercase().replace(['’', 'ʼ', '`'], "'");
    for word in lower.split(|c: char| !c.is_alphanumeric() && c != '\'') {
        let word = match word.strip_prefix('\'') {
            Some(quoted) => quoted.trim_matches('\''),
            None => word,
        };
        match vote(word) {
            Some("en") => en += 1,
            Some("ru") => ru += 1,
            Some(_) => tr += 1,
            None => {}
        }
    }
    if tr > 0 && tr >= en && tr > ru {
        return Some("tr");
    }
    if let Some(code) = by_letters(text)
        && !(en > ru && en > tr)
    {
        return Some(code);
    }
    match ru.cmp(&en) {
        Ordering::Greater => Some("ru"),
        Ordering::Less => Some("en"),
        Ordering::Equal => None,
    }
}

/// Whether a line in `code` (as [`detect`] tells it) is in neither the server's language (`chat.language`, by its
/// primary subtag: `ru` of `ru-RU`) nor English: nobody answers it.
pub fn foreign(code: &str, server: &str) -> bool {
    !code.eq_ignore_ascii_case(primary(server)) && !code.eq_ignore_ascii_case("en")
}

/// The language a player writes in, as [`detect`] tells it: their `line`, else the one most of their `others` are
/// in. `None` when none tells, or two languages tie.
pub fn writes<'a>(line: Option<&str>, others: impl IntoIterator<Item = &'a str>) -> Option<&'static str> {
    if let Some(code) = line.and_then(detect) {
        return Some(code);
    }
    let mut votes: Vec<(&'static str, usize)> = Vec::new();
    for code in others.into_iter().filter_map(detect) {
        match votes.iter_mut().find(|(c, _)| *c == code) {
            Some((_, n)) => *n += 1,
            None => votes.push((code, 1)),
        }
    }
    let most = votes.iter().map(|&(_, n)| n).max()?;
    let mut top = votes.iter().filter(|&&(_, n)| n == most);
    let (code, _) = top.next()?;
    top.next().is_none().then_some(*code)
}

/// What the reason for a line adds for a player who writes `code` on a server of `server` (`chat.language`): answer
/// in English, or in Russian. Nothing for the server's own language, nor for any other: lines in it get no answer.
pub fn spoken(lang: Lang, code: &str, server: &str) -> Option<&'static str> {
    if code.eq_ignore_ascii_case(primary(server)) {
        return None;
    }
    match (lang, code) {
        (Lang::Ru, "en") => Some("Пишет по-английски — ответь по-английски."),
        (Lang::En, "en") => Some("They write in English: answer in English."),
        (Lang::En, "ru") => Some("They write in Russian: answer in Russian."),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phrases() {
        assert_eq!(weapon_phrase(Lang::Ru, "crowbar"), "ломом");
        assert_eq!(weapon_phrase(Lang::En, "9mmAR"), "with the MP5");
        assert_eq!(weapon_phrase(Lang::Ru, "world"), "");
        assert_eq!(weapon_phrase(Lang::Ru, "trigger_hurt"), "(trigger_hurt)");
        assert_eq!(weapon_name(Lang::Ru, "crossbow"), "арбалет");
        assert!(is_explosive("satchel") && !is_explosive("crowbar"));
        assert_eq!(Lang::of("RU"), Lang::Ru);
        assert_eq!(Lang::of("de"), Lang::En);
    }

    #[test]
    fn codes_go_by_their_primary_subtag() {
        assert_eq!(
            (primary(" ru-RU "), primary("pt_BR"), primary("en")),
            ("ru", "pt", "en")
        );
        for code in ["ru-RU", "RU_ru", " ru "] {
            assert_eq!(Lang::of(code), Lang::Ru, "{code}");
        }
        assert_eq!(Lang::of("rus"), Lang::En);
    }

    #[test]
    fn a_player_s_language_and_the_hint_for_it() {
        assert_eq!(writes(Some("where are you hiding?"), ["привет всем"]), Some("en"));
        assert_eq!(
            writes(
                Some("hi"),
                ["i dont understand cyrillic", "привет", "where is everyone"]
            ),
            Some("en")
        );
        assert_eq!(writes(Some("gg"), ["привет", "where are you"]), None, "a tie");
        assert_eq!(writes(None, ["lol", ")))"]), None);
        assert_eq!(writes(None, ["где все", "gg", "го рельсы"]), Some("ru"));
        assert_eq!(
            spoken(Lang::Ru, "en", "ru-RU"),
            Some("Пишет по-английски — ответь по-английски.")
        );
        assert_eq!(spoken(Lang::Ru, "ru", "ru-RU"), None);
        assert_eq!(spoken(Lang::En, "en", "en"), None);
        assert_eq!(
            spoken(Lang::En, "en", "de"),
            Some("They write in English: answer in English.")
        );
        assert_eq!(
            spoken(Lang::En, "ru", "en"),
            Some("They write in Russian: answer in Russian.")
        );
        assert_eq!(
            spoken(Lang::Ru, "tr", "ru"),
            None,
            "lines in other languages get no answer"
        );
    }

    #[test]
    fn manners_cover_every_personality() {
        for m in 0..CHAT_MANNERS {
            assert!(!manner(Lang::Ru, m).is_empty() && !manner(Lang::En, m).is_empty());
        }
    }

    #[test]
    fn times() {
        assert_eq!(ago(Lang::Ru, 2.0), "только что");
        assert_eq!(ago(Lang::Ru, 12.4), "12 с назад");
        assert_eq!(ago(Lang::En, 200.0), "3 min ago");
        assert_eq!(days_ago(Lang::Ru, 86_400 * 3), "3 дня назад");
        assert_eq!(days_ago(Lang::Ru, 86_400 * 11), "11 дней назад");
        assert_eq!(days_ago(Lang::Ru, 86_400 * 21), "21 день назад");
        assert_eq!(days_ago(Lang::En, 3600), "today");
        assert_eq!(ago(Lang::Ru, 5399.0), "90 мин назад");
        assert_eq!(ago(Lang::Ru, 7200.0), "2 ч назад");
        assert_eq!(ago(Lang::Ru, 86_399.0), "24 ч назад");
        assert_eq!(ago(Lang::Ru, 90_000.0), "вчера");
        assert_eq!(ago(Lang::En, 7200.0), "2 h ago");
        assert_eq!(ago(Lang::En, 3.5 * 86_400.0), "3 days ago");
    }

    #[test]
    fn the_card_calls_nobody_a_cheater() {
        for lang in [Lang::Ru, Lang::En] {
            for s in [0, 50, 70, 100] {
                assert!(!skill(lang, s).contains("читер") && !skill(lang, s).contains("cheat"));
            }
        }
        assert_eq!(manner(Lang::Ru, 4), "с подколками, но беззлобно");
        assert_eq!(manner(Lang::En, 4), "teasing, but never mean");
    }

    #[test]
    fn languages_of_real_lines() {
        for (line, code) in [
            ("Lambda loser", Some("en")),
            ("DAMN.", Some("en")),
            ("me too", Some("en")),
            ("Plutonium,fuck you", Some("en")),
            ("i dont understand cyrillic", Some("en")),
            ("i don't understand", Some("en")),
            ("whwre u from bro", Some("en")),
            ("stop aim bot", Some("en")),
            ("plutonium shittie bot", Some("en")),
            (
                "i think he is something else... i didnt see like such a bot",
                Some("en"),
            ),
            ("you 2 are boyfriends?", Some("en")),
            ("cheater player orospu cocug", Some("tr")),
            ("HİÇ GÜZEL Bİ OYUN OLMADI BEN SİZİ VURAMIYORM", Some("tr")),
            ("ananı gotunden sıkeyım player", Some("tr")),
            ("mol4it, obi4no pogovorit' ljubit", Some("ru")),
            ("plutonium y teb9 4it est?", Some("ru")),
            ("2 4ela vs 2 bota)", Some("ru")),
            ("posidi v specte i posmotri gde ya 4iter", Some("ru")),
            ("slovarnij zapas nemmalin'kij", Some("ru")),
            ("ti bot?", Some("ru")),
            ("ta lan", Some("ru")),
            ("bot tupoi", Some("ru")),
            ("nu daa", Some("ru")),
            ("debil eto bot", Some("ru")),
            ("ESLI CHE TAK", Some("ru")),
            ("kak zaebal bot", Some("ru")),
            ("spasibo", Some("ru")),
            ("привет", Some("ru")),
            ("ок да", Some("ru")),
            ("plutonium пропиши agstart", Some("ru")),
            ("0_o Ніхуясобібля! щє в'єбав... е", Some("ru")),
            ("spaß", Some("de")),
            ("6İ6A FUCK YOU MOTHER", Some("en")),
            ("fuck you mother profıl1", Some("en")),
            ("bır goool dahaaaaa", Some("tr")),
            ("been there", Some("en")),
            ("seen", None),
            ("tüh", Some("de")),
            ("sorry mama netu takoi zemli", Some("ru")),
            ("cgc gkenjybev", None),
            ("HAHAHHA noob", None),
            ("O,zagovoril)))", None),
            ("gg", None),
            ("hi", None),
            ("A TO)", None),
            ("lol", None),
            ("))", None),
            ("gg_cold_rock", None),
            ("4ck", None),
            ("=D", None),
            ("'hello'", None),
        ] {
            assert_eq!(detect(line), code, "{line}");
        }
    }

    #[test]
    fn foreign_lines() {
        assert!(foreign("tr", "ru") && foreign("de", "ru-RU"));
        assert!(!foreign("ru", "ru") && !foreign("ru", "RU_ru") && !foreign("en", "ru") && !foreign("en", "en"));
        assert!(foreign("ru", "de") && !foreign("de", "de"));
    }
}
