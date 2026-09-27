//! Interned strings announced by the adapter (`LB_EV_STRING`), user message and event names.

#[derive(Default, Debug)]
pub struct StringTable {
    strings: Vec<Option<Vec<u8>>>,
    msg_names: Vec<(i32, Vec<u8>)>,
    event_names: Vec<(i32, Vec<u8>)>,
}

impl StringTable {
    pub fn set_string(&mut self, id: u16, bytes: Vec<u8>) {
        let i = id as usize;
        if self.strings.len() <= i {
            self.strings.resize(i + 1, None);
        }
        self.strings[i] = Some(bytes);
    }

    pub fn string(&self, id: u16) -> Option<&[u8]> {
        self.strings.get(id as usize).and_then(|s| s.as_deref())
    }

    pub fn string_lossy(&self, id: u16) -> String {
        self.string(id)
            .map(|b| String::from_utf8_lossy(b).into_owned())
            .unwrap_or_default()
    }

    pub fn set_msg_name(&mut self, id: i32, name: Vec<u8>) {
        match self.msg_names.iter_mut().find(|(i, _)| *i == id) {
            Some(entry) => entry.1 = name,
            None => self.msg_names.push((id, name)),
        }
    }

    pub fn msg_name(&self, id: i32) -> Option<&[u8]> {
        self.msg_names.iter().find(|(i, _)| *i == id).map(|(_, n)| n.as_slice())
    }

    pub fn msg_id(&self, name: &[u8]) -> Option<i32> {
        self.msg_names
            .iter()
            .find(|(_, n)| n.as_slice() == name)
            .map(|(i, _)| *i)
    }

    pub fn set_event_name(&mut self, index: i32, name: Vec<u8>) {
        match self.event_names.iter_mut().find(|(i, _)| *i == index) {
            Some(entry) => entry.1 = name,
            None => self.event_names.push((index, name)),
        }
    }

    pub fn event_name(&self, index: i32) -> Option<&[u8]> {
        self.event_names
            .iter()
            .find(|(i, _)| *i == index)
            .map(|(_, n)| n.as_slice())
    }

    pub fn event_count(&self) -> usize {
        self.event_names.len()
    }

    pub fn clear_map_scoped(&mut self) {
        self.event_names.clear();
    }
}
