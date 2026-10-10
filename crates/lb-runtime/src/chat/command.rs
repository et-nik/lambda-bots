//! `lb chat`: what the chat does, its log, lines, moments and tests by hand, the memory of players.

use lb_chat::botchat::Phase;
use lb_chat::{Notable, Trigger, Who, sanitize};
use lb_config::chat_phrases::Moment;
use lb_host::Host;

use super::{ChatLog, Job};
use crate::Runtime;
use crate::cvars::Cv;

/// The bot `name` names: `#userid`, its personality or in-game name, or the start of one.
fn find(rt: &Runtime, name: &str) -> Option<usize> {
    if let Some(id) = name.strip_prefix('#') {
        let id: i32 = id.parse().ok()?;
        return rt.bots.iter().position(|b| b.userid == id);
    }
    let netname = |i: usize| rt.clients.get(rt.bots[i].id.slot).map_or("", |c| c.name.as_str());
    let lower = name.to_lowercase();
    (0..rt.bots.len())
        .find(|&i| rt.bots[i].persona.name.eq_ignore_ascii_case(name) || netname(i).eq_ignore_ascii_case(name))
        .or_else(|| {
            let mut starts = (0..rt.bots.len()).filter(|&i| netname(i).to_lowercase().starts_with(&lower));
            match (starts.next(), starts.next()) {
                (Some(i), None) => Some(i),
                _ => None,
            }
        })
}

/// The words of a line as the console split them, put back together: it cuts `,` `:` `(` `)` `'` off as words of
/// their own.
fn words(args: &[&str]) -> String {
    let mut out = String::new();
    for w in args {
        let glued = matches!(*w, "," | ":" | ")" | "'") || out.ends_with(['(', '\'']);
        if !out.is_empty() && !glued {
            out.push(' ');
        }
        out.push_str(w);
    }
    out
}

/// Who writes in `lb chat test`: the player giving the command, else "admin".
fn tester(rt: &Runtime) -> Who {
    match rt.command_slot.and_then(|s| rt.clients.get(s).map(|c| (s, c))) {
        Some((slot, c)) => Who {
            slot,
            userid: c.userid,
            name: c.name.clone(),
            bot: false,
        },
        None => Who {
            slot: 0,
            userid: -1,
            name: "admin".into(),
            bot: false,
        },
    }
}

/// The trigger of `moment` for the bot `me`; `other` is the other one in it.
fn moment_trigger(moment: Moment, me: Who, other: Who) -> Trigger {
    let weapon = "satchel".to_string();
    match moment {
        Moment::Greet => Trigger::Joined { who: other },
        Moment::Streak => Trigger::Notable(Notable::Streak {
            killer: me,
            count: 10,
            humans: 10,
        }),
        Moment::StreakOther => Trigger::Notable(Notable::Streak {
            killer: other,
            count: 10,
            humans: 0,
        }),
        Moment::Multikill => Trigger::Notable(Notable::Multikill {
            killer: me,
            count: 3,
            humans: 3,
        }),
        Moment::MultikillOther => Trigger::Notable(Notable::Multikill {
            killer: other,
            count: 3,
            humans: 0,
        }),
        Moment::Revenge => Trigger::Notable(Notable::Revenge {
            killer: me,
            victim: other,
            run: 3,
        }),
        Moment::Nemesis => Trigger::Notable(Notable::Nemesis {
            killer: other,
            victim: me,
            times: 3,
        }),
        Moment::Crowbarred => Trigger::Notable(Notable::Humiliation {
            killer: other,
            victim: me,
        }),
        Moment::CrowbarKill => Trigger::Notable(Notable::Humiliation {
            killer: me,
            victim: other,
        }),
        Moment::OwnBlast => Trigger::Notable(Notable::OwnBlast { victim: me, weapon }),
        Moment::OwnBlastOther => Trigger::Notable(Notable::OwnBlast { victim: other, weapon }),
        Moment::KilledTyping => Trigger::KilledWhileTyping { killer: Some(other) },
        Moment::LastLevel => Trigger::LastLevel,
        Moment::Win => Trigger::MatchEnd {
            winner: Some(me),
            won: true,
        },
        Moment::Gg => Trigger::MatchEnd {
            winner: Some(other),
            won: false,
        },
    }
}

/// Whether requests are written down, and where today.
fn transcript_line(rt: &Runtime) -> String {
    let path = super::transcript::today(&rt.init.install_dir.join("logs"));
    if rt.config.chat.transcript {
        format!("transcript: on, {}", path.display())
    } else {
        "transcript: off (`lb chat transcript on`)".into()
    }
}

fn status(rt: &Runtime) -> Vec<String> {
    let c = &rt.config.chat;
    let n = rt.chat.counts;
    let mut out = vec![format!(
        "chat {} (lb_chat), language {}, {} {}; humans on the server: {}",
        if c.enabled { "on" } else { "off" },
        c.language,
        match c.provider.kind {
            lb_config::main_config::ProviderKind::Anthropic => "anthropic",
            lb_config::main_config::ProviderKind::Openai => "openai",
        },
        c.provider.model,
        rt.clients.humans(false)
    )];
    let worker = rt.chat.backend.status();
    let mut worker = worker.lines();
    out.push(format!("worker: {}", worker.next().unwrap_or_default()));
    out.extend(worker.map(str::to_string));
    out.extend([
        transcript_line(rt),
        ChatLog::status(rt.chat.chatlog.as_ref(), c.chatlog, c.chatlog_days),
        format!(
            "this session: {} asked: {} model lines, {} phrases, {} kept quiet, {} failed; {} said",
            n.asked, n.lines, n.phrases, n.silent, n.failed, n.said
        ),
    ]);
    let talks: Vec<String> = rt
        .chat
        .social
        .talks
        .threads()
        .iter()
        .filter(|t| t.active(rt.now))
        .map(|t| format!("{} with {} ({:.0} s ago)", t.bot, t.name, rt.now.since(t.last)))
        .collect();
    if !talks.is_empty() {
        out.push(format!("talks: {}", talks.join(", ")));
    }
    if rt.chat.director.has_waiting() {
        out.push("answers wait for their bot".into());
    }
    for bot in &rt.bots {
        let state = match bot.chat.phase() {
            Phase::Idle => continue,
            Phase::Asked { .. } => "waiting for its line".to_string(),
            Phase::Ready { text } => format!("about to type: {text}"),
            Phase::Typing { text, done, .. } => format!(
                "typing{} ({:.1} s left): {text}",
                if bot.chat.typing_in_the_open() {
                    ", standing"
                } else {
                    ""
                },
                done.since(rt.now).max(0.0)
            ),
        };
        out.push(format!("  {}: {state}", bot.persona.name));
    }
    out
}

pub(crate) fn command(rt: &mut Runtime, host: &mut dyn Host, args: &[&str]) -> Vec<String> {
    let usage = || {
        vec![
            "lb chat [status] | log [n] | say <bot> <text> | test <bot> <text> | event <bot> <moment> | \
             prompt <bot> [text] | memory <player> [forget] | transcript [on|off] | reload | on | off"
                .to_string(),
        ]
    };
    match args {
        [] | ["status"] => status(rt),
        ["log", rest @ ..] => {
            let n = rest.first().and_then(|n| n.parse::<usize>().ok()).unwrap_or(20);
            let skip = rt.chat.log.len().saturating_sub(n);
            let mut out: Vec<String> = rt.chat.log.iter().skip(skip).cloned().collect();
            if out.is_empty() {
                out.push("chat: nothing yet".into());
            }
            out
        }
        ["transcript"] => vec![transcript_line(rt)],
        ["transcript", "on" | "off"] => {
            rt.config.chat.transcript = args[1] == "on";
            rt.chat.backend.send(Job::Configure(Box::new(rt.config.chat.clone())));
            vec![transcript_line(rt)]
        }
        ["on" | "off"] => {
            let on = args[0] == "on";
            rt.cvars.set(host, Cv::Chat, if on { "1" } else { "0" });
            if on != rt.config.chat.enabled {
                rt.config.chat.enabled = on;
                if on {
                    rt.chat_sync_backend();
                } else {
                    rt.chat_switched_off();
                }
            }
            vec![format!("chat {}", if on { "on" } else { "off" })]
        }
        ["say", bot, text @ ..] if !text.is_empty() => {
            let Some(i) = find(rt, bot) else {
                return vec![format!("chat: no bot `{bot}`")];
            };
            let slot = rt.bots[i].id.slot;
            let budget = rt.chat_budget(slot, false);
            let ascii = rt.chat_ascii_needed();
            let Some(line) = sanitize::fit_say(&words(text), budget, &rt.config.chat.blocked, ascii) else {
                return vec!["chat: nothing left to say after cleaning the line up".into()];
            };
            let now = rt.now;
            rt.chat.unsay(|p| p.slot == slot);
            let bot = &mut rt.bots[i];
            bot.chat.set_line(line.clone(), now, super::KEEP_ANSWER);
            vec![format!("chat: {} will type `{line}`", bot.persona.name)]
        }
        ["test", bot, text @ ..] if !text.is_empty() => {
            if !rt.config.chat.enabled {
                return vec!["chat is off: `lb chat on` first".into()];
            }
            let Some(i) = find(rt, bot) else {
                return vec![format!("chat: no bot `{bot}`")];
            };
            let (slot, name) = (rt.bots[i].id.slot, rt.bots[i].persona.name.clone());
            let from = tester(rt);
            rt.chat_test(slot, from, words(text));
            vec![format!(
                "chat: asked {name} for an answer; see `lb chat status` and `lb chat log`"
            )]
        }
        ["event", bot, key] => {
            if !rt.config.chat.enabled {
                return vec!["chat is off: `lb chat on` first".into()];
            }
            let Some(moment) = Moment::parse(key) else {
                let keys: Vec<&str> = Moment::ALL.iter().map(|m| m.key()).collect();
                return vec![format!("chat: no moment `{key}`; the moments: {}", keys.join(", "))];
            };
            let Some(i) = find(rt, bot) else {
                return vec![format!("chat: no bot `{bot}`")];
            };
            let b = &rt.bots[i];
            let (slot, name) = (b.id.slot, b.persona.name.clone());
            let me = Who {
                slot,
                userid: b.userid,
                name: rt.clients.get(slot).map_or_else(|| name.clone(), |c| c.name.clone()),
                bot: true,
            };
            let trigger = moment_trigger(moment, me, tester(rt));
            rt.chat_event(slot, trigger);
            vec![format!(
                "chat: asked {name} to speak of {key}; see `lb chat status` and `lb chat log`"
            )]
        }
        ["prompt", bot, text @ ..] => {
            let Some(i) = find(rt, bot) else {
                return vec![format!("chat: no bot `{bot}`")];
            };
            if !rt.chat.backend.live() {
                return vec!["chat: no worker runs (chat is off, or this is a replay)".into()];
            }
            let slot = rt.bots[i].id.slot;
            let text = if text.is_empty() {
                "привет".to_string()
            } else {
                words(text)
            };
            let from = tester(rt);
            rt.chat_preview(slot, from, text);
            vec!["chat: the worker prints the prompt to the server console and the log".into()]
        }
        ["memory", rest @ ..] if !rest.is_empty() => {
            let forget = rest.last() == Some(&"forget") && rest.len() > 1;
            let query = if forget { &rest[..rest.len() - 1] } else { rest }.join(" ");
            if !rt.chat.backend.live() {
                return vec!["chat: no worker runs (chat is off, or this is a replay)".into()];
            }
            rt.chat.backend.send(Job::Memory { query, forget });
            vec!["chat: the worker prints the answer to the server console".into()]
        }
        ["reload"] => {
            if !rt.chat.backend.live() {
                return vec![
                    "chat: no worker runs (chat is off, or this is a replay); config/chat/ is read when chat starts"
                        .into(),
                ];
            }
            rt.chat.backend.send(Job::Reload);
            vec![
                "chat: the worker reads config/chat/*.yaml again and retries the provider; what it read goes to the \
                 server console and the log"
                    .into(),
            ]
        }
        _ => usage(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn console_words_glue_back() {
        assert_eq!(words(&["привет", ",", "как", "дела"]), "привет, как дела");
        assert_eq!(words(&["ну", "(", "бот", ")", ":", "ок"]), "ну (бот): ок");
        assert_eq!(words(&["it", "'", "s", "ok"]), "it's ok");
    }

    #[test]
    fn every_moment_comes_as_its_own() {
        let who = |userid, bot| Who {
            slot: 1,
            userid,
            name: if bot { "Kleiner" } else { "Gordon" }.into(),
            bot,
        };
        for moment in Moment::ALL {
            let trigger = moment_trigger(moment, who(11, true), who(3, false));
            assert_eq!(lb_chat::phrases::key(&trigger, 11), Some(moment), "{trigger:?}");
        }
    }
}
