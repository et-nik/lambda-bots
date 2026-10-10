# Bots in the chat

Bots talk in the game chat the way players do, through a chat model: the Anthropic Messages API or any
OpenAI-compatible chat completions endpoint (OpenAI, OpenRouter, vLLM, Ollama, llama.cpp, a gateway of your own).
They answer players who speak to them and keep the talk going, and now and then say something about the game: a word
to a player who just joined, gg when a match ends, a complaint about a nemesis, a laugh at their own satchel. Game
moments mostly get ready phrases, which cost nothing and wait for no model; the model writes the rest. The bots
remember the players they meet and what they talked about, between maps and restarts.

Chat is off until `chat.enabled: true` (or `lb_chat 1`), and nothing is sent while no human is on the server. The
chat log, every line of the game chat in `logs/chatlog.<date>.log`, is written whether chat is on or off.

## How it plays

- **Who answers.** A player who names a bot (`Атлас` for `DUT9 ATLASA`, `klein` for `Kleiner`, `плутоныч` for
  `Plutonium`, in either script, or a name from its profile's `chat.call`) or speaks to "the bots" (`боты, вы где?`)
  is answered almost always, by two bots at most. A question to a bot (`ты где?`, `you there?`) is answered most of
  the time when the player has just fought or talked with it, less often otherwise; a question to everybody, now and
  then. A line to everybody, a player joining, the end of a match and game moments get a word now and then, as often
  as a bot's chattiness says. Bots never answer bots, and moments among bots alone get nothing.
- **Talks.** A player who names a bot, asks it something or gets its answer is talking with it: the next lines go to
  that bot without its name while less than 150 s pass between them, across a map change too. A bot named or asked
  while it types or waits for its line answers once it is free, within 30 s; a newer line of the same player replaces
  the waiting one. What they said goes to the memory, and a talk picked up days later comes with it.
- **Left alone.** `бот` in the third person (`ты че с ботом разговариваешь`) is players talking among themselves: a
  bot rarely joins in. A line that names another player on the server is left to them. Noise gets nothing: laughter,
  smileys and lines of only signs, `%l`-style macros, map names, binds in Latin capitals (`HEADSHOT!!!`), a lone short
  word outside a talk. Nor does a repeat: the same words answered in the last 5 minutes, the bot's name or not, or
  sent within 3 hours without it. Swearing, insults, nations and politics get an answer only when they name a bot and
  no other player. A line in a language that is neither the server's nor English gets none at all.
- **How much.** All bots together say at most `limits.lines_per_minute` lines a minute (2 by default) and
  `limits.remarks_per_hour` lines nobody asked for an hour (6): game moments, greetings, comments on lines meant for
  everybody. gg at the end of a match and answers to players who name a bot, talk with it or ask a question are not
  counted. Between two lines nobody asked for pass `limits.remark_gap` seconds, and `limits.bot_remark_gap` for one
  bot; a player's lines to everybody get one such answer in 4 minutes at most. A line not said (the model wrote `-`,
  the filter dropped it, it came too late) costs nothing.
- **Greetings.** A player who joins may get a word, at most once in 45 minutes for one nickname: a player back after a
  map change or a short break is not greeted again. A player's own hi to everybody (`прив`, `hi all`) may get one
  back from one bot, under the same rule. Players the bots know (from the memory or `players.yaml`) are greeted by the
  model, anyone else with a ready phrase. If the player answers the greeting within 45 s, the bot goes on talking.
- **Typing.** A line takes as long as a player's would: noticing and reading what it answers, thinking (short lines
  come at once), typing at the personality's speed (200–380 characters a minute for personalities without their own,
  `typing.cpm`). An alive bot types only in a calm moment (no enemy seen or heard for `typing.calm` seconds, not
  fighting, on the ground) and stands still while it types, as a player whose keyboard is in the chat input. An enemy
  in sight or close, or damage, makes it drop the chat and fight; it types the line again later, once. A dead bot
  types at once and holds its respawn until the line is out, at most 5 s (about when the game respawns it anyway); a
  bot respawned mid-line finishes it standing, unless a fight comes first. At the end of a match (the intermission, or
  GunGame's frozen players) everyone types freely.
- **Language.** Lines are in `chat.language` (Russian by default). A player who writes in English gets English: the
  model is told so, and phrases come from the `en` section. A line in any other language gets no answer; one that
  tells nothing by itself (`gg`, `ok`, `)))`) goes by the player's earlier lines.
- **Tone.** Friendly teasing, never mean; slang for everyone, swearing only for personalities with
  `chat.profanity: true`. The rules tell the model not to accuse anyone of cheating, even as a joke; not to repeat
  swearing and insults from the chat or the memory, nor side with them; not to pick on newcomers (no `мясо`, `нуб`);
  to laugh off anger rather than snap back; to be kind about sad or serious things; not to mock how someone writes;
  not to talk about players who left; to use what it knows of the map and itself in answers, not to recite it. Never:
  insults about nationality, religion, sex or orientation, threats, players' real lives, politics, server commands.
  Bots never claim to be human; asked sincerely whether they are bots, they do not deny it (they may joke about it).
- **The filter.** What the model wrote is checked before the bot types it. A line with swearing is dropped for a bot
  without `chat.profanity`, a slur for every bot, and a cheating accusation unless the player it answers spoke of
  cheats first. A dropped line is silence; the log and the transcript say `dropped (swearing)`, `dropped (slur)` or
  `dropped (cheating)`. Ready phrases are yours and go as written, but a nickname with swearing never goes into one.
- **GunGame.** The crowbar is a level's weapon there, so a crowbar kill is no humiliation: the bots neither crow nor
  grumble about it. A bot reaching the last level may say so.
- **Team chat.** In team modes a bot hears its own team's `say_team` and answers there. Outside them `say_team` shows
  only to its writer, so the bots leave it alone; it goes to the chat log only.
- **The line itself.** One line, cut to what `Host_Say` takes (`123 − name bytes` for `say`), on a character boundary;
  no quotes, `%`, control characters, emoji, command prefixes (`/ ! @ . #`) or a first word in `chat.blocked` (plugin
  commands such as `rtv`). The SDK's `Host_Say` before 2023 drops a line without an ASCII character, so with
  `game.dll: classic` pure Cyrillic gets a `)`; BugfixedHL-Rebased, the 2023 update and hlsdk-portable take UTF-8.

## What the model sees

For every line one request. First what is the same for every bot: the rules, then the server (`chat.server`, its
language, your `config/chat/server.yaml`). Then the bot: name, skill, style, favourite weapons, its manner of writing,
its own words (the profile's `chat.about`, then your `config/chat/bots.yaml`), swearing or not. Then the scene:

- the map and mode, with your notes on it from `config/chat/maps.yaml`;
- the minute, the bot's score and GunGame level, the leader, its mood;
- the players on the server and the bot's score against each on this map; what the bots know of them, in full only
  for the player it answers; the last two maps;
- what happened in the game: up to 10 kills and other events of the last 2 minutes;
- its own recent lines (up to 5), so that it neither repeats nor contradicts itself;
- its talk with the player: lines older than the chat below, from the last map, or from days before (the memory),
  each marked with when;
- the chat: up to 12 lines of the last 5 minutes, which kills do not push out; a line sent several times in a row
  shows once, with how many times;
- the reason to speak and, for a player who writes in English while the server speaks another language, a note to
  answer in English.

A line shows once, in the block that fits it best. The prompt is in Russian for `language: ru` and in English for any
other language. `lb chat prompt <bot>` prints it as it would go.

Kill feed, chat, joins and the scoreboard are what every player sees; the request holds nothing a player could not
know, besides your own words. Which players are bots is not said.

## Your words: `config/chat/`

What you want the bots to know goes in `config/chat/`, one file for each kind of thing. The shipped templates hold
commented examples, and `phrases.yaml` the full set of phrases.

| File           | About                                 | Sent                               |
|----------------|---------------------------------------|------------------------------------|
| `players.yaml` | regular players: aliases and notes    | with requests that show the player |
| `server.yaml`  | the server                            | with every request                 |
| `bots.yaml`    | single bots, on top of their profiles | with that bot's requests           |
| `maps.yaml`    | maps                                  | with requests on that map          |
| `phrases.yaml` | ready phrases for game moments        | never: the bots say them (below)   |

The worker reads them when chat starts and again on `lb chat reload` or `lb config reload`, and prints one line a file
(`bots.yaml: 2 bots`, `maps.yaml: not found`, `phrases: built-in`); `lb chat status` shows the summary. A missing
file means nothing to say. A broken one is reported on the server console and in the log and read as empty, except
`phrases.yaml`: it keeps the phrases read before, or the built-in set at start. Check the files with
`lb-cli config check config/` before a reload.

Your text costs tokens in every request it goes with: 100 characters, about 50 tokens.

### `players.yaml`

```yaml
schema: lambdabots/chat-players@1
players:
  - id: STEAM_0:0:219579426      # a SteamID, or the nickname of a player without one (a bot's too)
    name: ATLAS Gamer
    alias: [Атлас, Атласыч]      # what the bots call the player: one name, or several
    note: Хороший игрок, один из лучших. Любит арбалет, после каждой карты пишет gg.
  - id: ET^NiK
    alias: Ник
  - id: "[KZ] Lynx :>"           # in quotes: a leading [ breaks YAML
    by_name: true                # this nickname, whatever the SteamID
    alias: Рысь
```

An alias is what the bots call a player instead of a hard nickname, one name or several (the first is the main one).
The prompt names the player by them everywhere: `Атлас или Атласыч (ник ATLAS Gamer)` in the list of players, where
the bots may mix the names, and the main one, `Атлас`, in the kill feed and the chat. Should the model still write the
whole nickname, the line says the main alias instead. Players without an alias are called briefly too: the rules ask
for nicknames without clan tags and symbols.

With a SteamID as `id` the note and the aliases go to that SteamID only (`name` is for you to read) and stay when the
player changes nickname. A nickname as `id` serves bots and players without a SteamID only; with `by_name: true` it
serves anyone with that nickname, whatever the SteamID: that is for players whose SteamID changes every visit, and
whoever takes the nickname gets the alias and the note. The bots look by SteamID first, then `by_name`, then the plain
nickname. A note is up to 500 characters; up to 8 aliases of up to 32 characters, without quotes, `%` or `;`.

### `server.yaml`

```yaml
schema: lambdabots/chat-server@1
context: |
  GunGame-сервер: на каждом уровне своё оружие, на последнем — лом. Вечером людно, днём почти пусто.
  Карту меняют голосованием (rtv), правила и админы описаны на сайте сервера.
```

The server in your words: its rules, its regulars, what goes on there. Up to 2000 characters; empty for none. It goes
into every request with the rules, after `chat.server`, the one-line name in `lambdabots.yaml`.

### `bots.yaml`

```yaml
schema: lambdabots/chat-bots@1
bots:
  - name: "Plutonium"
    context: |
      Играет здесь с первых дней сервера, всех завсегдатаев знает по никам.
      Любит гаусс и длинные прыжки, к лому относится с уважением.
```

What one bot should know of itself on this server, on top of its profile: who it is here, whom it knows, what it
likes to talk about. `name` is the personality's name as `lb list` shows it, or the bot's nickname in the game (with
`bots.name_prefix`), in any case; an entry for the personality wins over one for the nickname, and a name comes once.
Quote it: names like `[B] …` break YAML otherwise. `context` is up to 1000 characters, given after the profile's
`chat.about` in that bot's requests.

### `maps.yaml`

```yaml
schema: lambdabots/chat-maps@1
maps:
  - map: crossfire
    note: Кнопка в бункере запускает авиаудар по открытой площадке; кто снаружи, тот погибает.
  - map: [gg_*, ag_*]
    note: Маленькие арены для GunGame и дуэлей, всё простреливается.
```

`map` is a map's name, a pattern where `*` stands for any text (`gg_*`; quote one that starts with `*`), or a list of
them, in any case: up to 64 characters, without spaces, `/` or `\`. `note` is up to 1000 characters. A map gets the
notes naming it first, then those whose pattern fits it, each in file order, at most 1500 characters in all: a note
that would go past them is left out.

## Ready phrases

Game moments and greetings mostly get a ready phrase instead of a request: `config/chat/phrases.yaml` has them for
every bot and for single bots, in Russian and English, and the module has the shipped set built in for a server
without the file. A phrase costs no tokens and no request and is picked at once; the bot types it like any line.

- **Phrase or model.** A moment gets one of its phrases unless the model's share comes up (`chat.phrases.ai_share`,
  0.15): then the model says something of its own. Greetings go to the model for players the bots know. Answers to
  players (named, in a talk, questions, lines to everybody) always come from the model. While the model is out of
  reach (no key or a refused one, out of API credit, the day's tokens spent, waiting after failures), moments keep
  their phrases and players get no answers; a moment whose request fails gets a phrase while it is still fresh
  (10 s). A moment without a phrase that fits goes to the model. Phrases need `chat.enabled`;
  `chat.phrases.enabled: false` sends every moment to the model.
- **Language.** A moment with a player who writes in English gets the `en` phrases; any other, those of
  `chat.language`, or `en` when the file has no such section. A moment its section lacks goes to the model.
- **Variety.** The last 30 phrases of all bots are not picked again while others are left, nor is a line already in
  the chat.
- **Manner.** A bot without its own `chat.style` says the common phrases its way: in lower case without a final full
  stop if it writes so, now and then with `))` (` :)` in English) if it likes them. A bot's own phrases are said as
  written.
- **Names.** `{name}` is the other player: the main alias from `players.yaml`, else the nickname without clan tags
  and colour codes (`[B] Lynx` → `Lynx`, `^1Shephard` → `Shephard`). A phrase is skipped when a placeholder has no
  value, when the line comes out longer than the bot may send, and, for `{name}`, when the nickname holds swearing.

The moments, when they come, and the placeholders their phrases take:

| Key               | When                                                                  | Placeholders        |
|-------------------|-----------------------------------------------------------------------|---------------------|
| `greet`           | a player the bots do not know joins, or says hi to everybody          | `{name}`            |
| `streak`          | the bot kills 10 or more without dying, players among them            | `{count}`           |
| `streak_other`    | a player kills 10 or more without dying                               | `{name}` `{count}`  |
| `multikill`       | the bot kills 3 or more within 6 s, players among them                | `{count}`           |
| `multikill_other` | a player kills 3 or more within 6 s                                   | `{name}` `{count}`  |
| `revenge`         | the bot kills a player who had killed it 3 or more times in a row     | `{name}`            |
| `nemesis`         | a player kills the bot the 3rd time in a row, then the 5th, the 7th … | `{name}` `{count}`  |
| `crowbarred`      | a player kills the bot with the crowbar, not in GunGame               | `{name}`            |
| `crowbar_kill`    | the bot kills a player with the crowbar, not in GunGame               | `{name}`            |
| `own_blast`       | the bot blows itself up                                               | `{weapon}`          |
| `own_blast_other` | a player blows themselves up                                          | `{name}` `{weapon}` |
| `killed_typing`   | a player kills the bot while it types                                 | `{name}`            |
| `last_level`      | the bot reaches the last GunGame level                                |                     |
| `win`             | the map ends, the bot won                                             |                     |
| `gg`              | the map ends, a player won                                            | `{name}`            |

`{map}`, the map's name, fits every moment; any other placeholder is an error, and a phrase may use none. Put
`{name}` where it needs no case ending (`{name}, привет`), `{count}` where it needs no agreement (`{count} подряд`,
`уже {count}`); `{weapon}` reads `сачелем`, `из ракетницы`, `with a rocket`, as in `сам себя {weapon}`.

```yaml
schema: lambdabots/chat-phrases@1
phrases:                           # every bot's: by language, then by moment
  ru:
    gg:
      - "gg, {name}, заслуженно"
      - "красиво сыграно, {name}"
  en:
    gg:
      - "gg wp {name}"
bots:                              # single bots' own
  - name: "Plutonium"              # as in bots.yaml
    phrases:
      ru:
        win:
          - "реактор доволен, gg"
  - name: "Kleiner"
    replace: true                  # instead of the common phrases, for the moments listed here
    phrases:
      en:
        own_blast:
          - "an experiment {weapon}, noted"
```

A bot's own phrases join the common ones of their moment; with `replace: true` they replace them for the moments the
bot lists, and its other moments keep the common ones. Quote every phrase: unquoted, `- {name}, hi` reads as a YAML
mapping and breaks the file. A phrase is 1 to 60 characters (keep it under 45: bots with long nicknames get shorter
lines), without `"` or `%`, does not start with `/ ! @ . #`, and takes only its moment's placeholders; at most 64
phrases a moment in a language. A language is a code as in `chat.language` (`ru`, `en`, `pt-br`). Check the file with
`lb-cli config check config/`.

## Memory of players

The worker keeps `data/chat/memory.json`. A player is remembered by SteamID, or by nickname when there is none
(`STEAM_ID_LAN`, `STEAM_ID_PENDING`). For each one: the names they used, when they were first and last seen, maps
played, kills and deaths, the score against each bot, favourite weapons, GunGame wins, their last ten lines (noise,
swearing and slurs left out), their talks with the bots (the last 12 lines with each of the 3 bots they talked to
last, by the bot's personality), moments worth remembering, and notes. After every map one request asks the model to
update the notes on the players the bots met (1–2 sentences each: how they play, what stood out, how they talk)
unless `memory.ai_notes` is off; swearing and slurs are cut out of the notes a prompt shows. Players unseen for
`memory.forget_after_days` are forgotten; `lb chat memory <player> forget` forgets one now, talks included.

A talk comes back when the player talks with that bot again, days later too: the prompt shows its last lines with
when they were said (`вчера`, `3 дня назад`). A player whose SteamID changes every visit is new every time, to the
memory and its talks; for aliases and notes give them a `by_name` entry in `players.yaml`.

## Settings

`config/lambdabots.yaml`, section `chat` (the shipped file lists every key):

| Key                          | Default                           | Meaning                                                              |
|------------------------------|-----------------------------------|----------------------------------------------------------------------|
| `enabled`                    | `false`                           | chat on (cvar `lb_chat`)                                             |
| `language`                   | `ru`                              | what bots write in; English to players who write English             |
| `server`                     | `""`                              | a line about the server for the bots ("GunGame server hldm.org")     |
| `require_humans`             | `true`                            | no requests while no human is on the server                          |
| `provider.kind`              | `anthropic`                       | `anthropic` or `openai` (OpenAI-compatible)                          |
| `provider.base_url`          | `""`                              | `""` = `https://api.anthropic.com`; a gateway goes here              |
| `provider.model`             | `claude-haiku-4-5`                | model name                                                           |
| `provider.api_key`           | `""`                              | the key; else `api_key_file`; else the `api_key_env` variable        |
| `provider.api_key_file`      | `""`                              | a file holding the key (relative to `addons/lambdabots/`)            |
| `provider.api_key_env`       | `ANTHROPIC_API_KEY`               | environment variable holding the key                                 |
| `provider.ca_file`           | `""`                              | PEM certificates trusted on top of the system's                      |
| `provider.headers`           | `{}`                              | extra HTTP headers (a gateway's own key)                             |
| `provider.timeout`           | `12`                              | seconds a request may take                                           |
| `provider.max_tokens`        | `100`                             | answer length cap                                                    |
| `provider.temperature`       | not sent                          | sampling temperature, sent only when set                             |
| `provider.extra_body`        | `""`                              | JSON object merged into every request                                |
| `limits.lines_per_minute`    | `2`                               | lines of all bots together                                           |
| `limits.remark_gap`          | `60`                              | seconds between two lines nobody asked for                           |
| `limits.bot_remark_gap`      | `240`                             | the same for one bot                                                 |
| `limits.remarks_per_hour`    | `6`                               | lines nobody asked for, an hour, all bots together; 0 = none         |
| `limits.requests_per_minute` | `6`                               | requests to the model                                                |
| `limits.tokens_per_day`      | `2000000`                         | tokens (in and out) a UTC day; then the bots keep quiet; 0 = no cap  |
| `typing.cpm`                 | `[200, 380]`                      | typing speeds of personalities without their own                     |
| `typing.calm`                | `3`                               | seconds without an enemy before an alive bot types                   |
| `memory.enabled`             | `true`                            | remember players and their talks with the bots                       |
| `memory.ai_notes`            | `true`                            | one request after each map for notes on players                      |
| `memory.forget_after_days`   | `120`                             | forget players unseen this long                                      |
| `blocked`                    | `[rtv, rockthevote, nominate, …]` | plugin chat commands: never said, and not chat when players say them |
| `transcript`                 | `false`                           | every request to the model and its answer in `logs/chat.<date>.log`  |
| `chatlog`                    | `true`                            | the game chat, joins and leaves in `logs/chatlog.<date>.log`         |
| `chatlog_days`               | `30`                              | days of chat logs kept                                               |
| `phrases.enabled`            | `true`                            | ready phrases for game moments and greetings                         |
| `phrases.ai_share`           | `0.15`                            | the share of moments the model gets instead of a phrase              |

`blocked` holds the usual map vote and info commands and AG's: `rtv`, `rockthevote`, `nominate`, `nominations`,
`timeleft`, `nextmap`, `thetime`, `currentmap`, `listmaps`, `recentmaps`, `votemap`, `callvote`, `agstart`, `agabort`,
`agpause`, `agallow`, `agmap`, `agnextmap`, `agnextmode`. A player's line that starts with one of them, in any case,
or with `/`, `!` or `@`, is a command, not chat: the bots leave it alone, the chat log keeps it. Add your plugins'
commands, one word each.

The key never leaves the chat worker: recordings and `lb config show` show `<redacted>` (the header values too).
Keep it out of `config/` when that is a link into a repository (`--link-config` on the stand): use `api_key_file`
outside it or `api_key_env`. Plain `http://` carries a key, the API key, a header named like one (`authorization`,
`x-api-key`, `x-gateway-token`) or `user:password@` in `base_url`, only to this machine (`localhost`, `127.0.0.1`,
`[::1]`): a gateway elsewhere needs `https://`.

Notes on models:

- Claude Haiku 4.5 (`claude-haiku-4-5`) is quick and cheap enough for chat lines: a line costs about 2000 tokens in
  and 20 out.
- Claude Sonnet 5.5 and Opus 5.5 refuse `temperature`; leave it unset. Opus 5.5 always thinks: set
  `extra_body: '{"output_config": {"effort": "low"}}'` to keep it quick.
- A refused answer (`stop_reason: refusal`, `finish_reason: content_filter`) is silence.
- OpenAI-compatible reasoning models put their thinking in `<think>…</think>`; it is dropped. Models that want
  `max_completion_tokens` get it from `extra_body: '{"max_completion_tokens": 100}'` (then `max_tokens` is not sent).

## When the provider fails

- **Refused settings** (HTTP 401, 403, 404: a bad key, an unknown model): requests stop until `lb chat reload` or
  `lb config reload`.
- **Busy or out of reach** (429, 5xx, a timeout): requests wait 5 s, then 10, 20 … up to a minute, or as long as
  `retry-after` says.
- **Out of API credit**: HTTP 402, or a 400, 403 or 429 whose error says the account has no money (an insufficient
  balance, a credit balance too low, `insufficient_quota`, API usage limits reached). Then one request every
  10 minutes tries again, whatever `retry-after` says, and `lb chat status` shows
  `out of API credit since 09:13 UTC, next try at 09:23 UTC` with the provider's message. The console gets an error
  when it begins and a line when the provider answers again. After a top-up the chat comes back by itself within
  10 minutes, or at once on `lb chat reload`.
- **The day's tokens spent** (`limits.tokens_per_day`): requests wait for 00:00 UTC; `lb chat status` says so, and
  the log gets one warning.

Meanwhile game moments keep their ready phrases; players get no answers until the model is back.

`lb chat reload` and `lb config reload` read `config/chat/` again and lift a refusal, a wait after failures and the
wait for API credit: the next request goes to the provider at once. The day's spent tokens stay spent.

## What was sent and what came back

With `chat.transcript: true` (or `lb chat transcript on`, until the next `lb config reload`) every request to the
model and its answer go to `logs/chat.<date>.log`, one file a UTC day, a week of them kept. An entry holds the time,
the bot and why it speaks, the address, the request body and the answer's status, time and body, and what came of it:
the line the bot types, nothing, why it was dropped, or why it failed. The JSON bodies are written as YAML, so the
prompts read as text:

```text
===== 2026-10-05 21:14:03 UTC · DUT9 ATLASA · Gordon to the bot: атлас, как ты так быстро бегаешь?
>>> POST https://api.moonshot.ai/v1/chat/completions
max_completion_tokens: 4000
messages:
- content: |-
    Ты — бот-игрок на сервере Half-Life Deathmatch (HLDM) …
  role: system
- content: |-
    Карта crossfire, DM.
    …
  role: user
model: kimi-k3
reasoning_effort: low
<<< 200 in 1840 ms
choices:
- finish_reason: stop
  message:
    content: распрыжка) могу показать
    reasoning_content: …
usage:
  completion_tokens: 212
  prompt_tokens: 1240
=== line: распрыжка) могу показать
```

A line the filter drops ends with `=== no line: dropped (swearing): <the line>`. The headers are not written, so
neither is the key, nor a user, a password or a query in the address. The players' chat and names are: keep the files
as you keep the server's other logs. Requests that were not sent (chat off, the day's tokens spent, waiting after
failures or for API credit) leave no entry, and neither do ready phrases.

## The chat log

Every line of the game chat goes to `logs/chatlog.<date>.log`, one file a UTC day, with UTC times; players joining
and leaving too. It is written whether chat is on or off (`chat.chatlog: true` by default), never by a replay:

```text
---- gg_cold_rock ----
21:14:03 + Gordon
21:14:10   Gordon: всем привет
21:14:13 » Plutonium: привет, Gordon
21:15:02   Alyx (team): го на рельсы
21:15:40   Gordon: /login ***
21:31:55 - Gordon
```

A map's lines come under its name, and the header also starts every file the server opens (a new UTC day, a restart,
`lb config reload`); a map without lines gets none. After the time: a space for a player, `»` for one of our bots,
`+` for a player joining (not one back after a map change), `-` for one leaving; `(team)` marks `say_team`. A name or
a line takes one line: control and invisible characters go, runs of spaces become one.

Players' lines are written as typed, plugin commands too (`rtv`, `/top15`). When the first word, in lower case and
without one leading `/`, `!` or `.`, is `login`, `reg`, `register`, `password`, `pass`, `pw`, `setpw` or `auth`,
whatever follows it is written as `***`; a command alone stays as typed. Not in the log: lines of other plugins' bots,
`say` from the server console or rcon, plugin messages (`amx_say`, GunGame's announcements). The plugin does not see
them as chat.

A day's file is deleted once its date is more than `chat.chatlog_days` days back (30), by the date in its name; no
other file in `logs/` is touched. `lb chat status` names today's file. A file that cannot be written is warned about
once, and tried again the next UTC day or after `lb config reload`. On Windows a file in use cannot be moved or
deleted: the server lets go of a day's file at the next day's first line, or on `lb config reload`.

## What is kept, and for how long

| File                         | Holds                                                         | Kept                                                  |
|------------------------------|---------------------------------------------------------------|-------------------------------------------------------|
| `logs/chatlog.<date>.log`    | every chat line with its nickname, joins and leaves           | `chat.chatlog_days` days (30)                         |
| `data/chat/memory.json`      | per player: names, scores, last lines, talks with bots, notes | `memory.forget_after_days` after the last visit (120) |
| `logs/chat.<date>.log`       | requests and answers, the chat in them (`chat.transcript`)    | a week                                                |
| `logs/lambdabots.<date>.log` | the bots' lines with what they answer                         | `logging.max_files` files (7)                         |
| `records/*.lbrec`            | a recorded map, its chat too (`docs/replay.md`)               | until you delete them                                 |

`lb chat memory <player> forget` forgets a player at once, talks included. Keep these files as you keep the server's
other logs.

## Personalities

A personality's `chat` block in `profiles/*.yaml` says how it talks; anything left out comes from its seed and
style (rushers talk more, snipers less):

```yaml
  - name: "Plutonium"
    chat:
      chattiness: 0.7            # 0..1: how readily it speaks unasked
      profanity: true            # it may swear; default false
      typing_cpm: 300            # characters a minute
      style: "коротко, строчными, ставит ))"
      about: "играет тут каждый вечер, любит гаусс"
      call: [Плутон, Плутоныч]   # names players call it by, besides its nickname
```

A line with a name from `call` names the bot, as its nickname does. The profile goes with the personality to every
server; what one server wants the bot to know goes in that server's `config/chat/bots.yaml`.

## Commands and cvar

| Command                            | Action                                                                    |
|------------------------------------|---------------------------------------------------------------------------|
| `lb chat [status]`                 | on or off, the worker, model lines and phrases, the chat log, who types   |
| `lb chat log [n]`                  | the last lines and decisions                                              |
| `lb chat on\|off`                  | the same as `lb_chat 1\|0`                                                |
| `lb chat say <bot> <text>`         | the bot types and says this line                                          |
| `lb chat test <bot> <text>`        | as if a player wrote this to the bot: the model answers, the bot types it |
| `lb chat event <bot> <key>`        | as if the moment `key` (`win`, `gg`, `nemesis` …) came, you the other one |
| `lb chat prompt <bot> [text]`      | the prompt such a line would get, printed to the server console           |
| `lb chat memory <player> [forget]` | what the bots remember of a player; `forget` forgets them, talks included |
| `lb chat reload`                   | read `config/chat/*.yaml` again and try the provider at once              |
| `lb chat transcript [on\|off]`     | write requests and answers to `logs/chat.<date>.log` (`chat.transcript`)  |

`lb chat status` tells whether the worker is ready or why requests wait (refused settings, failures, out of API
credit, the day's tokens), what it read from `config/chat/`, the session's requests (model lines, phrases, silences,
failures), today's chat log and what every bot is typing. `lb chat event` takes the keys of `phrases.yaml` and, like
`test`, needs chat on: the bot says a phrase, or the model's line when its share comes up (with
`chat.phrases.ai_share: 0`, always a phrase).

`<bot>` is a personality or in-game name, its beginning, or `#userid`; a name with spaces goes in quotes. The worker's
output goes to the server console and the log (`logs/lambdabots.<date>.log`: every line with its reason, time and
tokens, every phrase with its moment; prompts at `debug`).

## Inside

- `crates/lb-chat`: the map's journal and its moments, the director (who speaks, limits), talks (`talk.rs`: who talks
  with whom, the bots' recent lines, greeting times, the hourly cap, carried over map changes), a bot's line from the
  request to `say` (typing, holding the respawn), the prompts, ready phrases (`phrases.rs`: picking and filling them
  in), the text rules and the words the filter looks for (`profanity.rs`), the memory. No threads, files or network.
- `crates/lb-llm`: the two APIs over HTTPS (ureq, rustls with ring, the system's root certificates); it tells an
  account out of money from a busy service.
- `crates/lb-runtime/src/chat/`: the journal fed from the kill feed, `say`/`say_team`, joins and the scoreboard; the
  chat log (`chatlog.rs`), written on the main thread as lines come; the worker thread `lb-chat`, which holds the key,
  the memory, `config/chat/` and the day's token count, and answers every request: with a phrase or the model's line
  passed through the filter. It answers phrases before any model call, so they never wait behind one.
- Replies are an outside input of a recording (`Outside::Chat`), taken once at the start of every `frame_post`: the
  model's lines, the phrases, silences (dropped lines among them) and failures. A replay types the same lines at the
  same frames and never asks the model, reads `config/chat/` or writes the chat log. See `docs/replay.md`.
- After a map the worker saves the memory at once and asks for the notes once it has had nothing to do for 15 s, so
  answers after a map change go first; before the next map ends at the latest. Notes still waiting when the plugin
  stops are dropped.
- The worker stops with the plugin (`meta unload` waits for a request in flight, up to `provider.timeout + 2` s).
