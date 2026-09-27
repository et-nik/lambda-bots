//! Deterministic random streams. The generator is implemented here (PCG32 XSH-RR, SplitMix64
//! seeding) so replays never change when third-party crates are upgraded.

#[derive(Clone, Debug)]
pub struct Pcg32 {
    state: u64,
    inc: u64,
}

const PCG_MULT: u64 = 6364136223846793005;

impl Pcg32 {
    pub fn new(seed: u64, stream: u64) -> Pcg32 {
        let mut rng = Pcg32 {
            state: 0,
            inc: (stream << 1) | 1,
        };
        rng.next_u32();
        rng.state = rng.state.wrapping_add(seed);
        rng.next_u32();
        rng
    }

    pub fn next_u32(&mut self) -> u32 {
        let old = self.state;
        self.state = old.wrapping_mul(PCG_MULT).wrapping_add(self.inc);
        let xorshifted = (((old >> 18) ^ old) >> 27) as u32;
        let rot = (old >> 59) as u32;
        xorshifted.rotate_right(rot)
    }

    /// Uniform in `[0, 1)` with 24 bits of precision.
    pub fn next_f32(&mut self) -> f32 {
        (self.next_u32() >> 8) as f32 * (1.0 / 16_777_216.0)
    }

    /// Uniform in `[lo, hi]` (yapb `rg(a, b)` semantics for floats).
    pub fn range_f32(&mut self, lo: f32, hi: f32) -> f32 {
        if hi <= lo { lo } else { lo + (hi - lo) * self.next_f32() }
    }

    /// Uniform integer in `[lo, hi]` inclusive.
    pub fn range_i32(&mut self, lo: i32, hi: i32) -> i32 {
        if hi <= lo {
            return lo;
        }
        let span = (hi as i64 - lo as i64 + 1) as u64;
        (lo as i64 + (self.next_u32() as u64 % span) as i64) as i32
    }

    /// `true` with probability `percent / 100`.
    pub fn chance(&mut self, percent: f32) -> bool {
        self.next_f32() * 100.0 < percent
    }

    /// Standard normal deviate (Box–Muller; one of the pair is dropped so every call uses exactly two draws).
    pub fn normal(&mut self) -> f32 {
        let u1 = self.next_f32().max(1e-7);
        let u2 = self.next_f32();
        (-2.0 * u1.ln()).sqrt() * (std::f32::consts::TAU * u2).cos()
    }
}

pub fn splitmix64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E3779B97F4A7C15);
    let mut z = x;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
    z ^ (z >> 31)
}

/// FNV-1a 64, used to derive stable seeds from names.
pub fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RngDomain {
    Perception = 1,
    Decision = 2,
    Combat = 3,
    Motor = 4,
    Cosmetic = 5,
    Profile = 6,
}

/// Independent streams per bot, so an extra cosmetic draw never changes combat outcomes.
#[derive(Clone, Debug)]
pub struct BotRng {
    pub perception: Pcg32,
    pub decision: Pcg32,
    pub combat: Pcg32,
    pub motor: Pcg32,
    pub cosmetic: Pcg32,
    pub profile: Pcg32,
}

impl BotRng {
    /// `persona_seed` belongs to the bot's personality. The profile stream depends on it alone, so traits
    /// drawn from it are the same in every session; the other streams also mix in the session's master seed.
    pub fn new(master_seed: u64, persona_seed: u64) -> BotRng {
        let stream = |d: RngDomain| {
            let seed = splitmix64(master_seed ^ splitmix64(persona_seed.wrapping_mul(31).wrapping_add(d as u64)));
            Pcg32::new(seed, d as u64)
        };
        let profile = splitmix64(persona_seed ^ RngDomain::Profile as u64);
        BotRng {
            perception: stream(RngDomain::Perception),
            decision: stream(RngDomain::Decision),
            combat: stream(RngDomain::Combat),
            motor: stream(RngDomain::Motor),
            cosmetic: stream(RngDomain::Cosmetic),
            profile: Pcg32::new(profile, RngDomain::Profile as u64),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pcg32_reference_vector() {
        // Reference output of pcg32_srandom(42, 54) from the PCG paper's demo.
        let mut rng = Pcg32::new(42, 54);
        let got: Vec<u32> = (0..6).map(|_| rng.next_u32()).collect();
        assert_eq!(
            got,
            vec![0xa15c02b7, 0x7b47f409, 0xba1d3330, 0x83d2f293, 0xbfa4784b, 0xcbed606e]
        );
    }

    #[test]
    fn ranges_are_inclusive_and_bounded() {
        let mut rng = Pcg32::new(1, 1);
        for _ in 0..10_000 {
            let v = rng.range_i32(3, 5);
            assert!((3..=5).contains(&v));
            let f = rng.range_f32(0.5, 1.5);
            assert!((0.5..=1.5).contains(&f));
        }
    }

    #[test]
    fn normal_deviates_have_unit_spread() {
        let mut rng = Pcg32::new(3, 3);
        let v: Vec<f32> = (0..20_000).map(|_| rng.normal()).collect();
        let mean = v.iter().sum::<f32>() / v.len() as f32;
        let var = v.iter().map(|x| (x - mean).powi(2)).sum::<f32>() / v.len() as f32;
        assert!(mean.abs() < 0.03 && (var - 1.0).abs() < 0.05, "{mean} {var}");
    }

    #[test]
    fn domains_are_independent() {
        let mut a = BotRng::new(7, 1);
        let b = BotRng::new(7, 1);
        a.cosmetic.next_u32();
        assert_eq!(a.combat.clone().next_u32(), b.combat.clone().next_u32());
    }
}
