//! `lb-cli demo`: what a GoldSrc demo holds, and its timeline as TSV files for analysis.

use std::fmt::Write as _;
use std::path::Path;

use anyhow::{Context, Result};
use lb_demo::timeline::{EF_NODRAW, Recording};
use rustc_hash::FxHashMap;

fn load(path: &Path) -> Result<Vec<Recording>> {
    let bytes = std::fs::read(path).with_context(|| path.display().to_string())?;
    lb_demo::read(&bytes).with_context(|| path.display().to_string())
}

fn top<K: Clone + Ord>(counts: FxHashMap<K, u32>, n: usize) -> Vec<(K, u32)> {
    let mut v: Vec<(K, u32)> = counts.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    v.truncate(n);
    v
}

pub fn info(path: &Path, opts: &[&str]) -> Result<bool> {
    let names = opts.contains(&"--names");
    let started = std::time::Instant::now();
    let recs = load(path)?;
    let elapsed = started.elapsed();
    println!(
        "{}: {} map(s), read in {:.2} s",
        path.display(),
        recs.len(),
        elapsed.as_secs_f64()
    );
    let mut ok = true;
    for rec in &recs {
        ok &= info_one(rec, names);
    }
    Ok(ok)
}

fn info_one(rec: &Recording, names: bool) -> bool {
    let kind = if rec.is_hltv() { "hltv" } else { "pov" };
    println!(
        "== {} ({}) on {:?}, {kind}, {:.1} s, max clients {}",
        rec.map, rec.game_dir, rec.hostname, rec.duration, rec.max_clients
    );
    let s = &rec.stats;
    println!(
        "messages {}, frames failed {}, missing delta base {}, entity count mismatches {}",
        s.messages, s.frames_failed, s.missing_delta_base, s.count_mismatch
    );
    for (t, e) in rec.errors.iter().take(8) {
        println!("  error at {t:.2}: {e}");
    }
    let rate = rec.snapshots.len() as f32 / rec.duration.max(1.0);
    println!(
        "snapshots {} ({rate:.1}/s), player states {}, shots {}, kills {}, explosions {}, projectile samples {}, \
         item changes {}, sounds {}",
        rec.snapshots.len(),
        rec.states.len(),
        rec.shots.len(),
        rec.kills.len(),
        rec.explosions.len(),
        rec.projectiles.len(),
        rec.items.len(),
        rec.sounds.len()
    );
    if !rec.is_hltv() {
        println!(
            "recorder slot {:?}, views {}, commands {}, console {}",
            rec.recorder,
            rec.views.len(),
            rec.commands.len(),
            rec.console.len()
        );
    }
    println!("players:");
    for p in &rec.players {
        let kind = if p.hltv {
            "hltv"
        } else if p.bot {
            "bot"
        } else {
            "human"
        };
        let name = if names { p.name.as_str() } else { "" };
        println!(
            "  slot {:2} userid {:4} {kind:5} {:7.1}-{:7.1} s  {name}",
            p.slot, p.user_id, p.first_seen, p.last_seen
        );
    }
    let mut events: FxHashMap<String, u32> = FxHashMap::default();
    for sh in &rec.shots {
        *events.entry(sh.event.clone()).or_default() += 1;
    }
    let mut line = String::new();
    for (e, n) in top(events, 30) {
        let _ = write!(line, " {e}={n}");
    }
    println!("events:{line}");
    let mut weapons: FxHashMap<String, u32> = FxHashMap::default();
    for k in &rec.kills {
        *weapons.entry(k.weapon.clone()).or_default() += 1;
    }
    let mut line = String::new();
    for (w, n) in top(weapons, 30) {
        let _ = write!(line, " {w}={n}");
    }
    println!("kills by weapon:{line}");
    let mut held: FxHashMap<String, u32> = FxHashMap::default();
    for st in &rec.states {
        *held.entry(rec.model_name(st.weapon).to_string()).or_default() += 1;
    }
    let mut line = String::new();
    for (w, n) in top(held, 20) {
        let _ = write!(line, " {w}={n}");
    }
    println!("weapon models held (states):{line}");
    let mut sounds: FxHashMap<String, u32> = FxHashMap::default();
    for so in &rec.sounds {
        *sounds
            .entry(rec.sound_names[usize::from(so.sample)].clone())
            .or_default() += 1;
    }
    let mut line = String::new();
    for (w, n) in top(sounds, 25) {
        let _ = write!(line, " {w}={n}");
    }
    println!("sounds:{line}");
    let mut line = String::new();
    for (w, n) in top(rec.user_messages.clone(), 30) {
        let _ = write!(line, " {w}={n}");
    }
    println!("user messages:{line}");
    let mut proj: FxHashMap<String, u32> = FxHashMap::default();
    for p in &rec.projectiles {
        *proj.entry(rec.model_name(p.model).to_string()).or_default() += 1;
    }
    let mut line = String::new();
    for (w, n) in top(proj, 10) {
        let _ = write!(line, " {w}={n}");
    }
    println!("projectile samples:{line}");
    let mut items: FxHashMap<String, u32> = FxHashMap::default();
    for it in &rec.items {
        *items.entry(rec.model_name(it.model).to_string()).or_default() += 1;
    }
    let mut line = String::new();
    for (w, n) in top(items, 20) {
        let _ = write!(line, " {w}={n}");
    }
    println!("item changes:{line}");
    rec.stats.frames_failed == 0
}

/// Writes the timeline as TSV files (`players`, `states`, `shots`, `kills`, `sounds`, `explosions`,
/// `projectiles`, `items`, and for POV demos `views` and `commands`) into `out`.
pub fn export(path: &Path, opts: &[&str]) -> Result<bool> {
    let out = match opts {
        ["--out", dir] => Path::new(dir).to_path_buf(),
        [] => path.with_extension("tsv.d"),
        _ => anyhow::bail!("usage: lb-cli demo export <file.dem> [--out <dir>]"),
    };
    let recs = load(path)?;
    let mut ok = true;
    for (k, rec) in recs.iter().enumerate() {
        let dir = if recs.len() == 1 {
            out.clone()
        } else {
            out.join(format!("part{}-{}", k + 1, rec.map))
        };
        ok &= export_one(rec, &dir)?;
        println!("{} -> {}", path.display(), dir.display());
    }
    Ok(ok)
}

fn export_one(rec: &Recording, out: &Path) -> Result<bool> {
    std::fs::create_dir_all(out)?;
    let write = |name: &str, body: String| -> Result<()> {
        std::fs::write(out.join(name), body).with_context(|| out.join(name).display().to_string())
    };

    let mut meta = String::new();
    let _ = writeln!(meta, "map\t{}", rec.map);
    let _ = writeln!(meta, "game_dir\t{}", rec.game_dir);
    let _ = writeln!(meta, "hostname\t{}", rec.hostname);
    let _ = writeln!(meta, "kind\t{}", if rec.is_hltv() { "hltv" } else { "pov" });
    let _ = writeln!(meta, "recorder\t{}", rec.recorder.map_or(-1, i32::from));
    let _ = writeln!(meta, "max_clients\t{}", rec.max_clients);
    let _ = writeln!(meta, "duration\t{:.3}", rec.duration);
    let _ = writeln!(meta, "frames_failed\t{}", rec.stats.frames_failed);
    write("meta.tsv", meta)?;

    let mut models: Vec<(&u16, &String)> = rec.models.iter().collect();
    models.sort();
    let mut s = String::from("index\tmodel\n");
    for (i, m) in models {
        let _ = writeln!(s, "{i}\t{m}");
    }
    write("models.tsv", s)?;

    let mut s = String::from("who\tslot\tuser_id\tkind\tfirst\tlast\tsteam_id\tname\n");
    for (i, p) in rec.players.iter().enumerate() {
        let kind = if p.hltv {
            "hltv"
        } else if p.bot {
            "bot"
        } else {
            "human"
        };
        let _ = writeln!(
            s,
            "{i}\t{}\t{}\t{kind}\t{:.3}\t{:.3}\t{}\t{}",
            p.slot,
            p.user_id,
            p.first_seen,
            p.last_seen,
            p.steam_id,
            p.name.replace(['\t', '\n'], " ")
        );
    }
    write("players.tsv", s)?;

    let mut s = String::from(
        "time\tserver_time\tslot\twho\tx\ty\tz\tpitch\tyaw\tweapon\tsequence\tgait\teffects\tmovetype\tsolid\tusehull\tspectator\tiuser1\tnodraw\n",
    );
    for snap in &rec.snapshots {
        for st in rec.states_of(snap) {
            let who = if st.who == u16::MAX { -1 } else { i32::from(st.who) };
            let weapon = rec
                .model_name(st.weapon)
                .trim_start_matches("models/")
                .trim_end_matches(".mdl");
            let _ = writeln!(
                s,
                "{:.3}\t{:.3}\t{}\t{who}\t{:.1}\t{:.1}\t{:.1}\t{:.2}\t{:.2}\t{weapon}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                snap.time,
                snap.server_time,
                st.slot,
                st.origin[0],
                st.origin[1],
                st.origin[2],
                st.angles[0],
                st.angles[1],
                st.sequence,
                st.gaitsequence,
                st.effects,
                st.movetype,
                st.solid,
                st.usehull,
                st.spectator,
                st.iuser1,
                u8::from(st.effects & EF_NODRAW != 0)
            );
        }
    }
    write("states.tsv", s)?;

    let mut s = String::from(
        "time\tevent\tentity\tx\ty\tz\tpitch\tyaw\tfparam1\tfparam2\tiparam1\tiparam2\tbparam1\tbparam2\tducking\n",
    );
    for sh in &rec.shots {
        let _ = writeln!(
            s,
            "{:.3}\t{}\t{}\t{:.1}\t{:.1}\t{:.1}\t{:.2}\t{:.2}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            sh.time,
            sh.event,
            sh.entity,
            sh.origin[0],
            sh.origin[1],
            sh.origin[2],
            sh.angles[0],
            sh.angles[1],
            sh.fparam1,
            sh.fparam2,
            sh.iparam1,
            sh.iparam2,
            sh.bparam1,
            sh.bparam2,
            sh.ducking
        );
    }
    write("shots.tsv", s)?;

    let mut s = String::from("time\tkiller\tvictim\tweapon\n");
    for k in &rec.kills {
        let _ = writeln!(s, "{:.3}\t{}\t{}\t{}", k.time, k.killer, k.victim, k.weapon);
    }
    write("kills.tsv", s)?;

    let mut s = String::from("time\tentity\tchannel\tsample\tvolume\tx\ty\tz\n");
    for so in &rec.sounds {
        let _ = writeln!(
            s,
            "{:.3}\t{}\t{}\t{}\t{:.2}\t{:.1}\t{:.1}\t{:.1}",
            so.time,
            so.entity,
            so.channel,
            rec.sound_names[usize::from(so.sample)],
            so.volume,
            so.origin[0],
            so.origin[1],
            so.origin[2]
        );
    }
    write("sounds.tsv", s)?;

    let mut s = String::from("time\tx\ty\tz\tscale\n");
    for e in &rec.explosions {
        let _ = writeln!(
            s,
            "{:.3}\t{:.1}\t{:.1}\t{:.1}\t{}",
            e.time, e.origin[0], e.origin[1], e.origin[2], e.scale
        );
    }
    write("explosions.tsv", s)?;

    let mut s = String::from("time\tentity\tmodel\tx\ty\tz\towner\n");
    for p in &rec.projectiles {
        let _ = writeln!(
            s,
            "{:.3}\t{}\t{}\t{:.1}\t{:.1}\t{:.1}\t{}",
            p.time,
            p.entity,
            rec.model_name(p.model),
            p.origin[0],
            p.origin[1],
            p.origin[2],
            p.owner
        );
    }
    write("projectiles.tsv", s)?;

    let mut s = String::from("time\tentity\tmodel\tx\ty\tz\tvisible\tremoved\n");
    for it in &rec.items {
        let _ = writeln!(
            s,
            "{:.3}\t{}\t{}\t{:.1}\t{:.1}\t{:.1}\t{}\t{}",
            it.time,
            it.entity,
            rec.model_name(it.model),
            it.origin[0],
            it.origin[1],
            it.origin[2],
            u8::from(it.visible),
            u8::from(it.removed)
        );
    }
    write("items.tsv", s)?;

    if !rec.is_hltv() {
        let mut s = String::from("time\tx\ty\tz\tpitch\tyaw\n");
        for v in &rec.views {
            let _ = writeln!(
                s,
                "{:.4}\t{:.2}\t{:.2}\t{:.2}\t{:.3}\t{:.3}",
                v.time, v.origin[0], v.origin[1], v.origin[2], v.angles[0], v.angles[1]
            );
        }
        write("views.tsv", s)?;
        let mut s = String::from("time\tmsec\tpitch\tyaw\tforward\tside\tbuttons\thealth\n");
        for c in &rec.commands {
            let _ = writeln!(
                s,
                "{:.4}\t{}\t{:.3}\t{:.3}\t{}\t{}\t{}\t{}",
                c.time, c.msec, c.viewangles[0], c.viewangles[1], c.forwardmove, c.sidemove, c.buttons, c.health
            );
        }
        write("commands.tsv", s)?;
    }
    Ok(rec.stats.frames_failed == 0)
}
