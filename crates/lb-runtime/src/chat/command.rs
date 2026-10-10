//! `lb chat`: what the chat does, its log, lines and tests by hand, the memory of players.

use lb_chat::botchat::Phase;
use lb_chat::{Who, sanitize};
use lb_host::Host;

use super::Job;
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
    let mut out = vec![
        format!(
            "chat {} (lb_chat), language {}, {} {}; humans on the server: {}",
            if c.enabled { "on" } else { "off" },
            c.language,
            match c.provider.kind {
                lb_config::main_config::ProviderKind::Anthropic => "anthropic",
                lb_config::main_config::ProviderKind::Openai => "openai",
            },
            c.provider.model,
            rt.clients.humans(false)
        ),
        format!("worker: {}", rt.chat.backend.status()),
        transcript_line(rt),
        format!(
            "this session: {} asked, {} said, {} kept quiet, {} failed",
            n.asked, n.said, n.silent, n.failed
        ),
    ];
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
            "lb chat [status] | log [n] | say <bot> <text> | test <bot> <text> | prompt <bot> [text] | \
             memory <player> [forget] | transcript [on|off] | reload | on | off"
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
            rt.chat.backend.send(Job::Reload);
            vec!["chat: the worker reads the notes on players again and retries the provider".into()]
        }
        _ => usage(),
    }
}

#[cfg(test)]
mod tests {
    use super::words;

    #[test]
    fn console_words_glue_back() {
        assert_eq!(words(&["привет", ",", "как", "дела"]), "привет, как дела");
        assert_eq!(words(&["ну", "(", "бот", ")", ":", "ок"]), "ну (бот): ок");
        assert_eq!(words(&["it", "'", "s", "ok"]), "it's ok");
    }
}
