//! Game mode detection: FFA deathmatch, team deathmatch, GunGame (FFA or team).

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GameModeKind {
    Ffa,
    Teamplay,
    GunGame { team: bool },
}

#[derive(Clone, Debug, Default)]
pub struct ModeInputs {
    /// `lb_game_mode`: -1 auto, 0 FFA, 1 teamplay.
    pub forced_mode: i32,
    pub teamplay_cvar: bool,
    pub teamplay_message: bool,
    /// `auto`, `on`, `off`.
    pub gungame_mode: String,
    pub gungame_cvar: Option<f32>,
    pub gungame_teamplay_cvar: Option<f32>,
    pub gungame_bridge_state: Option<bool>,
}

pub fn detect(i: &ModeInputs) -> GameModeKind {
    let teamplay = match i.forced_mode {
        0 => false,
        1 => true,
        _ => i.teamplay_cvar || i.teamplay_message,
    };
    let gungame = match i.gungame_mode.as_str() {
        "on" => true,
        "off" => false,
        _ => i
            .gungame_bridge_state
            .unwrap_or(i.gungame_cvar.is_some_and(|v| v > 0.0)),
    };
    if gungame {
        GameModeKind::GunGame {
            team: i.gungame_teamplay_cvar.is_some_and(|v| v > 0.0),
        }
    } else if teamplay {
        GameModeKind::Teamplay
    } else {
        GameModeKind::Ffa
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gungame_detection_order() {
        let mut i = ModeInputs {
            forced_mode: -1,
            gungame_mode: "auto".into(),
            ..Default::default()
        };
        assert_eq!(detect(&i), GameModeKind::Ffa);
        i.gungame_cvar = Some(1.0);
        assert_eq!(detect(&i), GameModeKind::GunGame { team: false });
        i.gungame_bridge_state = Some(false);
        assert_eq!(detect(&i), GameModeKind::Ffa, "bridge state is authoritative");
        i.gungame_mode = "on".into();
        assert_eq!(detect(&i), GameModeKind::GunGame { team: false });
        i.gungame_mode = "off".into();
        i.teamplay_message = true;
        assert_eq!(detect(&i), GameModeKind::Teamplay);
    }
}
