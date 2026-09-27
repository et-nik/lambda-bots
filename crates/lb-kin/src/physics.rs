//! Movement settings (`sv_*` cvars and game rules) and fall damage.

/// Downward speed above which a landing hurts.
pub const SAFE_FALL_SPEED: f32 = 580.0;
/// Landing speed that kills with progressive fall damage (100 damage).
pub const FATAL_FALL_SPEED: f32 = 1024.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Physics {
    pub gravity: f32,
    /// `sv_maxspeed`; HL's `server.cfg` sets 270.
    pub maxspeed: f32,
    pub accelerate: f32,
    pub airaccelerate: f32,
    pub friction: f32,
    /// Friction multiplier near a ledge (`edgefriction`).
    pub edgefriction: f32,
    pub stopspeed: f32,
    pub stepsize: f32,
    pub maxvelocity: f32,
    /// Jumping faster than 1.7 × maxspeed crops the speed (the SDK always; BugfixedHL only with `mp_bunnyhop 0`).
    pub bunnyhop_cap: bool,
    /// Holding use on the ground slows to a third of maxspeed (HL 25th anniversary, BugfixedHL `mp_useslowdown 1`).
    pub use_slowdown: bool,
    /// `mp_falldamage 1`: damage grows with the landing speed; 0: a flat 10 above the safe speed.
    pub progressive_fall_damage: bool,
}

impl Default for Physics {
    fn default() -> Physics {
        Physics {
            gravity: 800.0,
            maxspeed: 270.0,
            accelerate: 10.0,
            airaccelerate: 10.0,
            friction: 4.0,
            edgefriction: 2.0,
            stopspeed: 100.0,
            stepsize: 18.0,
            maxvelocity: 2000.0,
            bunnyhop_cap: true,
            use_slowdown: false,
            progressive_fall_damage: false,
        }
    }
}

impl Physics {
    /// Damage of landing at `speed` (armor does not absorb it; landing in water does not hurt).
    pub fn fall_damage(&self, speed: f32) -> f32 {
        if speed <= SAFE_FALL_SPEED {
            0.0
        } else if self.progressive_fall_damage {
            (speed - SAFE_FALL_SPEED) * 100.0 / (FATAL_FALL_SPEED - SAFE_FALL_SPEED)
        } else {
            10.0
        }
    }

    /// Landing speed after falling `height` units from rest.
    pub fn fall_speed(&self, height: f32) -> f32 {
        (2.0 * self.gravity * height.max(0.0)).sqrt()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fall_damage_modes() {
        let mut p = Physics::default();
        let safe = SAFE_FALL_SPEED * SAFE_FALL_SPEED / (2.0 * p.gravity);
        assert!((safe - 210.25).abs() < 0.01, "safe drop from rest is 210.25 u");
        assert_eq!(p.fall_damage(p.fall_speed(200.0)), 0.0);
        assert_eq!(p.fall_damage(p.fall_speed(400.0)), 10.0);
        p.progressive_fall_damage = true;
        assert!((p.fall_damage(FATAL_FALL_SPEED) - 100.0).abs() < 1e-3);
    }
}
