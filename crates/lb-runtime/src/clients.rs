//! Connected clients as seen by the runtime (lifecycle, names, auth ids).

use lb_core::handles::{ClientRef, MapEpoch};
use lb_raw::{ClientEvent, ClientEventKind};

#[derive(Clone, Debug, Default)]
pub struct ClientInfo {
    pub connected: bool,
    pub in_game: bool,
    pub is_fake: bool,
    pub is_ours: bool,
    pub userid: i32,
    pub bot_gen: u32,
    pub name: String,
    pub model: String,
    pub auth_id: String,
    pub address: String,
}

#[derive(Default)]
pub struct Clients {
    pub slots: Vec<ClientInfo>,
}

impl Clients {
    pub fn reset(&mut self, max_clients: usize) {
        self.slots = vec![ClientInfo::default(); max_clients + 1];
    }

    pub fn get(&self, slot: u8) -> Option<&ClientInfo> {
        self.slots.get(slot as usize).filter(|c| c.connected)
    }

    pub fn apply(&mut self, e: &ClientEvent) {
        let Some(c) = self.slots.get_mut(e.slot as usize) else {
            return;
        };
        match e.kind {
            ClientEventKind::Connect => {
                *c = ClientInfo {
                    connected: true,
                    in_game: false,
                    is_fake: e.is_fake,
                    is_ours: e.is_ours,
                    userid: e.userid,
                    bot_gen: e.bot_gen,
                    name: String::from_utf8_lossy(&e.name).into_owned(),
                    model: String::from_utf8_lossy(&e.model).into_owned(),
                    auth_id: String::from_utf8_lossy(&e.auth_id).into_owned(),
                    address: String::from_utf8_lossy(&e.address).into_owned(),
                };
            }
            ClientEventKind::PutInServer => {
                c.connected = true;
                c.in_game = true;
                c.userid = e.userid;
                if !e.auth_id.is_empty() {
                    c.auth_id = String::from_utf8_lossy(&e.auth_id).into_owned();
                }
            }
            ClientEventKind::Info => {
                if !e.name.is_empty() {
                    c.name = String::from_utf8_lossy(&e.name).into_owned();
                }
                if !e.model.is_empty() {
                    c.model = String::from_utf8_lossy(&e.model).into_owned();
                }
            }
            ClientEventKind::Disconnect | ClientEventKind::ConnectRejected => *c = ClientInfo::default(),
        }
    }

    /// Humans that occupy or are about to occupy a slot.
    pub fn humans(&self, count_connecting: bool) -> u32 {
        self.slots
            .iter()
            .filter(|c| c.connected && !c.is_fake && (count_connecting || c.in_game))
            .count() as u32
    }

    pub fn occupied(&self) -> u32 {
        self.slots.iter().filter(|c| c.connected).count() as u32
    }

    pub fn find_userid(&self, userid: i32) -> Option<u8> {
        self.slots
            .iter()
            .position(|c| c.connected && c.userid == userid)
            .map(|i| i as u8)
    }

    pub fn find_name(&self, name: &str) -> Option<u8> {
        self.slots
            .iter()
            .position(|c| c.connected && c.name.eq_ignore_ascii_case(name))
            .map(|i| i as u8)
    }

    pub fn client_ref(&self, epoch: MapEpoch, slot: u8) -> Option<ClientRef> {
        self.get(slot).map(|c| ClientRef {
            epoch,
            slot,
            userid: c.userid,
        })
    }
}
