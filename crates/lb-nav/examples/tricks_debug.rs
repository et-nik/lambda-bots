//! Prints the trick links of a `.lbnav` graph: how far and how high they go, and how much longer the way round is.
//!
//! Usage: cargo run -p lb-nav --example tricks_debug -- <graph.lbnav>

use lb_nav::plan::{path_time, plain, plan};
use lb_nav::{LinkKind, NodeId};

fn main() {
    let path = std::env::args().nth(1).expect("usage: tricks_debug <graph.lbnav>");
    let bytes = std::fs::read(&path).expect("read");
    let (_, g) = lb_nav::store::read(&bytes).expect("parse");
    usage(&g);
    for kind in [LinkKind::LongJump, LinkKind::GaussBoost] {
        let mut rows = Vec::new();
        for a in 0..g.len() as NodeId {
            for l in g.links(a).iter().filter(|l| l.kind == kind) {
                let (na, nb) = (g.node(a), g.node(l.to));
                let d = (nb.origin - na.origin).truncate().length();
                let dz = nb.origin.z - na.origin.z;
                let round = plan(&g, a, l.to, &plain).map(|p| path_time(&g, &p));
                rows.push((a, l.to, d, dz, l.cost, round));
            }
        }
        let up = rows.iter().filter(|r| r.3 > 48.0).count();
        let down = rows.iter().filter(|r| r.3 < -48.0).count();
        let none = rows.iter().filter(|r| r.5.is_none()).count();
        println!(
            "{}: {} links, {} up, {} down, {} level; {} to places no plain way gets to",
            kind.as_str(),
            rows.len(),
            up,
            down,
            rows.len() - up - down,
            none
        );
        for r in rows.iter().step_by((rows.len() / 12).max(1)) {
            println!(
                "  {} -> {}: {:.0} u across, {:+.0} u up, cost {:.1} s, the way round {}",
                r.0,
                r.1,
                r.2,
                r.3,
                r.4,
                r.5.map_or("none".to_string(), |t| format!("{t:.1} s"))
            );
        }
    }
}

/// How often plans between random places take a trick link, and how far the places are.
fn usage(g: &lb_nav::NavGraph) {
    let mut rng = lb_core::rng::Pcg32::new(5, 5);
    let n = g.len() as i32;
    let (mut total, mut with_boost, mut with_lj, mut far) = (0, 0, 0, 0);
    let mut saved = 0.0f32;
    for _ in 0..3000 {
        let (a, b) = (rng.range_i32(0, n - 1) as NodeId, rng.range_i32(0, n - 1) as NodeId);
        let Some(plain_path) = plan(g, a, b, &plain) else {
            continue;
        };
        let Some(tricky) = plan(g, a, b, &|_, _| 0.0) else {
            continue;
        };
        total += 1;
        let d = g.node(a).origin.distance(g.node(b).origin);
        if d > 1400.0 || plain_path.len() > 13 {
            far += 1;
        }
        let kinds: Vec<LinkKind> = tricky
            .windows(2)
            .filter_map(|w| g.find_link(w[0], w[1]))
            .map(|l| l.kind)
            .collect();
        if kinds.contains(&LinkKind::GaussBoost) {
            with_boost += 1;
            saved += path_time(g, &plain_path) - path_time(g, &tricky);
        }
        if kinds.contains(&LinkKind::LongJump) {
            with_lj += 1;
        }
    }
    println!(
        "{total} random plans: {with_boost} take a gauss boost (saving {:.1} s on average), {with_lj} a long jump; {far} \
         far enough for a gauss jump on the way",
        saved / with_boost.max(1) as f32
    );
}
