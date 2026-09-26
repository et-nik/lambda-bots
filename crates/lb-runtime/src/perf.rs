//! Core time per server frame (frame_pre + frame_post), kept as a ring of recent samples.

const RING: usize = 4096;

pub struct CoreTimes {
    ring: Box<[u32; RING]>,
    next: usize,
    len: usize,
    pre_ns: u64,
    pub last_ns: u64,
    pub max_ns: u64,
    pub total_ns: u64,
    pub frames: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Serialize)]
pub struct Percentiles {
    pub p50_us: f64,
    pub p95_us: f64,
    pub p99_us: f64,
    pub max_us: f64,
    pub samples: usize,
}

impl Default for CoreTimes {
    fn default() -> Self {
        CoreTimes {
            ring: Box::new([0; RING]),
            next: 0,
            len: 0,
            pre_ns: 0,
            last_ns: 0,
            max_ns: 0,
            total_ns: 0,
            frames: 0,
        }
    }
}

impl CoreTimes {
    pub fn set_pre(&mut self, ns: u64) {
        self.pre_ns = ns;
    }

    /// Closes the frame: `post_ns` plus the time recorded by [`CoreTimes::set_pre`].
    pub fn finish_frame(&mut self, post_ns: u64) {
        let ns = self.pre_ns + post_ns;
        self.pre_ns = 0;
        self.last_ns = ns;
        self.max_ns = self.max_ns.max(ns);
        self.total_ns += ns;
        self.frames += 1;
        self.ring[self.next] = ns.min(u32::MAX as u64) as u32;
        self.next = (self.next + 1) % RING;
        self.len = (self.len + 1).min(RING);
    }

    pub fn percentiles(&self) -> Percentiles {
        if self.len == 0 {
            return Percentiles::default();
        }
        let mut v: Vec<u32> = self.ring[..self.len].to_vec();
        v.sort_unstable();
        let at = |q: f64| v[((v.len() - 1) as f64 * q).round() as usize] as f64 / 1000.0;
        Percentiles {
            p50_us: at(0.5),
            p95_us: at(0.95),
            p99_us: at(0.99),
            max_us: at(1.0),
            samples: v.len(),
        }
    }

    pub fn reset(&mut self) {
        *self = CoreTimes::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentiles_over_the_ring() {
        let mut t = CoreTimes::default();
        for i in 1..=100u64 {
            t.set_pre(i * 500);
            t.finish_frame(i * 500);
        }
        let p = t.percentiles();
        assert_eq!(p.samples, 100);
        assert!((p.p50_us - 51.0).abs() < 1.01, "{p:?}");
        assert_eq!(p.max_us, 100.0);
        assert_eq!(t.last_ns, 100_000);
    }

    #[test]
    fn ring_keeps_the_latest_samples() {
        let mut t = CoreTimes::default();
        for _ in 0..RING {
            t.finish_frame(1_000_000);
        }
        for _ in 0..RING {
            t.finish_frame(1_000);
        }
        assert_eq!(t.percentiles().max_us, 1.0);
        assert_eq!(t.max_ns, 1_000_000);
    }
}
