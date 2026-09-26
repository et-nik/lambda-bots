/// Token bucket refilled by simulation time, with a per-frame burst cap. Budgets are expressed in
/// deterministic work units (traces, node expansions) so replays stay exact.
#[derive(Clone, Debug)]
pub struct TokenBucket {
    rate_per_sec: f64,
    capacity: f64,
    per_frame_cap: u32,
    tokens: f64,
    taken_this_frame: u32,
}

impl TokenBucket {
    pub fn new(rate_per_sec: f64, capacity: f64, per_frame_cap: u32) -> TokenBucket {
        TokenBucket {
            rate_per_sec,
            capacity,
            per_frame_cap,
            tokens: capacity,
            taken_this_frame: 0,
        }
    }

    pub fn begin_frame(&mut self, dt: f64) {
        self.tokens = (self.tokens + self.rate_per_sec * dt.max(0.0)).min(self.capacity);
        self.taken_this_frame = 0;
    }

    pub fn try_take(&mut self, n: u32) -> bool {
        if self.taken_this_frame + n > self.per_frame_cap || self.tokens < n as f64 {
            return false;
        }
        self.tokens -= n as f64;
        self.taken_this_frame += n;
        true
    }

    pub fn available(&self) -> u32 {
        (self.per_frame_cap - self.taken_this_frame).min(self.tokens as u32)
    }

    pub fn set_rate(&mut self, rate_per_sec: f64) {
        self.rate_per_sec = rate_per_sec;
    }

    pub fn rate(&self) -> f64 {
        self.rate_per_sec
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn respects_rate_and_frame_cap() {
        let mut b = TokenBucket::new(100.0, 10.0, 4);
        b.begin_frame(0.0);
        assert!(b.try_take(4));
        assert!(!b.try_take(1));
        b.begin_frame(0.0);
        assert!(b.try_take(4));
        assert!(!b.try_take(4));
        b.begin_frame(0.1);
        assert!(b.try_take(4));
    }
}
