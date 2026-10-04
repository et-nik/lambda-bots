# Bots in the chat

Bots talk in the game chat the way players do, through a chat model: the Anthropic Messages API or any
OpenAI-compatible chat completions endpoint (OpenAI, OpenRouter, vLLM, Ollama, llama.cpp, a gateway of your own).
They answer players who speak to them and now and then say something about the game: a word to a player who just
joined, gg when a match ends, a complaint about a nemesis or a crowbar kill, a laugh at their own satchel. They
remember the players they meet, between maps and restarts.

Chat is off until `chat.enabled: true` (or `lb_chat 1`), and nothing is sent while no human is on the server.

## How it plays

- **Who speaks.** A player who names a bot (`Атлас` for `DUT9 ATLASA`, `klein` for `Kleiner`, in either script)
  or writes to "the bots" is answered almost always; a player answering a bot's line within 20 s goes on talking to
  it. A line to everybody, a player joining, the end of a match and notable moments (a nemesis, a crowbar kill, a
  revenge, a player blowing themselves up, a multikill, a streak, a rage quit, a GunGame bot reaching the last level)
  get a word now and then, as often as a bot's chattiness says. Bots never answer bots.
- **How much.** All bots together say at most `limits.lines_per_minute` lines a minute (2 by default); between two
  lines nobody asked for pass `limits.remark_gap` seconds, and `limits.bot_remark_gap` for one bot. A line the model
  chose not to say (`-`) costs nothing.
- **Typing.** A line takes as long as a player's would: noticing and reading what it answers, thinking (short lines
  come at once), typing at the personality's speed. An alive bot types only in a calm moment (no enemy seen or heard
  for `typing.calm` seconds, not fighting, on the ground) and stands still while it types, as a player whose keyboard
  is in the chat input. An enemy in sight or close, or damage, makes it drop the chat and fight; it types the line
  again later, once. A dead bot types at once and holds its respawn until the line is out, at most 5 s (about when the
  game respawns it anyway); a bot respawned mid-line finishes it standing, unless a fight comes first. At the end of a
  match (the intermission, or GunGame's frozen players) everyone types freely.
- **Language.** Lines are in `chat.language` (Russian by default), and in the player's language when a player speaks
  to a bot in another.
- **Tone.** Slang and teasing for everyone; swearing only for personalities with `chat.profanity: true`. Never:
  insults about nationality, religion, sex or orientation, threats, players' real lives, politics, server commands.
  Bots never claim to be human; asked sincerely whether they are bots, they do not deny it (they may joke about it).
- **The line itself.** One line, cut to what `Host_Say` takes (`123 − name bytes` for `say`), on a character boundary;
  no quotes, `%`, control characters, emoji, command prefixes (`/ ! @ . #`) or a first word in `chat.blocked` (plugin
  commands such as `rtv`). The SDK's `Host_Say` before 2023 drops a line without an ASCII character, so with
  `game.dll: classic` pure Cyrillic gets a `)`; BugfixedHL-Rebased, the 2023 update and hlsdk-portable take UTF-8.

## What the model sees

For every line one request: the rules (the same for every bot), the bot (name, skill, style, favourite weapons, its
manner of writing, its own words, swearing or not, its mood), and the scene: map and mode, minute, its score and
GunGame level, the leader, the players on the server and its score against each on this map, what the bots know of
them, the last two maps, the last three minutes of kills and chat with how long ago they happened, and the reason to
speak. The prompt is in Russian for `language: ru` and in English for any other language. `lb chat prompt <bot>`
prints it.

Kill feed, chat, joins and the scoreboard are what every player sees; the request holds nothing a player could not
know. Which players are bots is not said.

## Memory of players

The worker keeps `data/chat/memory.json`. A player is remembered by SteamID, or by nickname when there is none
(`STEAM_ID_LAN`, `STEAM_ID_PENDING`). For each one: the names they used, when they were first and last seen, maps
played, kills and deaths, the score against each bot, favourite weapons, GunGame wins, their last ten lines, moments
worth remembering, and notes. After every map one request asks the model to update the notes on the players the
bots met (1–2 sentences each: how they play, what stood out, how they talk) unless `memory.ai_notes` is off.
Players unseen for `memory.forget_after_days` are forgotten; `lb chat memory <player> forget` forgets one now.

Your own words on regulars go in `config/chat/players.yaml`; the bots read them with their own notes:

```yaml
schema: lambdabots/chat-players@1
players:
  - id: STEAM_0:0:219579426      # a SteamID, or the nickname of a player without one
    name: ATLAS Gamer
    note: Хороший игрок, один из лучших. Многие называют его читером.
```

## Settings

`config/lambdabots.yaml`, section `chat` (the shipped file lists every key):

| Key                          | Default                           | Meaning                                                              |
|------------------------------|-----------------------------------|----------------------------------------------------------------------|
| `enabled`                    | `false`                           | chat on (cvar `lb_chat`)                                             |
| `language`                   | `ru`                              | what bots write in unless spoken to in another language              |
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
| `limits.requests_per_minute` | `6`                               | requests to the model                                                |
| `limits.tokens_per_day`      | `2000000`                         | tokens (in and out) a UTC day; then the bots keep quiet; 0 = no cap  |
| `typing.cpm`                 | `[150, 330]`                      | typing speeds of personalities without their own                     |
| `typing.calm`                | `3`                               | seconds without an enemy before an alive bot types                   |
| `memory.enabled`             | `true`                            | remember players                                                     |
| `memory.ai_notes`            | `true`                            | one request after each map for notes on players                      |
| `memory.forget_after_days`   | `120`                             | forget players unseen this long                                      |
| `blocked`                    | `[rtv, rockthevote, nominate, …]` | plugin chat commands: never said, and not chat when players say them |

The key never leaves the chat worker: recordings and `lb config show` show `<redacted>` (the header values too).
Keep it out of `config/` when that is a link into a repository (`--link-config` on the stand): use `api_key_file`
outside it or `api_key_env`.

Notes on models:

- Claude Haiku 4.5 (`claude-haiku-4-5`) is quick and cheap enough for chat lines: a line costs about 2000 tokens in
  and 20 out.
- Claude Sonnet 5.5 and Opus 5.5 refuse `temperature`; leave it unset. Opus 5.5 always thinks: set
  `extra_body: '{"output_config": {"effort": "low"}}'` to keep it quick.
- A refused answer (`stop_reason: refusal`, `finish_reason: content_filter`) is silence.
- OpenAI-compatible reasoning models put their thinking in `<think>…</think>`; it is dropped. Models that want
  `max_completion_tokens` get it from `extra_body: '{"max_completion_tokens": 100}'` (then `max_tokens` is not sent).

When the provider refuses the settings (HTTP 401, 403, 404: a bad key, an unknown model), chat stops until
`lb chat reload` or `lb config reload`. When it is busy or out of reach (429, 5xx, a timeout), requests wait 5 s,
then 10, 20 … up to a minute, or as long as `retry-after` says.

## Personalities

A personality's `chat` block in `profiles/*.yaml` says how it talks; anything left out comes from its seed and
style (rushers talk more, snipers less):

```yaml
  - name: "DUT9 ATLASA"
    chat:
      chattiness: 0.7            # 0..1: how readily it speaks unasked
      profanity: true            # it may swear; default false
      typing_cpm: 320            # characters a minute
      style: "коротко, строчными, ставит ))"
      about: "довольно хороший игрок, иногда его зовут читером"
```

## Commands and cvar

| Command                            | Action                                                                    |
|------------------------------------|---------------------------------------------------------------------------|
| `lb chat [status]`                 | on or off, the worker, tokens today, what every bot is typing             |
| `lb chat log [n]`                  | the last lines and decisions                                              |
| `lb chat on\|off`                  | the same as `lb_chat 1\|0`                                                |
| `lb chat say <bot> <text>`         | the bot types and says this line                                          |
| `lb chat test <bot> <text>`        | as if a player wrote this to the bot: the model answers, the bot types it |
| `lb chat prompt <bot> [text]`      | the prompt such a line would get, printed to the server console           |
| `lb chat memory <player> [forget]` | what the bots remember of a player; `forget` forgets them                 |
| `lb chat reload`                   | read `config/chat/players.yaml` again and retry the provider              |

`<bot>` is a personality or in-game name, its beginning, or `#userid`; a name with spaces goes in quotes. The worker's
output goes to the server console and the log (`logs/lambdabots.<date>.log`: every line with its reason, time and
tokens; prompts at `debug`).

## Inside

- `crates/lb-chat`: the map's journal and its notable moments, the director (who speaks, limits), a bot's line from
  the request to `say` (typing, holding the respawn), the prompts, the text rules, the memory. No threads, files or
  network.
- `crates/lb-llm`: the two APIs over HTTPS (ureq, rustls with ring, the system's root certificates).
- `crates/lb-runtime/src/chat/`: the journal fed from the kill feed, `say`/`say_team`, joins and the scoreboard; the
  worker thread `lb-chat`, which holds the key, the memory and the day's token count and answers every request.
- Replies are an outside input of a recording (`Outside::Chat`), taken once at the start of every `frame_post`; a
  replay types the same lines at the same frames and never asks the model. See `docs/replay.md`.
- The worker stops with the plugin (`meta unload` waits for a request in flight, up to `provider.timeout + 2` s).
