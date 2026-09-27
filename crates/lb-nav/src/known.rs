//! What a bot has learned about links by failing them (design v2 §9.5): each failure blocks the link for a while,
//! by its reason. Only the bot's own failures write here, so a door that closed out of sight does not change the
//! plans of bots that did not run into it. Links that break the same way for several bots are switched off for
//! everyone until the live check looks at them again (`LinkHealth`).

use rustc_hash::{FxHashMap, FxHashSet};

use crate::graph::NodeId;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FailReason {
    /// A player (or another bot) is in the way.
    TemporarilyOccupied,
    /// A door, lift or other mechanism did not do its part in time.
    WaitingForInteraction,
    /// The bot lacks what the link needs (health for a fall, a weapon for a breakable).
    MissingCapability,
    /// The bot could not carry the move out (missed a jump, fell off a ladder).
    ControllerFailure,
    /// The map does not let the move through where the graph says it does.
    GeometryInvalid,
}

impl FailReason {
    pub fn as_str(self) -> &'static str {
        match self {
            FailReason::TemporarilyOccupied => "occupied",
            FailReason::WaitingForInteraction => "mechanism",
            FailReason::MissingCapability => "capability",
            FailReason::ControllerFailure => "controller",
            FailReason::GeometryInvalid => "geometry",
        }
    }

    /// How long the link stays blocked after failing `repeats` times before.
    pub fn ttl(self, repeats: u32) -> f64 {
        match self {
            FailReason::TemporarilyOccupied => (4.0 + 2.0 * f64::from(repeats)).min(20.0),
            FailReason::WaitingForInteraction => 15.0,
            FailReason::MissingCapability => 30.0,
            FailReason::ControllerFailure => (10.0 * 2f64.powi(repeats.min(4) as i32)).min(120.0),
            FailReason::GeometryInvalid => 120.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LinkFailure {
    pub reason: FailReason,
    pub until: f64,
    pub repeats: u32,
    pub at: f64,
}

/// One bot's blocked links.
#[derive(Clone, Debug, Default)]
pub struct KnownChanges {
    links: FxHashMap<(NodeId, NodeId), LinkFailure>,
}

/// A failure is remembered this long after its block ends, so repeats grow the next block.
const MEMORY: f64 = 60.0;

impl KnownChanges {
    /// Records a failure; returns how long the link is blocked.
    pub fn fail(&mut self, from: NodeId, to: NodeId, reason: FailReason, now: f64) -> f64 {
        let repeats = self
            .links
            .get(&(from, to))
            .filter(|f| f.reason == reason && now < f.until + MEMORY)
            .map_or(0, |f| f.repeats + 1);
        let ttl = reason.ttl(repeats);
        self.links.insert(
            (from, to),
            LinkFailure {
                reason,
                until: now + ttl,
                repeats,
                at: now,
            },
        );
        ttl
    }

    pub fn blocked(&self, from: NodeId, to: NodeId, now: f64) -> bool {
        self.links.get(&(from, to)).is_some_and(|f| now < f.until)
    }

    /// Extra planning cost of the link: infinite while blocked.
    pub fn penalty(&self, from: NodeId, to: NodeId, now: f64) -> f32 {
        if self.blocked(from, to, now) {
            f32::INFINITY
        } else {
            0.0
        }
    }

    /// Forgets failures long past.
    pub fn expire(&mut self, now: f64) {
        self.links.retain(|_, f| now < f.until + MEMORY);
    }

    pub fn clear(&mut self) {
        self.links.clear();
    }

    /// Links blocked now, with their failure.
    pub fn active(&self, now: f64) -> impl Iterator<Item = ((NodeId, NodeId), &LinkFailure)> {
        self.links
            .iter()
            .filter(move |(_, f)| now < f.until)
            .map(|(k, f)| (*k, f))
    }
}

/// Geometry failures seen by several bots within this window switch the link off for everyone.
const SUSPECT_WINDOW: f64 = 600.0;
const SUSPECT_BOTS: usize = 3;

/// Links every bot avoids: the same geometry failure from several bots.
#[derive(Clone, Debug, Default)]
pub struct LinkHealth {
    reports: FxHashMap<(NodeId, NodeId), Vec<(u32, f64)>>,
    disabled: FxHashSet<(NodeId, NodeId)>,
}

impl LinkHealth {
    /// Records bot `bot`'s geometry failure on a link; returns true when that switched the link off.
    pub fn report(&mut self, from: NodeId, to: NodeId, bot: u32, now: f64) -> bool {
        let list = self.reports.entry((from, to)).or_default();
        list.retain(|(b, t)| *b != bot && now - t < SUSPECT_WINDOW);
        list.push((bot, now));
        if list.len() >= SUSPECT_BOTS && self.disabled.insert((from, to)) {
            return true;
        }
        false
    }

    /// Switches a link off for everyone (the live check disagreed with the offline one).
    pub fn disable(&mut self, from: NodeId, to: NodeId) {
        self.disabled.insert((from, to));
    }

    pub fn disabled(&self, from: NodeId, to: NodeId) -> bool {
        self.disabled.contains(&(from, to))
    }

    /// The live check found the link fine after all.
    pub fn restore(&mut self, from: NodeId, to: NodeId) {
        self.disabled.remove(&(from, to));
        self.reports.remove(&(from, to));
    }

    pub fn disabled_links(&self) -> impl Iterator<Item = (NodeId, NodeId)> + '_ {
        self.disabled.iter().copied()
    }

    pub fn clear(&mut self) {
        self.reports.clear();
        self.disabled.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_expire_and_repeats_grow() {
        let mut k = KnownChanges::default();
        assert_eq!(k.fail(1, 2, FailReason::TemporarilyOccupied, 0.0), 4.0);
        assert!(k.blocked(1, 2, 3.9) && !k.blocked(1, 2, 4.1));
        assert_eq!(
            k.fail(1, 2, FailReason::TemporarilyOccupied, 5.0),
            6.0,
            "second time longer"
        );
        assert_eq!(k.fail(3, 4, FailReason::ControllerFailure, 0.0), 10.0);
        assert_eq!(k.fail(3, 4, FailReason::ControllerFailure, 11.0), 20.0);
        assert_eq!(k.fail(5, 6, FailReason::GeometryInvalid, 0.0), 120.0);
        assert!(k.penalty(5, 6, 60.0).is_infinite());
        k.expire(1000.0);
        assert_eq!(k.active(1000.0).count(), 0);
    }

    #[test]
    fn three_bots_switch_a_link_off() {
        let mut h = LinkHealth::default();
        assert!(!h.report(1, 2, 7, 0.0));
        assert!(!h.report(1, 2, 7, 1.0), "the same bot counts once");
        assert!(!h.report(1, 2, 8, 2.0));
        assert!(h.report(1, 2, 9, 3.0));
        assert!(h.disabled(1, 2));
        h.restore(1, 2);
        assert!(!h.disabled(1, 2));
    }
}
