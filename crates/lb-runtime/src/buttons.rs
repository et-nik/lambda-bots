//! Engine button bits (`IN_*`).

pub const IN_ATTACK: u16 = 1 << 0;
pub const IN_JUMP: u16 = 1 << 1;
pub const IN_DUCK: u16 = 1 << 2;
pub const IN_FORWARD: u16 = 1 << 3;
pub const IN_BACK: u16 = 1 << 4;
pub const IN_USE: u16 = 1 << 5;
pub const IN_MOVELEFT: u16 = 1 << 9;
pub const IN_MOVERIGHT: u16 = 1 << 10;
pub const IN_ATTACK2: u16 = 1 << 11;
pub const IN_RELOAD: u16 = 1 << 13;
pub const IN_SCORE: u16 = 1 << 15;

/// Direction buttons consistent with analog moves (ladders and some mods read only the buttons).
pub fn direction_buttons(forward: f32, side: f32) -> u16 {
    let mut b = 0;
    if forward > 0.0 {
        b |= IN_FORWARD;
    } else if forward < 0.0 {
        b |= IN_BACK;
    }
    if side > 0.0 {
        b |= IN_MOVERIGHT;
    } else if side < 0.0 {
        b |= IN_MOVELEFT;
    }
    b
}
