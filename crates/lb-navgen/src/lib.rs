//! The navigation graph generator: floods the floor of a map from where players start, places nodes over it,
//! links them and classifies every link against the map. Runs on a worker thread in the server and in `lb-cli`; the
//! same map and settings always give the same graph.

#![forbid(unsafe_code)]

pub mod build;
pub mod cache;
pub mod field;
pub mod patch;
pub mod place;
pub mod push;
pub mod report;
pub mod site;

pub use build::{GenOptions, Generated, generate};

#[cfg(test)]
mod tests {
    use super::*;
    use lb_bsp::BspWorld;
    use lb_bsp::mech::Mechanisms;

    #[test]
    fn crossfire_generates() {
        let Some(maps) = lb_bsp::test_maps_dir() else { return };
        let Ok(bsp) = std::fs::read(maps.join("crossfire.bsp")) else {
            return;
        };
        let mut world = BspWorld::load(&bsp).unwrap();
        let mech = Mechanisms::from_world(&world);
        let g = generate(&mut world, &mech, &GenOptions::default(), "generated");
        let s = &g.graph.stats;
        eprintln!(
            "crossfire: {} spans, {} nodes, {} links ({}), {} lift links, jumps {}/{}, {} traces, {} ms: {:?}",
            g.field.len(),
            s.nodes,
            s.links,
            s.kinds(),
            s.added,
            g.jumps.1,
            g.jumps.0,
            s.traces,
            s.millis,
            g.timings
        );
        let c = report::coverage(&g);
        eprintln!(
            "coverage {:.1}% ({} of {} spans), nodes {} reachable {} roundtrip {}, items {}/{}, spawns {}; cut off {:?}",
            c.ratio() * 100.0,
            c.covered,
            c.spans,
            c.nodes,
            c.reachable,
            c.roundtrip,
            c.items_ok,
            c.items,
            c.spawns,
            &c.cut_off[..c.cut_off.len().min(8)]
        );
        assert!(s.nodes > 500);
    }
}
