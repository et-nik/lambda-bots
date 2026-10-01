//! Bots on command (`lb give`, `lb do`, `lb test`): taken off their behavior, a bot goes to the spot it is told with
//! the items it is given, and the console, the log and the test report say how it went. The going is
//! `lb_nav::reach`; the weapons' part of a gauss boost is played as in the obstacle course (`lb nav test`). While a
//! bot is on command, the bots that are not stand still.

use lb_core::Vec3;
use lb_core::time::SimTime;
use lb_game::weapons::WeaponId;
use lb_nav::reach::{Allowed, Reach};

/// Seconds an attempt may take unless told otherwise.
pub const TIMEOUT: f64 = 60.0;

/// A spot as a command names it.
#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    Point(Vec3),
    Node(u32),
    /// Where the player who typed the command stands.
    Me,
    /// What the player who typed the command looks at.
    Aim,
    /// A place of the map's overlays.
    Place(String),
}

impl Target {
    /// A target from the start of `args`: `x y z`, `node <n>`, `@me`, `@aim` or `place <name>`, and the words it took.
    pub fn parse(args: &[&str]) -> Result<(Target, usize), String> {
        match args {
            ["@me", ..] => Ok((Target::Me, 1)),
            ["@aim", ..] => Ok((Target::Aim, 1)),
            ["node", n, ..] => n
                .parse()
                .map(|n| (Target::Node(n), 2))
                .map_err(|_| format!("`{n}` is not a node number")),
            ["place", name, ..] => Ok((Target::Place((*name).to_string()), 2)),
            [x, y, z, ..] => match (x.parse(), y.parse(), z.parse()) {
                (Ok(x), Ok(y), Ok(z)) => Ok((Target::Point(Vec3::new(x, y, z)), 3)),
                _ => Err(format!(
                    "`{x} {y} {z}`: expected a spot (x y z, node <n>, @me, @aim, place <name>)"
                )),
            },
            _ => Err("expected a spot: x y z, node <n>, @me, @aim or place <name>".into()),
        }
    }
}

/// How an attempt goes: `radius R`, `timeout T` and `tricks <jump|longjump|gauss|any|none>...`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GoOptions {
    pub radius: f32,
    pub timeout: f64,
    pub allowed: Allowed,
}

impl Default for GoOptions {
    fn default() -> GoOptions {
        GoOptions {
            radius: lb_nav::reach::RADIUS,
            timeout: TIMEOUT,
            allowed: Allowed::default(),
        }
    }
}

impl GoOptions {
    pub fn parse(args: &[&str]) -> Result<GoOptions, String> {
        let mut o = GoOptions::default();
        let mut i = 0;
        while i < args.len() {
            let value = args.get(i + 1).copied();
            match args[i] {
                "radius" => {
                    o.radius = value
                        .and_then(|v| v.parse().ok())
                        .filter(|r: &f32| *r > 0.0)
                        .ok_or("radius <units>")?;
                    i += 2;
                }
                "timeout" => {
                    o.timeout = value
                        .and_then(|v| v.parse().ok())
                        .filter(|t: &f64| *t > 0.0)
                        .ok_or("timeout <seconds>")?;
                    i += 2;
                }
                "tricks" => {
                    let words: Vec<&str> = args[i + 1..]
                        .iter()
                        .copied()
                        .take_while(|w| !matches!(*w, "radius" | "timeout"))
                        .collect();
                    o.allowed = Allowed::parse(&words)?;
                    i += 1 + words.len();
                }
                other => return Err(format!("unknown option `{other}` (radius, timeout, tricks)")),
            }
        }
        Ok(o)
    }
}

/// The `give` commands for what a command names: a weapon with its ammo, `uranium`, `longjump`, `health`, `armor`,
/// or any `weapon_`, `ammo_` or `item_` classname as it is. The game honours them with `sv_cheats 1`.
pub fn give_list<S: AsRef<str>>(names: &[S]) -> Result<Vec<String>, String> {
    let mut out: Vec<String> = Vec::new();
    let mut add = |item: &str, times: usize| out.extend(std::iter::repeat_n(item.to_string(), times));
    for name in names {
        let n = name.as_ref().to_ascii_lowercase();
        match n.as_str() {
            "uranium" => add("ammo_gaussclip", 5),
            "longjump" | "lj" => add("item_longjump", 1),
            "health" | "hp" => add("item_healthkit", 4),
            "armor" | "armour" | "battery" => add("item_battery", 7),
            n if n.starts_with("ammo_") || n.starts_with("item_") => add(n, 1),
            n => match weapon(n) {
                Some(w) => {
                    add(w.classname(), if w.is_throwable() { 3 } else { 1 });
                    for a in ammo_of(w) {
                        add(a, if w == WeaponId::Gauss { 5 } else { 3 });
                    }
                }
                None => return Err(format!("unknown item `{}`", name.as_ref())),
            },
        }
    }
    Ok(out)
}

/// A weapon by its classname or a short name.
pub fn weapon(name: &str) -> Option<WeaponId> {
    match name.to_ascii_lowercase().as_str() {
        "357" | "python" => Some(WeaponId::Python),
        "mp5" | "9mmar" => Some(WeaponId::Mp5),
        "glock" | "9mmhandgun" => Some(WeaponId::Glock),
        "hornet" | "hornetgun" => Some(WeaponId::Hornetgun),
        "grenade" | "handgrenade" => Some(WeaponId::HandGrenade),
        other => WeaponId::from_classname(other),
    }
}

/// The ammo a weapon takes, by the classnames that give it.
pub fn ammo_of(w: WeaponId) -> &'static [&'static str] {
    match w {
        WeaponId::Glock => &["ammo_9mmclip"],
        WeaponId::Mp5 => &["ammo_9mmAR", "ammo_ARgrenades"],
        WeaponId::Python => &["ammo_357"],
        WeaponId::Crossbow => &["ammo_crossbow"],
        WeaponId::Shotgun => &["ammo_buckshot"],
        WeaponId::Rpg => &["ammo_rpgclip"],
        WeaponId::Gauss | WeaponId::Egon => &["ammo_gaussclip"],
        _ => &[],
    }
}

/// What a bot on command does.
pub enum OrderKind {
    Go(Box<Reach>),
    /// Stand still until then: a test run gives items, or lets the bot settle at the start.
    Hold(SimTime),
}

pub struct Order {
    pub kind: OrderKind,
    /// What was asked, for the lines that report it.
    pub what: String,
    /// The weapons' part of a gauss boost under way.
    pub boost: Option<lb_combat::arms::boost::GaussBoost>,
    /// Part of a test run, which takes the outcome; otherwise the outcome is printed.
    pub test: bool,
    /// The player who gave the command, told the outcome too.
    pub reply_to: Option<u8>,
}

impl Order {
    pub fn go(reach: Reach, what: String, test: bool) -> Order {
        Order {
            kind: OrderKind::Go(Box::new(reach)),
            what,
            boost: None,
            test,
            reply_to: None,
        }
    }

    pub fn hold(until: SimTime) -> Order {
        Order {
            kind: OrderKind::Hold(until),
            what: "hold".into(),
            boost: None,
            test: true,
            reply_to: None,
        }
    }

    pub fn reach(&self) -> Option<&Reach> {
        match &self.kind {
            OrderKind::Go(r) => Some(r),
            OrderKind::Hold(_) => None,
        }
    }

    pub fn done(&self, now: SimTime) -> bool {
        match &self.kind {
            OrderKind::Go(r) => r.outcome().is_some(),
            OrderKind::Hold(until) => now >= *until,
        }
    }
}

/// The lines saying how an attempt went: the outcome with the time against the plan, then the trick, where it came
/// down, and the links that failed on the way.
pub fn outcome_lines(reach: &Reach) -> Vec<String> {
    let r = reach.report();
    let Some(outcome) = reach.outcome() else {
        return Vec::new();
    };
    let plan = r.planned.map(|p| format!(" (plan {p:.1} s)")).unwrap_or_default();
    let mut out = vec![format!("{} in {:.1} s{plan}", outcome.describe(), r.seconds)];
    if !r.links.is_empty() {
        let kinds: Vec<String> = r.links.iter().map(|(k, n)| format!("{k} {n}")).collect();
        out.push(format!(
            "  way: {} links ({})",
            r.path.len().saturating_sub(1),
            kinds.join(", ")
        ));
    }
    if let Some(t) = &r.trick {
        out.push(format!("  trick: {}", t.describe()));
    }
    if let Some(l) = r.landing {
        out.push(format!(
            "  came down at {:.0} {:.0} {:.0}, {:.0} u from the spot",
            l.x,
            l.y,
            l.z,
            (reach.spot - l).truncate().length()
        ));
    }
    for f in &r.failures {
        out.push(format!(
            "  {:.1} s: link {} -> {} ({}) failed: {}",
            f.t, f.from, f.to, f.kind, f.reason
        ));
    }
    let st = r.search;
    if st.takeoffs > 0 {
        out.push(format!(
            "  trick search: {} takeoffs, {} jumps, {} long jumps, {} gauss boosts checked",
            st.takeoffs, st.jumps, st.longjumps, st.boosts
        ));
    }
    out
}

/// Where a player stands at `target`: the origin of a standing player there. `@me` and `@aim` are the player's who
/// typed the command.
pub(crate) fn spot_of(rt: &crate::Runtime, host: &mut dyn lb_host::Host, target: &Target) -> Result<Vec3, String> {
    let mut tracer = crate::nav::LiveTracer { host, count: 0 };
    let player = || {
        rt.command_slot
            .and_then(|s| rt.clients_now.iter().find(|c| c.slot == s))
            .ok_or_else(|| {
                "@me and @aim are where the player typing the command stands and looks: run it from the game console"
                    .to_string()
            })
    };
    let floor = |tracer: &mut crate::nav::LiveTracer<'_>, p: Vec3, what: &str| {
        lb_nav::reach::standing_spot(tracer, p)
            .ok_or_else(|| format!("no floor to stand on under {what} ({:.0} {:.0} {:.0})", p.x, p.y, p.z))
    };
    match target {
        Target::Point(p) => floor(&mut tracer, *p, "the spot"),
        Target::Node(n) => {
            let g = rt.graph.as_deref().ok_or("no navigation graph")?;
            if *n as usize >= g.len() {
                return Err(format!("no node {n}: the graph has {}", g.len()));
            }
            Ok(lb_nav::classify::stand_origin(g.node(*n)))
        }
        Target::Me => {
            let c = player()?;
            floor(&mut tracer, c.origin, "you")
        }
        Target::Aim => {
            let c = player()?;
            let eye = c.origin + c.view_ofs;
            // The engine turns a player's model a third of the way the view pitches, the other way round.
            let view = Vec3::new(-3.0 * c.angles.x, c.angles.y, 0.0);
            let (dir, _, _) = lb_core::math::view_angle_vectors(view);
            let hit = lb_worldq::Tracer::trace(&mut tracer, &lb_worldq::TraceQuery::line(eye, eye + dir * 8192.0));
            if hit.fraction >= 1.0 {
                return Err("you look at nothing within 8192 units".into());
            }
            floor(&mut tracer, hit.end - dir * 16.0, "where you look")
        }
        Target::Place(name) => {
            let place = rt
                .overlays
                .iter()
                .flat_map(|o| o.places.iter())
                .find(|p| p.name == *name)
                .ok_or_else(|| format!("no place `{name}` in the map's overlays"))?;
            floor(&mut tracer, Vec3::from(place.at), &format!("place {name}"))
        }
    }
}

/// Lines about bots on command: to the log, the server console and the player who gave the command.
pub(crate) fn say(host: &mut dyn lb_host::Host, to: Option<u8>, lines: &[String]) {
    for line in lines {
        tracing::info!("{line}");
        crate::logging::console_line(format!("[lambdabots] {line}"));
        if let Some(slot) = to {
            host.client_print(slot, lb_host::PrintKind::Console, &format!("{line}\n"));
        }
    }
}

/// How many commands `logs/tests/<map>-orders.jsonl` keeps: the oldest go past this.
const ORDERS_KEPT: usize = 200;

/// Adds a command's outcome to `logs/tests/<map>-orders.jsonl`, where the map editor finds the links that failed.
fn log_order(install: &std::path::Path, map: &str, entry: &serde_json::Value) {
    let path = install.join("logs").join("tests").join(format!("{map}-orders.jsonl"));
    let mut lines: Vec<String> = std::fs::read_to_string(&path)
        .map(|t| t.lines().map(str::to_string).collect())
        .unwrap_or_default();
    lines.push(entry.to_string());
    let skip = lines.len().saturating_sub(ORDERS_KEPT);
    let text: String = lines[skip..].iter().map(|l| format!("{l}\n")).collect();
    let write = || -> std::io::Result<()> {
        std::fs::create_dir_all(path.parent().expect("logs/tests has a parent"))?;
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, &path)
    };
    if let Err(e) = write() {
        tracing::warn!("{}: {e}", path.display());
    }
}

/// `sv_cheats` is on: the game gives items (it must be on when the map starts).
pub(crate) fn cheats(rt: &mut crate::Runtime, host: &mut dyn lb_host::Host) -> bool {
    rt.cvars.game_value(host, "sv_cheats").is_none_or(|v| v.trim() != "0")
}

pub(crate) const CHEATS_OFF: &str = "sv_cheats is 0: the game gives nothing; set sv_cheats 1 and start the map again";

/// Live bots `who` names (a name, `#userid` or `all`).
pub(crate) fn live_bots(rt: &crate::Runtime, who: &str) -> Vec<usize> {
    crate::commands::find_bots(rt, Some(who))
        .into_iter()
        .filter(|&i| rt.bots[i].state == crate::manager::BotState::Alive)
        .collect()
}

fn names(rt: &crate::Runtime, bots: &[usize]) -> String {
    let names: Vec<&str> = bots.iter().map(|&i| rt.bots[i].persona.name.as_str()).collect();
    names.join(", ")
}

/// `lb give <bot|all> <item>...`: items for bots, now.
pub(crate) fn give(rt: &mut crate::Runtime, host: &mut dyn lb_host::Host, args: &[&str]) -> Vec<String> {
    let [who, items @ ..] = args else {
        return vec!["usage: lb give <name|#userid|all> <item>... (gauss, uranium, longjump, health, armor, a weapon or a classname)".into()];
    };
    if items.is_empty() {
        return vec!["lb give: name the items".into()];
    }
    let list = match give_list(items) {
        Ok(l) => l,
        Err(e) => return vec![e],
    };
    let bots = live_bots(rt, who);
    if bots.is_empty() {
        return vec![format!("no live bot `{who}`")];
    }
    for &i in &bots {
        for item in &list {
            rt.bots[i].pending_client_cmds.push(vec!["give".into(), item.clone()]);
        }
    }
    let mut out = vec![format!("{} given to {}", items.join(" "), names(rt, &bots))];
    if !cheats(rt, host) {
        out.push(CHEATS_OFF.into());
    }
    out
}

const DO_USAGE: &str = "usage: lb do <name|#userid|all> go <x y z|node <n>|@me|@aim|place <name>> [radius R] [timeout T] \
                        [tricks jump longjump gauss|any|none] | lb do [<name|all>] stop | lb do";

/// `lb do`: bots on command.
pub(crate) fn command(rt: &mut crate::Runtime, host: &mut dyn lb_host::Host, args: &[&str]) -> Vec<String> {
    let now = rt.now;
    match args {
        [] => {
            let mut out: Vec<String> = rt
                .bots
                .iter()
                .filter_map(|b| {
                    let o = b.order.as_ref()?;
                    let doing = match &o.kind {
                        OrderKind::Go(r) => format!("go {}: {}", o.what, r.phase(&b.nav)),
                        OrderKind::Hold(_) => "stands still for a test".into(),
                    };
                    Some(format!("{}: {doing}", b.persona.name))
                })
                .collect();
            if let Some(run) = &rt.test_run {
                out.push(run.status());
            }
            if out.is_empty() {
                out.push(format!("no bot on command; {DO_USAGE}"));
            }
            out
        }
        ["stop"] | [_, "stop"] => {
            let who = if args.len() == 2 { args[0] } else { "all" };
            let mut n = 0;
            for i in crate::commands::find_bots(rt, Some(who)) {
                let b = &mut rt.bots[i];
                if b.order.as_ref().is_some_and(|o| !o.test) {
                    b.order = None;
                    b.nav.clear();
                    n += 1;
                }
            }
            vec![format!("{n} commands called off")]
        }
        [who, "go", rest @ ..] => {
            if rt.test_run.is_some() {
                return vec!["a test run is under way: lb test stop".into()];
            }
            if rt.graph.is_none() || rt.search_world.is_none() {
                return vec![format!("no navigation yet: {}", rt.nav_status)];
            }
            let (target, used) = match Target::parse(rest) {
                Ok(t) => t,
                Err(e) => return vec![e],
            };
            let opts = match GoOptions::parse(&rest[used..]) {
                Ok(o) => o,
                Err(e) => return vec![e],
            };
            let spot = match spot_of(rt, host, &target) {
                Ok(s) => s,
                Err(e) => return vec![e],
            };
            let bots = live_bots(rt, who);
            if bots.is_empty() {
                return vec![format!("no live bot `{who}`")];
            }
            let what = format!("{:.0} {:.0} {:.0}", spot.x, spot.y, spot.z);
            let reply_to = rt.command_slot;
            for &i in &bots {
                let mut reach = Reach::new(spot, opts.radius, opts.allowed, opts.timeout, now.secs());
                reach.checks = lb_nav::reach::CHECKS_PER_FRAME;
                let mut order = Order::go(reach, what.clone(), false);
                order.reply_to = reply_to;
                let b = &mut rt.bots[i];
                b.nav.clear();
                b.order = Some(order);
            }
            vec![format!(
                "{} go to {what} (radius {:.0}, tricks {}, {:.0} s); the others stand still; the outcome goes to the \
                 console and the log",
                names(rt, &bots),
                opts.radius,
                opts.allowed.names(),
                opts.timeout
            )]
        }
        _ => vec![DO_USAGE.into()],
    }
}

impl crate::Runtime {
    /// Bots on command: a command's outcome is reported when it is done, the test run moves on, and while any bot is
    /// on command the others stand still.
    pub(crate) fn tend_orders(&mut self, host: &mut dyn lb_host::Host) {
        let now = self.now;
        for bot in &mut self.bots {
            if !bot.order.as_ref().is_some_and(|o| !o.test && o.done(now)) {
                continue;
            }
            let Some(order) = bot.order.take() else {
                continue;
            };
            if let Some(reach) = order.reach() {
                let mut lines = outcome_lines(reach);
                if let Some(first) = lines.first_mut() {
                    *first = format!("lb do: {} go {}: {first}", bot.persona.name, order.what);
                }
                say(host, order.reply_to, &lines);
                if let (Some(map), Some(outcome)) = (&self.map, reach.outcome()) {
                    let entry = serde_json::json!({
                        "at": crate::roster::stamp(),
                        "bot": bot.persona.name,
                        "what": order.what,
                        "outcome": outcome,
                        "report": reach.report(),
                    });
                    log_order(&self.init.install_dir, &map.name, &entry);
                }
            }
            bot.nav.clear();
        }
        self.tick_test_run(host);
        let commanded = self.test_run.is_some() || self.bots.iter().any(|b| b.order.is_some());
        match (commanded, self.freeze_before_orders) {
            (true, None) => {
                self.freeze_before_orders = Some(self.freeze);
                self.freeze = true;
            }
            (false, Some(before)) => {
                self.freeze = before;
                self.freeze_before_orders = None;
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targets_and_options_parse() {
        assert_eq!(
            Target::parse(&["10", "-20", "30.5", "radius", "16"]).unwrap(),
            (Target::Point(Vec3::new(10.0, -20.0, 30.5)), 3)
        );
        assert_eq!(Target::parse(&["node", "42"]).unwrap(), (Target::Node(42), 2));
        assert_eq!(Target::parse(&["@aim"]).unwrap(), (Target::Aim, 1));
        assert_eq!(
            Target::parse(&["place", "roof"]).unwrap(),
            (Target::Place("roof".into()), 2)
        );
        assert!(Target::parse(&["up"]).is_err());
        let o = GoOptions::parse(&["tricks", "gauss", "jump", "timeout", "20"]).unwrap();
        assert!(o.allowed.gauss && o.allowed.jump && !o.allowed.longjump);
        assert_eq!((o.timeout, o.radius), (20.0, lb_nav::reach::RADIUS));
        assert!(GoOptions::parse(&["radius"]).is_err());
        assert!(GoOptions::parse(&["fast"]).is_err());
    }

    #[test]
    fn give_names_a_weapon_its_ammo_and_the_items() {
        let g = give_list(&["gauss", "longjump", "item_battery"]).unwrap();
        assert_eq!(g.iter().filter(|i| *i == "weapon_gauss").count(), 1);
        assert_eq!(g.iter().filter(|i| *i == "ammo_gaussclip").count(), 5);
        assert!(g.contains(&"item_longjump".to_string()) && g.contains(&"item_battery".to_string()));
        assert!(give_list(&["bfg"]).is_err());
    }
}
