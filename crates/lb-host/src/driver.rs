//! Command emission for fake clients: exact msec accounting, button latching and seeding.
//!
//! The motor is serviced every server frame, but commands are sent to the engine at a
//! configurable rate (like a human client's `cl_cmdrate`). The sum of emitted msec never exceeds
//! elapsed frame time, so the engine's clock-window guard never ignores bot commands.

/// Largest msec sent in one command; the engine splits longer commands with rounding loss.
pub const MAX_CMD_MSEC: u32 = 50;

#[derive(Clone, Debug)]
pub struct CommandDriver {
    acc_ms: f64,
    quantum_ms: f64,
    max_debt_ms: f64,
    latched: u16,
    last_sent_buttons: u16,
    pub total_sent_ms: u64,
    pub total_frame_ms: f64,
    pub dropped_debt_ms: f64,
    pub commands_sent: u64,
}

/// What actually went out to the engine; motor actions advance only on this.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SentCommand {
    pub msec: u8,
    pub buttons: u16,
    pub previous_buttons: u16,
}

impl SentCommand {
    pub fn pressed(&self, button: u16) -> bool {
        self.buttons & button != 0 && self.previous_buttons & button == 0
    }
}

impl CommandDriver {
    /// `cmd_rate` = commands per second; 0 sends a command every frame (when at least 1 ms elapsed).
    pub fn new(cmd_rate: f64, max_debt_ms: f64) -> CommandDriver {
        let quantum_ms = if cmd_rate > 0.0 { 1000.0 / cmd_rate } else { 1.0 };
        CommandDriver {
            acc_ms: 0.0,
            quantum_ms: quantum_ms.max(1.0),
            max_debt_ms,
            latched: 0,
            last_sent_buttons: 0,
            total_sent_ms: 0,
            total_frame_ms: 0.0,
            dropped_debt_ms: 0.0,
            commands_sent: 0,
        }
    }

    pub fn set_rate(&mut self, cmd_rate: f64) {
        self.quantum_ms = if cmd_rate > 0.0 {
            (1000.0 / cmd_rate).max(1.0)
        } else {
            1.0
        };
    }

    pub fn reset(&mut self) {
        self.acc_ms = 0.0;
        self.latched = 0;
        self.last_sent_buttons = 0;
    }

    pub fn last_sent_buttons(&self) -> u16 {
        self.last_sent_buttons
    }

    /// Time not yet sent: frame time minus sent msec minus dropped debt. Stays below one quantum when nothing leaks.
    pub fn unsent_ms(&self) -> f64 {
        self.total_frame_ms - self.total_sent_ms as f64 - self.dropped_debt_ms
    }

    /// Called every frame with the frame time and the buttons the motor wants held right now.
    /// Returns the command to send this frame, if any.
    pub fn tick(&mut self, frame_ms: f64, held_buttons: u16) -> Option<SentCommand> {
        self.acc_ms += frame_ms.max(0.0);
        self.total_frame_ms += frame_ms.max(0.0);
        self.latched |= held_buttons;
        if self.acc_ms > self.max_debt_ms {
            self.dropped_debt_ms += self.acc_ms - self.max_debt_ms;
            self.acc_ms = self.max_debt_ms;
        }
        if self.acc_ms + 1e-9 < self.quantum_ms {
            return None;
        }
        let msec = (self.acc_ms.floor() as u32).min(MAX_CMD_MSEC);
        if msec == 0 {
            return None;
        }
        self.acc_ms -= msec as f64;
        let buttons = self.latched;
        let sent = SentCommand {
            msec: msec as u8,
            buttons,
            previous_buttons: self.last_sent_buttons,
        };
        self.last_sent_buttons = buttons;
        self.latched = buttons_to_keep(buttons, held_buttons);
        self.total_sent_ms += msec as u64;
        self.commands_sent += 1;
        Some(sent)
    }
}

/// After sending, only buttons still held keep their latch; a button pressed and released between
/// two commands has already been delivered once.
fn buttons_to_keep(_sent: u16, held: u16) -> u16 {
    held
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn never_sends_more_time_than_elapsed() {
        for &(fps, rate) in &[
            (1000.0, 100.0),
            (1000.0, 0.0),
            (500.0, 250.0),
            (100.0, 100.0),
            (60.0, 100.0),
            (1000.0, 30.0),
        ] {
            let mut d = CommandDriver::new(rate, 200.0);
            let frame_ms = 1000.0 / fps;
            let frames = (fps * 60.0) as usize;
            let mut sent_ms = 0u64;
            for _ in 0..frames {
                if let Some(c) = d.tick(frame_ms, 0) {
                    sent_ms += c.msec as u64;
                    assert!(c.msec as u32 <= MAX_CMD_MSEC);
                }
            }
            let elapsed = frame_ms * frames as f64;
            assert!(
                sent_ms as f64 <= elapsed + 1e-6,
                "fps {fps} rate {rate}: sent {sent_ms} > {elapsed}"
            );
            assert!(
                elapsed - sent_ms as f64 <= 50.0,
                "fps {fps} rate {rate}: drift {}",
                elapsed - sent_ms as f64
            );
            assert!(
                d.unsent_ms() < d.quantum_ms.max(frame_ms) + 1e-6,
                "fps {fps} rate {rate}: {}",
                d.unsent_ms()
            );
        }
    }

    #[test]
    fn short_press_between_commands_is_delivered() {
        let mut d = CommandDriver::new(100.0, 200.0);
        let mut sent = Vec::new();
        for i in 0..40 {
            let held = if (12..14).contains(&i) { 2 } else { 0 };
            if let Some(c) = d.tick(1.0, held) {
                sent.push(c);
            }
        }
        assert!(
            sent.iter().any(|c| c.pressed(2)),
            "a 2 ms press must reach the engine at 100 Hz"
        );
        assert!(
            sent.last().is_some_and(|c| c.buttons & 2 == 0),
            "and be released afterwards"
        );
    }

    #[test]
    fn long_stall_debt_is_bounded() {
        let mut d = CommandDriver::new(100.0, 200.0);
        let c = d.tick(300.0, 0).unwrap();
        assert_eq!(c.msec as u32, MAX_CMD_MSEC);
        assert!(d.dropped_debt_ms >= 99.0);
    }
}
