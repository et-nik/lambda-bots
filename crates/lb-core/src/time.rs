use core::ops::{Add, AddAssign, Sub};

/// Simulation time in seconds since map start, in double precision.
#[derive(Clone, Copy, Debug, Default, PartialEq, PartialOrd)]
pub struct SimTime(pub f64);

impl SimTime {
    pub const ZERO: SimTime = SimTime(0.0);

    pub fn secs(self) -> f64 {
        self.0
    }

    pub fn since(self, earlier: SimTime) -> f64 {
        self.0 - earlier.0
    }

    pub fn max(self, other: SimTime) -> SimTime {
        if other.0 > self.0 { other } else { self }
    }

    pub fn min(self, other: SimTime) -> SimTime {
        if other.0 < self.0 { other } else { self }
    }
}

impl Add<f64> for SimTime {
    type Output = SimTime;

    fn add(self, rhs: f64) -> SimTime {
        SimTime(self.0 + rhs)
    }
}

impl AddAssign<f64> for SimTime {
    fn add_assign(&mut self, rhs: f64) {
        self.0 += rhs;
    }
}

impl Sub for SimTime {
    type Output = f64;

    fn sub(self, rhs: SimTime) -> f64 {
        self.0 - rhs.0
    }
}

/// A one-shot deadline in simulation time.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Deadline(pub Option<SimTime>);

impl Deadline {
    pub const NONE: Deadline = Deadline(None);

    pub fn at(t: SimTime) -> Deadline {
        Deadline(Some(t))
    }

    pub fn after(now: SimTime, secs: f64) -> Deadline {
        Deadline(Some(now + secs))
    }

    pub fn passed(self, now: SimTime) -> bool {
        matches!(self.0, Some(t) if now >= t)
    }

    pub fn is_set(self) -> bool {
        self.0.is_some()
    }
}
