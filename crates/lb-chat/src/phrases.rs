//! Ready phrases for moments of the game (`config/chat/phrases.yaml`): which moment a bot's line is for, the phrases
//! it may take in the language of the player it is for, filled in with that player's name, a count, a weapon or the
//! map, in the bot's manner, and not those said lately. The worker picks them: nothing here reads files, and chance
//! comes only from the generator the caller passes.

use std::collections::{BTreeMap, VecDeque};

use lb_config::chat_phrases::{self, BotPhrases, ChatPhrasesFile, MAP_FILL, Moment, Phrasebook, Piece};
use lb_core::rng::Pcg32;
use lb_styles::persona::CHAT_MANNERS;

use crate::aliases::{self, Aliases};
use crate::journal::{Event, Notable, STREAK_SPOKEN, Who};
use crate::lang::{self, Lang};
use crate::profanity;
use crate::prompt;
use crate::request::{BotCard, ChatRequest, Recent, Trigger};

/// Phrases the ring keeps: one of them is not picked again while others are left.
pub const RING: usize = 30;

/// The moment `trigger` is for the bot whose `userid` is `bot`: its own streak or another player's (from
/// [`STREAK_SPOKEN`] kills), a greeting for a player who joined or said hi to everybody. `None` for an answer to a
/// player's line, and for the sides of a moment no phrase is for (a revenge on the bot, the bot as a nemesis, a rage
/// quit, a map another bot or nobody won).
pub fn key(trigger: &Trigger, bot: i32) -> Option<Moment> {
    let me = |w: &Who| w.userid == bot;
    match trigger {
        Trigger::Joined { .. } | Trigger::Greeted { .. } => Some(Moment::Greet),
        Trigger::Notable(n) => match n {
            Notable::Streak { count, .. } if *count < STREAK_SPOKEN => None,
            Notable::Streak { killer, .. } if me(killer) => Some(Moment::Streak),
            Notable::Streak { .. } => Some(Moment::StreakOther),
            Notable::Multikill { killer, .. } if me(killer) => Some(Moment::Multikill),
            Notable::Multikill { .. } => Some(Moment::MultikillOther),
            Notable::Revenge { killer, .. } => me(killer).then_some(Moment::Revenge),
            Notable::Nemesis { victim, .. } => me(victim).then_some(Moment::Nemesis),
            Notable::Humiliation { victim, .. } if me(victim) => Some(Moment::Crowbarred),
            Notable::Humiliation { killer, .. } => me(killer).then_some(Moment::CrowbarKill),
            Notable::OwnBlast { victim, .. } if me(victim) => Some(Moment::OwnBlast),
            Notable::OwnBlast { .. } => Some(Moment::OwnBlastOther),
            Notable::RageQuit { .. } => None,
        },
        Trigger::KilledWhileTyping { .. } => Some(Moment::KilledTyping),
        Trigger::LastLevel => Some(Moment::LastLevel),
        Trigger::MatchEnd { won: true, .. } => Some(Moment::Win),
        Trigger::MatchEnd { winner: Some(w), .. } if !w.bot => Some(Moment::Gg),
        Trigger::MatchEnd { .. } => None,
        Trigger::Addressed { .. }
        | Trigger::Continued { .. }
        | Trigger::Question { .. }
        | Trigger::Overheard { .. } => None,
    }
}

/// A phrase of `phrases.yaml` as written, trimmed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Phrase {
    pub text: String,
    /// The bot's own ([`BotPhrases`]): said as written, never in its manner.
    pub own: bool,
}

/// Whether two phrases are the same, trimmed, in any case.
fn same(a: &str, b: &str) -> bool {
    a.trim().to_lowercase() == b.trim().to_lowercase()
}

/// Whether two lines say the same, letters and digits alone, in any case: `гг, Атлас))` is `гг атлас`.
fn alike(a: &str, b: &str) -> bool {
    let words = |s: &str| -> Vec<String> {
        s.split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty())
            .map(str::to_lowercase)
            .collect()
    };
    let a = words(a);
    !a.is_empty() && a == words(b)
}

/// The bot's own phrases: by its persona, else by its nickname.
fn bot_phrases<'a>(file: &'a ChatPhrasesFile, persona: &str, nick: &str) -> Option<&'a BotPhrases> {
    file.bot(persona).or_else(|| file.bot(nick))
}

/// A book's section for the language `code`, written in any case.
fn section<'a>(book: &'a Phrasebook, code: &str) -> Option<&'a BTreeMap<Moment, Vec<String>>> {
    book.iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(code))
        .map(|(_, moments)| moments)
}

/// The phrases a book lists for `moment` in the language `code`.
fn listed<'a>(book: &'a Phrasebook, code: &str, moment: Moment) -> Option<&'a Vec<String>> {
    section(book, code).and_then(|moments| moments.get(&moment))
}

/// The language of a request's phrases: English for a player who writes it ([`prompt::partner_language`];
/// `remembered`: their lines the memory keeps), else the server's (`chat.language`, or its primary subtag: `ru` of
/// `ru-RU`); `en` when `phrases.yaml` has that section neither for every bot nor for this one. One language for every
/// moment: a section that leaves a moment out gets nothing of another language for it.
pub fn language(file: &ChatPhrasesFile, req: &ChatRequest, remembered: &[&str]) -> String {
    let wanted = match prompt::partner_language(req, remembered) {
        Some("en") => "en",
        _ => req.language.trim(),
    };
    let own = bot_phrases(file, &req.bot.persona, &req.bot.name);
    let has =
        |code: &str| section(&file.phrases, code).is_some() || own.is_some_and(|b| section(&b.phrases, code).is_some());
    [wanted, lang::primary(wanted)]
        .into_iter()
        .find(|code| !code.is_empty() && has(code))
        .unwrap_or("en")
        .to_ascii_lowercase()
}

/// The phrases of `moment` in the language `code` for the bot of this persona or nickname: its own ([`BotPhrases`])
/// and every bot's, or its own alone for a moment it lists with `replace: true`. A phrase found twice (trimmed, in any
/// case) is kept once, as the bot's own.
pub fn pool(file: &ChatPhrasesFile, code: &str, persona: &str, nick: &str, moment: Moment) -> Vec<Phrase> {
    let own = bot_phrases(file, persona, nick);
    let mine = own.and_then(|b| listed(&b.phrases, code, moment));
    let common = match own {
        Some(b) if b.replace && mine.is_some() => None,
        _ => listed(&file.phrases, code, moment),
    };
    let all = mine
        .into_iter()
        .flatten()
        .map(|text| (text, true))
        .chain(common.into_iter().flatten().map(|text| (text, false)));
    let mut out: Vec<Phrase> = Vec::new();
    for (text, own) in all {
        let text = text.trim();
        if !text.is_empty() && !out.iter().any(|p| same(&p.text, text)) {
            out.push(Phrase {
                text: text.to_string(),
                own,
            });
        }
    }
    out
}

/// What a phrase's placeholders are filled with, and how long it may be.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Fill {
    /// `{name}`: the other player of the moment, a human ([`prompt::partner`]), as the bots call them; none when the
    /// name swears or holds a slur ([`profanity::nick`]).
    pub name: Option<String>,
    /// `{count}`: kills in a row or at once, or how many times in a row the player killed the bot.
    pub count: Option<u32>,
    /// `{weapon}` as a phrase: `сачелем`, `with a satchel`.
    pub weapon: Option<String>,
    /// `{map}`: the map's name.
    pub map: Option<String>,
    /// Characters the filled phrase may take ([`ChatRequest::max_chars`]).
    pub max_chars: usize,
}

impl Fill {
    /// The values of the request's moment for phrases in `lang`. The name is the player's main alias, else their
    /// nickname without its tags ([`aliases::short`]).
    pub fn of(req: &ChatRequest, lang: Lang, aliases: &Aliases) -> Fill {
        let name = prompt::partner(req)
            .map(|w| {
                aliases
                    .of(&w.name)
                    .map_or_else(|| aliases::short(&w.name), str::to_string)
            })
            .filter(|name| !profanity::nick(name));
        let (count, weapon) = match &req.trigger {
            Trigger::Notable(Notable::Streak { count, .. } | Notable::Multikill { count, .. }) => (Some(*count), None),
            Trigger::Notable(Notable::Nemesis { times, .. }) => (Some(*times), None),
            Trigger::Notable(Notable::OwnBlast { weapon, .. }) => {
                let phrase = lang::weapon_phrase(lang, weapon);
                (None, (!phrase.is_empty() && !phrase.starts_with('(')).then_some(phrase))
            }
            _ => (None, None),
        };
        let map = req.scene.map.trim();
        Fill {
            name,
            count,
            weapon,
            map: (!map.is_empty()).then(|| map.to_string()),
            max_chars: req.max_chars,
        }
    }
}

/// `template` with its placeholders filled in, in one pass: a name that holds `{map}` stays as it is. `None` when a
/// placeholder has no value, or the line is longer than [`Fill::max_chars`].
pub fn fill(template: &str, values: &Fill) -> Option<String> {
    let mut out = String::new();
    for piece in chat_phrases::pieces(template.trim()).ok()? {
        match piece {
            Piece::Text(text) => out.push_str(text),
            Piece::Fill("name") => out.push_str(values.name.as_deref()?),
            Piece::Fill("count") => out.push_str(&values.count?.to_string()),
            Piece::Fill("weapon") => out.push_str(values.weapon.as_deref()?),
            Piece::Fill(MAP_FILL) => out.push_str(values.map.as_deref()?),
            Piece::Fill(_) => return None,
        }
    }
    (out.chars().count() <= values.max_chars).then_some(out)
}

/// A common phrase in the manner of a bot without a `chat.style` of its own ([`lang::manner`]): manners 0, 1, 2 and
/// 5 write in lower case without one full stop at the end, and manner 1 adds a smile half the time (`))` in Russian,
/// ` :)` otherwise) unless the line already ends in a bracket, `!` or `?`; the others write as the phrase does. For a
/// phrase as written: placeholders are lower case already, and a name filled in later keeps its own.
pub fn styled(text: &str, manner: u8, lang: Lang, rng: &mut Pcg32) -> String {
    let manner = manner % CHAT_MANNERS;
    let mut out = text.trim().to_string();
    if matches!(manner, 0 | 1 | 2 | 5) {
        out = out.to_lowercase();
        if out.ends_with('.') && !out.ends_with("..") {
            out.pop();
        }
    }
    if manner == 1 && !out.ends_with([')', '(', '!', '?']) && rng.next_f32() < 0.5 {
        out.push_str(match lang {
            Lang::Ru => "))",
            Lang::En => " :)",
        });
    }
    out
}

/// A phrase a request may take, filled in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub phrase: Phrase,
    /// The phrase with its placeholders filled ([`fill`]).
    pub filled: String,
}

/// The phrases picked lately by every bot, the latest last: the worker keeps one while it runs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Ring {
    /// Phrases as written, trimmed, in lower case.
    picks: VecDeque<String>,
}

impl Ring {
    /// One of `candidates`: at random among those not picked lately, else the one picked longest ago. It is the
    /// latest pick then.
    pub fn pick<'a>(&mut self, candidates: &'a [Candidate], rng: &mut Pcg32) -> Option<&'a Candidate> {
        let fresh: Vec<&Candidate> = candidates
            .iter()
            .filter(|c| self.when(&c.phrase.text).is_none())
            .collect();
        let chosen = if fresh.is_empty() {
            candidates.iter().min_by_key(|c| self.when(&c.phrase.text))?
        } else {
            fresh[rng.range_i32(0, fresh.len() as i32 - 1) as usize]
        };
        self.note(&chosen.phrase.text);
        Some(chosen)
    }

    /// Where the phrase stands among the picks, the oldest first.
    fn when(&self, text: &str) -> Option<usize> {
        let text = text.trim().to_lowercase();
        self.picks.iter().position(|p| *p == text)
    }

    fn note(&mut self, text: &str) {
        let text = text.trim().to_lowercase();
        self.picks.retain(|p| *p != text);
        self.picks.push_back(text);
        while self.picks.len() > RING {
            self.picks.pop_front();
        }
    }
}

/// The lines of the request's chat.
fn said(chat: &[Recent]) -> Vec<&str> {
    chat.iter()
        .filter_map(|r| match &r.event {
            Event::Chat { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

/// A request's moment and the phrases it may be met with.
#[derive(Clone, Debug, PartialEq)]
pub struct Offer {
    pub moment: Moment,
    /// The phrases' language ([`language`]).
    pub language: String,
    pub fill: Fill,
    /// The moment's phrases that fill, but those already said in the request's chat.
    pub candidates: Vec<Candidate>,
}

impl Offer {
    /// What `phrases.yaml` offers the request: `None` when its trigger is no moment ([`key`]) or none of the moment's
    /// phrases fills. `aliases`: those of the players the request speaks of; `remembered`: the lines the memory keeps
    /// of the player it is for, which may tell their language.
    pub fn of(file: &ChatPhrasesFile, req: &ChatRequest, aliases: &Aliases, remembered: &[&str]) -> Option<Offer> {
        let moment = key(&req.trigger, req.bot.userid)?;
        let language = language(file, req, remembered);
        let values = Fill::of(req, Lang::of(&language), aliases);
        let said = said(&req.chat);
        let candidates: Vec<Candidate> = pool(file, &language, &req.bot.persona, &req.bot.name, moment)
            .into_iter()
            .filter_map(|phrase| {
                let filled = fill(&phrase.text, &values)?;
                (!said.iter().any(|s| alike(s, &filled))).then_some(Candidate { phrase, filled })
            })
            .collect();
        (!candidates.is_empty()).then_some(Offer {
            moment,
            language,
            fill: values,
            candidates,
        })
    }

    /// The line to type: a phrase picked past the `ring` ([`Ring::pick`]), in the bot's manner ([`styled`]) when it
    /// is a common one and the bot has no `chat.style` of its own.
    pub fn pick(&self, ring: &mut Ring, bot: &BotCard, rng: &mut Pcg32) -> Option<String> {
        let c = ring.pick(&self.candidates, rng)?;
        if c.phrase.own || bot.manner_text.is_some() {
            return Some(c.filled.clone());
        }
        let styled = styled(&c.phrase.text, bot.manner, Lang::of(&self.language), rng);
        Some(fill(&styled, &self.fill).unwrap_or_else(|| c.filled.clone()))
    }
}

#[cfg(test)]
mod tests {
    use lb_config::chat_phrases::PHRASE_MAX;

    use super::*;
    use crate::request::{Said, Scene};

    fn who(slot: u8, name: &str, bot: bool) -> Who {
        Who {
            slot,
            userid: i32::from(slot) + 100,
            name: name.into(),
            bot,
        }
    }

    fn bot() -> Who {
        who(1, "[B] Plutonium", true)
    }

    fn gordon() -> Who {
        who(2, "Gordon", false)
    }

    fn request(trigger: Trigger) -> ChatRequest {
        let me = bot();
        ChatRequest {
            id: 7,
            bot: BotCard {
                name: me.name.clone(),
                persona: "Plutonium".into(),
                userid: me.userid,
                skill: 60,
                style: "rusher".into(),
                favourite_weapons: Vec::new(),
                profanity: false,
                manner_text: None,
                manner: 0,
                about: None,
                boldness: 0.0,
                alive: true,
                frags: 0,
                deaths: 0,
                level: None,
            },
            trigger,
            scene: Scene {
                map: "crossfire".into(),
                gungame: false,
                teamplay: false,
                elapsed: 300.0,
                players: Vec::new(),
                leader: None,
            },
            events: Vec::new(),
            chat: Vec::new(),
            own: Vec::new(),
            talk: Vec::new(),
            language: "ru".into(),
            max_chars: 56,
            team: false,
        }
    }

    fn file(body: &str) -> ChatPhrasesFile {
        ChatPhrasesFile::parse(&format!("schema: lambdabots/chat-phrases@1\n{body}"), "phrases.yaml").unwrap()
    }

    fn texts(pool: &[Phrase]) -> Vec<(&str, bool)> {
        pool.iter().map(|p| (p.text.as_str(), p.own)).collect()
    }

    #[test]
    fn moments_of_the_bot_and_of_others() {
        let (me, h, other) = (bot(), gordon(), who(3, "Kleiner", true));
        let id = me.userid;
        let of = |n: Notable| key(&Trigger::Notable(n), id);
        let streak = |killer: &Who, count| Notable::Streak {
            killer: killer.clone(),
            count,
            humans: 3,
        };
        assert_eq!(of(streak(&me, STREAK_SPOKEN)), Some(Moment::Streak));
        assert_eq!(of(streak(&h, STREAK_SPOKEN)), Some(Moment::StreakOther));
        for killer in [&me, &h] {
            assert_eq!(
                of(streak(killer, STREAK_SPOKEN - 1)),
                None,
                "a streak from STREAK_SPOKEN"
            );
        }
        let multikill = |killer: &Who| Notable::Multikill {
            killer: killer.clone(),
            count: 3,
            humans: 1,
        };
        assert_eq!(of(multikill(&me)), Some(Moment::Multikill));
        assert_eq!(of(multikill(&h)), Some(Moment::MultikillOther));
        let revenge = |killer: &Who, victim: &Who| Notable::Revenge {
            killer: killer.clone(),
            victim: victim.clone(),
            run: 3,
        };
        assert_eq!(of(revenge(&me, &h)), Some(Moment::Revenge));
        assert_eq!(of(revenge(&h, &me)), None);
        let nemesis = |killer: &Who, victim: &Who| Notable::Nemesis {
            killer: killer.clone(),
            victim: victim.clone(),
            times: 3,
        };
        assert_eq!(of(nemesis(&h, &me)), Some(Moment::Nemesis));
        assert_eq!(of(nemesis(&me, &h)), None, "no gloating");
        let crowbar = |killer: &Who, victim: &Who| Notable::Humiliation {
            killer: killer.clone(),
            victim: victim.clone(),
        };
        assert_eq!(of(crowbar(&h, &me)), Some(Moment::Crowbarred));
        assert_eq!(of(crowbar(&me, &h)), Some(Moment::CrowbarKill));
        assert_eq!(of(crowbar(&h, &other)), None);
        let blast = |victim: &Who| Notable::OwnBlast {
            victim: victim.clone(),
            weapon: "satchel".into(),
        };
        assert_eq!(of(blast(&me)), Some(Moment::OwnBlast));
        assert_eq!(of(blast(&h)), Some(Moment::OwnBlastOther));
        assert_eq!(
            of(Notable::RageQuit {
                who: h.clone(),
                deaths: 3
            }),
            None
        );
        for (trigger, moment) in [
            (Trigger::Joined { who: h.clone() }, Some(Moment::Greet)),
            (
                Trigger::Greeted {
                    from: h.clone(),
                    text: "прив всем".into(),
                },
                Some(Moment::Greet),
            ),
            (Trigger::KilledWhileTyping { killer: None }, Some(Moment::KilledTyping)),
            (Trigger::LastLevel, Some(Moment::LastLevel)),
            (
                Trigger::MatchEnd {
                    winner: Some(me.clone()),
                    won: true,
                },
                Some(Moment::Win),
            ),
            (
                Trigger::MatchEnd {
                    winner: Some(h.clone()),
                    won: false,
                },
                Some(Moment::Gg),
            ),
            (
                Trigger::MatchEnd {
                    winner: Some(other.clone()),
                    won: false,
                },
                None,
            ),
            (
                Trigger::MatchEnd {
                    winner: None,
                    won: false,
                },
                None,
            ),
            (
                Trigger::Addressed {
                    from: h.clone(),
                    text: "gg".into(),
                },
                None,
            ),
            (
                Trigger::Continued {
                    from: h.clone(),
                    text: "gg".into(),
                },
                None,
            ),
            (
                Trigger::Question {
                    from: h.clone(),
                    text: "где все?".into(),
                    to_me: false,
                },
                None,
            ),
            (
                Trigger::Overheard {
                    from: h.clone(),
                    text: "gg".into(),
                    about_bots: false,
                },
                None,
            ),
        ] {
            assert_eq!(key(&trigger, id), moment, "{trigger:?}");
        }
    }

    const BOOK: &str = "\
phrases:
  ru:
    greet: [\"привет, {name}\"]
    win: [\"gg, спасибо\", изи, \"ну наконец-то, gg\"]
  en:
    win: [gg all]
bots:
  - name: Plutonium
    phrases:
      ru:
        win: [реактор доволен, \"ИЗИ \"]
      de:
        win: [gut gemacht]
  - name: \"[B] Kleiner\"
    replace: true
    phrases:
      ru:
        win: [эксперимент удался]
        gg: []
";

    #[test]
    fn a_bot_s_pool() {
        let f = file(BOOK);
        assert_eq!(
            texts(&pool(&f, "ru", "Plutonium", "x", Moment::Win)),
            [
                ("реактор доволен", true),
                ("ИЗИ", true),
                ("gg, спасибо", false),
                ("ну наконец-то, gg", false)
            ],
            "its own first; the common изи is the same phrase"
        );
        assert_eq!(
            texts(&pool(&f, "ru", "Somebody", "[b] kleiner", Moment::Win)),
            [("эксперимент удался", true)],
            "by nickname, its own alone"
        );
        assert!(pool(&f, "ru", "Somebody", "[B] Kleiner", Moment::Gg).is_empty());
        assert_eq!(
            texts(&pool(&f, "ru", "Somebody", "[B] Kleiner", Moment::Greet)),
            [("привет, {name}", false)],
            "a moment it does not list keeps every bot's"
        );
        assert_eq!(texts(&pool(&f, "EN", "Kleiner", "x", Moment::Win)), [("gg all", false)]);
        assert!(pool(&f, "ru", "Plutonium", "x", Moment::Nemesis).is_empty());

        let mut req = request(Trigger::LastLevel);
        let lang = |req: &ChatRequest, remembered: &[&str]| language(&f, req, remembered);
        assert_eq!(lang(&req, &[]), "ru");
        req.language = "ru-RU".into();
        assert_eq!(lang(&req, &[]), "ru");
        req.language = "de".into();
        assert_eq!(lang(&req, &[]), "de", "Plutonium has its own");
        req.bot.persona = "Kleiner".into();
        req.bot.name = "Kleiner".into();
        assert_eq!(lang(&req, &[]), "en", "nobody has German phrases for Kleiner");
        let mut req = request(Trigger::Greeted {
            from: gordon(),
            text: "hi all".into(),
        });
        assert_eq!(lang(&req, &[]), "en", "an English writer");
        req.trigger = Trigger::Joined { who: gordon() };
        assert_eq!(lang(&req, &[]), "ru");
        assert_eq!(lang(&req, &["i cant read cyrillic"]), "en", "the memory tells");
        req.chat = vec![Recent {
            age: 10.0,
            event: Event::Chat {
                from: gordon(),
                text: "where is everyone".into(),
                team: false,
            },
        }];
        assert_eq!(lang(&req, &[]), "en", "the chat tells");
    }

    #[test]
    fn filling_in() {
        let values = Fill {
            name: Some("{map}".into()),
            count: Some(12),
            weapon: Some("сачелем".into()),
            map: Some("crossfire".into()),
            max_chars: 40,
        };
        assert_eq!(
            fill("{name}, уже {count} на {map}", &values).as_deref(),
            Some("{map}, уже 12 на crossfire"),
            "a name is never filled in again"
        );
        assert_eq!(
            fill("  сам себя {weapon} ", &values).as_deref(),
            Some("сам себя сачелем")
        );
        assert_eq!(fill("gg", &values).as_deref(), Some("gg"));
        let none = Fill {
            max_chars: 40,
            ..Fill::default()
        };
        for template in ["gg {name}", "уже {count}", "сам себя {weapon}", "{map} моя"] {
            assert_eq!(fill(template, &none), None, "{template}");
        }
        assert_eq!(fill("{who} здесь", &values), None);
        assert_eq!(fill("{name", &values), None);
        assert!(fill(&"я".repeat(40), &values).is_some());
        assert_eq!(fill(&"я".repeat(41), &values), None);
        assert_eq!(fill("привет, {name}!!!!!!!!!!!!!!!!!!!!!!!!!!!!", &values), None);
    }

    #[test]
    fn names_counts_and_weapons_of_a_moment() {
        let (me, h) = (bot(), gordon());
        let of = |trigger: Trigger, lang: Lang, aliases: &Aliases| Fill::of(&request(trigger), lang, aliases);
        let none = Aliases::default();
        let joined = |name: &str| Trigger::Joined {
            who: who(3, name, false),
        };
        assert_eq!(
            of(joined("[N] C o B A"), Lang::Ru, &none).name.as_deref(),
            Some("C o B A")
        );
        let mut aliases = Aliases::default();
        aliases.insert("[N] C o B A", &["Сова", "Совушка"]);
        assert_eq!(
            of(joined("[N] C o B A"), Lang::Ru, &aliases).name.as_deref(),
            Some("Сова")
        );
        for name in ["FUCK_THIS_GAME_noob", "xoxol_007", "Пидор228"] {
            assert_eq!(of(joined(name), Lang::Ru, &none).name, None, "{name}");
        }
        for name in ["pedik777", "xX_Piderok_Xx", "Pid0r_99", "suka1337", "h0h0l_x"] {
            assert_eq!(of(joined(name), Lang::Ru, &none).name, None, "{name}");
        }
        for name in ["Spiderman", "pedikur_pro", "Nordwind"] {
            assert_eq!(of(joined(name), Lang::Ru, &none).name.as_deref(), Some(name));
        }
        let gg = |winner: Who| Trigger::MatchEnd {
            winner: Some(winner),
            won: false,
        };
        assert_eq!(of(gg(h.clone()), Lang::Ru, &none).name.as_deref(), Some("Gordon"));
        assert_eq!(of(gg(who(3, "Kleiner", true)), Lang::Ru, &none).name, None, "a bot won");
        assert_eq!(
            of(Trigger::KilledWhileTyping { killer: None }, Lang::Ru, &none).name,
            None
        );
        let f = of(
            Trigger::Notable(Notable::Nemesis {
                killer: h.clone(),
                victim: me.clone(),
                times: 5,
            }),
            Lang::Ru,
            &none,
        );
        assert_eq!(
            (f.name.as_deref(), f.count, f.map.as_deref(), f.max_chars),
            (Some("Gordon"), Some(5), Some("crossfire"), 56)
        );
        let f = of(
            Trigger::Notable(Notable::Streak {
                killer: me.clone(),
                count: 10,
                humans: 4,
            }),
            Lang::Ru,
            &none,
        );
        assert_eq!((f.name, f.count), (None, Some(10)));
        let blast = |weapon: &str, lang: Lang| {
            of(
                Trigger::Notable(Notable::OwnBlast {
                    victim: h.clone(),
                    weapon: weapon.into(),
                }),
                lang,
                &none,
            )
            .weapon
        };
        assert_eq!(blast("satchel", Lang::Ru).as_deref(), Some("сачелем"));
        assert_eq!(blast("satchel", Lang::En).as_deref(), Some("with a satchel"));
        assert_eq!(blast("trigger_hurt", Lang::Ru), None);
        assert_eq!(blast("world", Lang::Ru), None);
    }

    #[test]
    fn common_phrases_in_the_bot_s_manner() {
        let mut rng = Pcg32::new(1, 2);
        assert_eq!(styled("Хорошая игра.", 0, Lang::Ru, &mut rng), "хорошая игра");
        assert_eq!(styled("GG, {name}.", 2, Lang::Ru, &mut rng), "gg, {name}");
        assert_eq!(styled("ну и ну..", 5, Lang::Ru, &mut rng), "ну и ну..");
        assert_eq!(styled("Ну…", 0, Lang::Ru, &mut rng), "ну…");
        assert_eq!(
            styled("GG WP.", 3, Lang::Ru, &mut rng),
            "GG WP.",
            "manner 3 writes as the phrase"
        );
        assert_eq!(styled("GG WP.", 8 + 3, Lang::Ru, &mut rng), "GG WP.");
        let smiles = |text: &str, lang: Lang, smile: &str| {
            (0..200)
                .filter(|&seed| {
                    let out = styled(text, 1, lang, &mut Pcg32::new(seed, 3));
                    assert!(
                        out == text.to_lowercase() || out == format!("{}{smile}", text.to_lowercase()),
                        "{out}"
                    );
                    out.ends_with(smile)
                })
                .count()
        };
        let n = smiles("Привет, {name}", Lang::Ru, "))");
        assert!((60..=140).contains(&n), "{n} of 200");
        let n = smiles("hey {name}", Lang::En, " :)");
        assert!((60..=140).contains(&n), "{n} of 200");
        for text in ["ну наконец)", "ой(", "gg!", "кто следующий?"] {
            assert_eq!(smiles(text, Lang::Ru, "))"), 0, "{text}");
        }
    }

    fn candidates(texts: &[&str]) -> Vec<Candidate> {
        texts
            .iter()
            .map(|t| Candidate {
                phrase: Phrase {
                    text: t.to_string(),
                    own: false,
                },
                filled: t.to_string(),
            })
            .collect()
    }

    #[test]
    fn the_ring_keeps_the_last_thirty() {
        let mut rng = Pcg32::new(9, 1);
        let mut ring = Ring::default();
        let three = candidates(&["a", "b", "c"]);
        let mut picked: Vec<&str> = (0..3)
            .map(|_| ring.pick(&three, &mut rng).unwrap().filled.as_str())
            .collect();
        let first = picked[0];
        picked.sort_unstable();
        assert_eq!(picked, ["a", "b", "c"], "none twice while others are left");
        assert_eq!(
            ring.pick(&three, &mut rng).unwrap().filled,
            first,
            "then the one picked longest ago"
        );
        assert!(ring.pick(&[], &mut rng).is_none());

        let mut ring = Ring::default();
        let names: Vec<String> = (0..=RING).map(|i| format!("фраза {i}")).collect();
        let all = candidates(&names.iter().map(String::as_str).collect::<Vec<_>>());
        let mut order: Vec<String> = (0..RING)
            .map(|_| ring.pick(&all, &mut rng).unwrap().filled.clone())
            .collect();
        let last = ring.pick(&all, &mut rng).unwrap().filled.clone();
        assert!(!order.contains(&last), "the one left");
        assert_eq!(
            ring.pick(&all, &mut rng).unwrap().filled,
            order[0],
            "the ring keeps {RING}: the first pick is out of it"
        );
        order.sort();
        order.dedup();
        assert_eq!(order.len(), RING);
        assert_eq!(
            ring.pick(&candidates(&["  ФРАЗА 5 "]), &mut rng).unwrap().filled,
            "  ФРАЗА 5 ",
            "phrases are the same in any case"
        );
        assert_eq!(ring.when("фраза 5"), Some(RING - 1));
    }

    #[test]
    fn an_offer_for_a_moment() {
        let f = file(
            "\
phrases:
  ru:
    gg: [\"gg, {name}\", Поздравляю., \"{name}, браво\"]
  en:
    gg: [\"gg {name}\"]
bots:
  - name: Plutonium
    phrases:
      ru:
        gg: [Реактор Доволен.]
",
        );
        let mut req = request(Trigger::MatchEnd {
            winner: Some(gordon()),
            won: false,
        });
        req.chat = vec![Recent {
            age: 3.0,
            event: Event::Chat {
                from: who(3, "Kleiner", true),
                text: "GG, Gordon))".into(),
                team: false,
            },
        }];
        let offer = Offer::of(&f, &req, &Aliases::default(), &[]).unwrap();
        assert_eq!((offer.moment, offer.language.as_str()), (Moment::Gg, "ru"));
        let filled: Vec<&str> = offer.candidates.iter().map(|c| c.filled.as_str()).collect();
        assert_eq!(
            filled,
            ["Реактор Доволен.", "Поздравляю.", "Gordon, браво"],
            "a bot said gg, Gordon already"
        );
        let mut echo = req.clone();
        echo.chat.push(Recent {
            age: 2.0,
            event: Event::Chat {
                from: who(4, "Barney", false),
                text: "поздравляю!".into(),
                team: false,
            },
        });
        let offer_echo = Offer::of(&f, &echo, &Aliases::default(), &[]).unwrap();
        assert_eq!(offer_echo.candidates.len(), 2, "nor what a player just wrote");
        let mut ring = Ring::default();
        let mut rng = Pcg32::new(req.id, 0);
        let lines: Vec<String> = (0..3)
            .map(|_| offer.pick(&mut ring, &req.bot, &mut rng).unwrap())
            .collect();
        for line in ["Реактор Доволен.", "поздравляю", "Gordon, браво"] {
            assert!(lines.iter().any(|l| l == line), "{line} in {lines:?}");
        }
        req.bot.manner_text = Some("как получится".into());
        let mut ring = Ring::default();
        let lines: Vec<String> = (0..3)
            .map(|_| offer.pick(&mut ring, &req.bot, &mut rng).unwrap())
            .collect();
        assert!(
            lines.iter().any(|l| l == "Поздравляю."),
            "a style of its own: {lines:?}"
        );

        req.trigger = Trigger::MatchEnd {
            winner: None,
            won: false,
        };
        assert!(Offer::of(&f, &req, &Aliases::default(), &[]).is_none(), "nobody won");
        req.trigger = Trigger::MatchEnd {
            winner: Some(who(5, "pedik777", false)),
            won: false,
        };
        let offer = Offer::of(&f, &req, &Aliases::default(), &[]).unwrap();
        assert_eq!(offer.candidates.len(), 2, "no name: no phrase that needs one");
        req.trigger = Trigger::MatchEnd {
            winner: Some(gordon()),
            won: false,
        };
        req.max_chars = 12;
        let offer = Offer::of(&f, &req, &Aliases::default(), &[]).unwrap();
        assert_eq!(offer.candidates[0].filled, "Поздравляю.");
        assert_eq!(offer.candidates.len(), 1, "the rest are too long");
        req.max_chars = 8;
        assert!(Offer::of(&f, &req, &Aliases::default(), &[]).is_none());
        req.max_chars = 56;
        req.chat.clear();
        req.talk = vec![Said {
            age: 400.0,
            mine: false,
            text: "where is everyone".into(),
        }];
        let offer = Offer::of(&f, &req, &Aliases::default(), &[]).unwrap();
        assert_eq!(
            (offer.language.as_str(), offer.candidates[0].filled.as_str()),
            ("en", "gg Gordon")
        );
        req.trigger = Trigger::Addressed {
            from: gordon(),
            text: "gg".into(),
        };
        assert!(Offer::of(&f, &req, &Aliases::default(), &[]).is_none(), "no moment");
        req.trigger = Trigger::LastLevel;
        assert!(
            Offer::of(&f, &req, &Aliases::default(), &[]).is_none(),
            "no phrase of it"
        );
    }

    /// Every shipped phrase fills in both languages, with each weapon a player can blow themselves up with and a long
    /// name, within the 56 characters a bot with a 13-letter nickname may say in Russian.
    #[test]
    fn the_shipped_phrases_all_fill() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/config/chat/phrases.yaml");
        let text = std::fs::read_to_string(path).expect("data/config/chat/phrases.yaml");
        let f = ChatPhrasesFile::parse(&text, path).unwrap();
        let explosives = [
            "grenade",
            "argrenade",
            "satchel",
            "tripmine",
            "rpg_rocket",
            "snark",
            "gauss",
            "hornet",
        ];
        for code in ["ru", "en"] {
            let lang = Lang::of(code);
            for moment in Moment::ALL {
                let phrases = pool(&f, code, "Plutonium", "[B] Plutonium", moment);
                assert!(phrases.len() >= 8, "{code}.{}: {}", moment.key(), phrases.len());
                let weapons: Vec<Option<String>> = if moment.allows("weapon") {
                    explosives
                        .iter()
                        .inspect(|w| assert!(lang::is_explosive(w)))
                        .map(|w| Some(lang::weapon_phrase(lang, w)))
                        .collect()
                } else {
                    vec![None]
                };
                for p in &phrases {
                    assert!(
                        p.text.chars().count() <= PHRASE_MAX,
                        "{code}.{}: {}",
                        moment.key(),
                        p.text
                    );
                    for weapon in &weapons {
                        let values = Fill {
                            name: Some("Сто двенадцать".into()),
                            count: Some(12),
                            weapon: weapon.clone(),
                            map: Some("gg_cold_rock".into()),
                            max_chars: 56,
                        };
                        let line = fill(&p.text, &values);
                        assert!(
                            line.as_deref().is_some_and(|l| !l.contains(['{', '}'])),
                            "{code}.{}: {} with {weapon:?}",
                            moment.key(),
                            p.text
                        );
                    }
                }
            }
        }
    }
}
