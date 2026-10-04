//! Words the prompts are made of, in Russian for `ru` and in English for any other language.

use lb_styles::persona::CHAT_MANNERS;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    Ru,
    En,
}

impl Lang {
    pub fn of(code: &str) -> Lang {
        if code.trim().eq_ignore_ascii_case("ru") {
            Lang::Ru
        } else {
            Lang::En
        }
    }
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
        "дерзко, любит подколоть",
        "немногословно, часто одним словом",
        "эмоционально, когда злится — капсом",
        "с юмором, шутит про карту и оружие",
    ];
    const EN: [&str; CHAT_MANNERS as usize] = [
        "short, lower case, no full stops",
        "short, lower case, often ends with :)",
        "hardly any punctuation, abbreviations (ty, np, idk)",
        "calm and polite: gg, wp, nice",
        "cheeky, likes to tease",
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
        (Lang::Ru, _) => "играешь очень сильно, тебя иногда зовут читером",
        (Lang::En, 0..=29) => "you play weakly, a newcomer",
        (Lang::En, 30..=59) => "you play about average",
        (Lang::En, 60..=84) => "you play well",
        (Lang::En, _) => "you play very well, some call you a cheater",
    }
}

/// `12 с назад`, `3 мин назад`, `12s ago`.
pub fn ago(lang: Lang, secs: f64) -> String {
    let secs = secs.max(0.0).round() as u64;
    match (lang, secs) {
        (Lang::Ru, 0..=4) => "только что".into(),
        (Lang::Ru, 5..=99) => format!("{secs} с назад"),
        (Lang::Ru, _) => format!("{} мин назад", (secs + 30) / 60),
        (Lang::En, 0..=4) => "just now".into(),
        (Lang::En, 5..=99) => format!("{secs}s ago"),
        (Lang::En, _) => format!("{} min ago", (secs + 30) / 60),
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
    }
}
