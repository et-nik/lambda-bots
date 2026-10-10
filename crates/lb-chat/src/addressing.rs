//! Whether a chat line speaks to a bot: its name as players shorten it (`Атлас` for `DUT9 ATLASA`), in either
//! script, or the bots as a whole.

/// Words that name nobody in particular, even when they are part of a nickname.
const COMMON: [&str; 16] = [
    "bot", "bots", "bota", "boty", "player", "igrok", "noob", "nub", "pro", "the", "gg", "lol", "hl", "mr", "king",
    "best",
];

/// Lower-case words of `text` written in Latin letters.
fn words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| latin(&w.to_lowercase()))
        .collect()
}

/// Cyrillic transliterated to Latin; everything else as it is.
fn latin(word: &str) -> String {
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

/// The words of `name` players would call its owner by: clan tags (`[TAG]`, `|TAG|`, `(TAG)`) left out unless
/// nothing else is left, three letters at least, common words skipped.
pub fn name_words(name: &str) -> Vec<String> {
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
    let keep = |w: &String| w.chars().count() >= 3 && !COMMON.contains(&w.as_str());
    let found: Vec<String> = words(&bare).into_iter().filter(keep).collect();
    if found.is_empty() {
        words(name).into_iter().filter(keep).collect()
    } else {
        found
    }
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
    common >= 4.max(w.min(t).saturating_sub(2))
}

/// Whether `text` calls the player named `name`: one of its words, or a word that starts the same way (`Атлас`,
/// `атласу` for `ATLASA`; `klein` for `Kleiner`).
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

/// Whether `text` speaks to the bots as a whole: `боты`, `bots`, `ботяра`.
pub fn to_bots(text: &str) -> bool {
    const ENDINGS: [&str; 15] = [
        "", "s", "z", "y", "a", "u", "e", "om", "ov", "am", "ami", "ah", "yara", "ik", "iki",
    ];
    words(text)
        .iter()
        .any(|w| w.strip_prefix("bot").is_some_and(|rest| ENDINGS.contains(&rest)))
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
    }

    #[test]
    fn mentions_by_shortened_and_declined_names() {
        assert!(mentions("Дитя Атласа, ты в своём уме?", "DUT9 ATLASA"));
        assert!(mentions("атлас го 1на1", "DUT9 ATLASA"));
        assert!(mentions("klein you camper", "[BOT] Kleiner"));
        assert!(mentions("кляйнеру привет", "Кляйнер"));
        assert!(mentions("nos gg", "-|NoS|-"));
        assert!(mentions("ковбою респект", "Ковбой"));
        assert!(!mentions("нос чешется", "Kleiner"));
        assert!(!mentions("and then", "Andrey"));
        assert!(!mentions("gg", "-|NoS|-"));
        assert!(!mentions("player one", "Pro Player"));
    }

    #[test]
    fn bots_as_a_whole() {
        assert!(to_bots("боты, вы где"));
        assert!(to_bots("eto bot?"));
        assert!(to_bots("ботяра"));
        assert!(!to_bots("bottle"));
        assert!(!to_bots("привет всем"));
    }
}
