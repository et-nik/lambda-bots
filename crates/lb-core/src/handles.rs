/// Incremented on every map start; any deferred result from an older epoch is discarded.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MapEpoch(pub u32);

/// Our bot in a given client slot; `generation` changes whenever the slot is reused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BotId {
    pub slot: u8,
    pub generation: u32,
}

/// Any connected client, valid only within one map epoch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ClientRef {
    pub epoch: MapEpoch,
    pub slot: u8,
    pub userid: i32,
}

/// An engine entity, validated by the edict serial number.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct EntityRef {
    pub epoch: MapEpoch,
    pub index: u16,
    pub serial: u32,
}

/// Tag attached to deferred work so stale results can be rejected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JobTag {
    pub epoch: MapEpoch,
    pub bot: Option<BotId>,
    pub version: u64,
}

impl JobTag {
    pub fn is_current(&self, epoch: MapEpoch, bot: Option<BotId>, version: u64) -> bool {
        self.epoch == epoch && self.bot == bot && self.version == version
    }
}
