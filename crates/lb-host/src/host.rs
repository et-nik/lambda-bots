use lb_core::Vec3;
use lb_ffi::{LbBotCommand, LbMoveFeedback};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CvarHandle(pub u16);

#[derive(Clone, Debug)]
pub struct CvarSpec {
    pub name: String,
    pub default_value: String,
    pub flags: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrintKind {
    Console,
    Center,
    Chat,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TraceKind {
    Line,
    Hull(u8),
    Model(lb_ffi::LbEntRef),
}

#[derive(Clone, Copy, Debug)]
pub struct TraceRequest {
    pub start: Vec3,
    pub end: Vec3,
    pub kind: TraceKind,
    pub ignore_monsters: bool,
    pub ignore_glass: bool,
    pub ignore: Option<lb_ffi::LbEntRef>,
}

#[derive(Serialize, Deserialize)]
#[serde(remote = "lb_ffi::LbEntRef")]
struct EntRefDef {
    index: u16,
    pad: u16,
    serial: u32,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct TraceResult {
    pub fraction: f32,
    pub end_pos: Vec3,
    pub plane_normal: Vec3,
    pub plane_dist: f32,
    #[serde(with = "EntRefDef")]
    pub hit: lb_ffi::LbEntRef,
    pub hitgroup: i32,
    pub all_solid: bool,
    pub start_solid: bool,
    pub in_open: bool,
    pub in_water: bool,
    pub hit_rendermode: u8,
    pub hit_renderfx: u8,
    pub hit_renderamt: f32,
    pub hit_solid: u8,
    pub hit_classname: u16,
}

#[derive(Clone, Debug)]
pub struct TrackRule {
    pub pattern: String,
    pub prefix: bool,
    pub kind: u8,
}

pub type EntitySnapshot = lb_ffi::LbEntitySnapshot;

#[derive(Clone, Debug)]
pub struct CreateBotRequest {
    pub name: String,
    pub infokeys: Vec<(String, String)>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CreateBotOutcome {
    Created { slot: u8, userid: i32, bot_gen: u32 },
    ServerFull,
    Rejected(String),
    Failed(i32),
}

#[derive(Clone, Debug)]
pub struct DebugPrim {
    pub line: Option<(Vec3, Vec3)>,
    pub text: Option<String>,
    pub color: [u8; 3],
    pub width: u8,
    pub life_ds: u8,
    pub channel: u8,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CompatFacts {
    pub engine_kind: u8,
    pub metamod_has_hook_tables: bool,
    pub rehlds_version: Option<(u8, u8, i32)>,
    pub channels: u32,
    pub engine_version: String,
    pub metamod_version: String,
    pub gamedll_desc: String,
    pub gamedll_path: String,
}

/// Everything the core may ask the engine to do. Implemented by `FfiHost` in production and by
/// test hosts; all calls happen on the engine main thread inside an `lb_core_*` call.
pub trait Host {
    fn server_print(&mut self, text: &str);
    fn client_print(&mut self, slot: u8, kind: PrintKind, text: &str);
    fn server_command(&mut self, command: &str);
    fn cvar_register(&mut self, specs: &[CvarSpec]) -> Vec<Option<CvarHandle>>;
    fn cvar_find(&mut self, name: &str) -> Option<CvarHandle>;
    fn cvar_float(&mut self, handle: CvarHandle) -> f32;
    fn cvar_string(&mut self, handle: CvarHandle) -> String;
    fn cvar_set(&mut self, handle: CvarHandle, value: &str);
    fn trace(&mut self, req: &TraceRequest) -> TraceResult;
    fn point_contents(&mut self, point: Vec3) -> i32;
    fn set_track_rules(&mut self, rules: &[TrackRule]) -> bool;
    fn snapshot_entities(&mut self, kind_mask: u32, out: &mut Vec<EntitySnapshot>);
    fn get_entity(&mut self, ent: lb_ffi::LbEntRef) -> Option<EntitySnapshot>;
    fn set_capture_mask(&mut self, mask: &[u8; 32]) -> bool;
    fn resolve_user_msg(&mut self, name: &str) -> Option<(i32, i32)>;
    fn create_bot(&mut self, req: &CreateBotRequest) -> CreateBotOutcome;
    fn kick_bot(&mut self, slot: u8, bot_gen: u32, reason: &str) -> bool;
    fn bot_client_command(&mut self, slot: u8, bot_gen: u32, argv: &[&str]) -> bool;
    fn run_player_moves(&mut self, cmds: &[LbBotCommand], feedback: &mut Vec<LbMoveFeedback>) -> bool;
    fn physics_key(&mut self, slot: u8, key: &str) -> String;
    fn client_info_key(&mut self, slot: u8, key: &str) -> String;
    fn player_stats(&mut self, slot: u8) -> Option<(i32, i32)>;
    fn load_file(&mut self, path: &str) -> Option<Vec<u8>>;
    /// Weapon prediction data of our bot in `slot` (what its client would be sent); `None` when unsupported.
    fn weapon_state(&mut self, _slot: u8) -> Option<lb_ffi::LbWeaponState> {
        None
    }
    fn send_debug(&mut self, slot: u8, prims: &[DebugPrim]) -> bool;
    fn compat_facts(&mut self) -> CompatFacts;
}
