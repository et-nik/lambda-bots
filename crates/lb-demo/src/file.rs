//! The demo file: header, directory of segments, frames.
//!
//! Layout as the engine and the HLTV proxy write it (ReHLDS `HLTV/common/DemoFile.cpp`).

use crate::Error;
use crate::bits::latin1;

pub(crate) const HEADER_SIZE: usize = 544;
pub(crate) const ENTRY_SIZE: usize = 92;
const MAX_ENTRIES: usize = 1024;
pub(crate) const DEMO_PROTOCOL: i32 = 5;
const PATH_SIZE: usize = 260;
/// `demo_info_t`: the client's `ref_params_t`, the last `usercmd_t`, `movevars_t`, view and view model.
const DEMO_INFO_SIZE: usize = 436;
const SEQUENCE_INFO_SIZE: usize = 28;
const MAX_MESSAGE: usize = 1 << 20;

#[derive(Clone, Debug)]
pub struct Header {
    pub demo_protocol: i32,
    pub net_protocol: i32,
    pub map: String,
    pub game_dir: String,
    pub map_crc: u32,
}

#[derive(Clone, Debug)]
pub struct Segment {
    /// 0 for the loading segment, 1 for playback.
    pub kind: i32,
    pub description: String,
    pub track_time: f32,
    pub frame_count: i32,
    offset: usize,
    length: usize,
}

/// What the recording client saw of itself (zero in HLTV recordings).
#[derive(Clone, Copy, Debug, Default)]
pub struct ClientView {
    pub vieworg: [f32; 3],
    pub viewangles: [f32; 3],
    pub frametime: f32,
    pub time: f32,
    pub spectator: i32,
    pub onground: i32,
    pub waterlevel: i32,
    pub simvel: [f32; 3],
    pub simorg: [f32; 3],
    pub viewheight: [f32; 3],
    pub cl_viewangles: [f32; 3],
    pub health: i32,
    pub punchangle: [f32; 3],
    pub maxclients: i32,
    pub viewentity: i32,
    pub playernum: i32,
}

/// The client's command sent with this frame.
#[derive(Clone, Copy, Debug, Default)]
pub struct UserCmd {
    pub lerp_msec: i16,
    pub msec: u8,
    pub viewangles: [f32; 3],
    pub forwardmove: f32,
    pub sidemove: f32,
    pub upmove: f32,
    pub buttons: u16,
    pub impulse: u8,
    pub weaponselect: u8,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct EventArgs {
    pub flags: i32,
    pub entindex: i32,
    pub origin: [f32; 3],
    pub angles: [f32; 3],
    pub velocity: [f32; 3],
    pub ducking: i32,
    pub fparam1: f32,
    pub fparam2: f32,
    pub iparam1: i32,
    pub iparam2: i32,
    pub bparam1: i32,
    pub bparam2: i32,
}

#[derive(Clone, Debug)]
pub enum FrameData<'a> {
    /// A network message as the client received it (types 0 and 1).
    Net {
        view: ClientView,
        cmd: UserCmd,
        /// The client's incoming sequence: deltas name their base frame by its low byte.
        incoming_sequence: i32,
        msg: &'a [u8],
    },
    DemoStart,
    ConsoleCommand(String),
    ClientData {
        origin: [f32; 3],
        viewangles: [f32; 3],
        weapon_bits: i32,
        fov: f32,
    },
    NextSection,
    Event {
        flags: i32,
        index: i32,
        delay: f32,
        args: EventArgs,
    },
    WeaponAnim {
        anim: i32,
        body: i32,
    },
    Sound {
        channel: i32,
        sample: String,
        attenuation: f32,
        volume: f32,
        flags: i32,
        pitch: i32,
    },
    DemoBuffer(&'a [u8]),
}

#[derive(Clone, Debug)]
pub struct Frame<'a> {
    pub time: f32,
    pub number: i32,
    pub data: FrameData<'a>,
}

pub struct DemoFile<'a> {
    bytes: &'a [u8],
    pub header: Header,
    pub segments: Vec<Segment>,
}

struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
    end: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], Error> {
        // The bytes themselves bound a read too: a segment may come from another file's directory.
        if n > self.end.min(self.bytes.len()).saturating_sub(self.pos) {
            return Err(Error::Truncated);
        }
        let s = &self.bytes[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    fn u8(&mut self) -> Result<u8, Error> {
        Ok(self.take(1)?[0])
    }

    fn i32(&mut self) -> Result<i32, Error> {
        Ok(i32::from_le_bytes(
            self.take(4)?.try_into().map_err(|_| Error::Truncated)?,
        ))
    }

    fn f32(&mut self) -> Result<f32, Error> {
        Ok(f32::from_le_bytes(
            self.take(4)?.try_into().map_err(|_| Error::Truncated)?,
        ))
    }

    fn vec3(&mut self) -> Result<[f32; 3], Error> {
        Ok([self.f32()?, self.f32()?, self.f32()?])
    }
}

fn le_i32(b: &[u8], o: usize) -> i32 {
    b.get(o..o + 4)
        .and_then(|s| s.try_into().ok())
        .map(i32::from_le_bytes)
        .unwrap_or(0)
}

fn le_f32(b: &[u8], o: usize) -> f32 {
    b.get(o..o + 4)
        .and_then(|s| s.try_into().ok())
        .map(f32::from_le_bytes)
        .unwrap_or(0.0)
}

fn le_vec3(b: &[u8], o: usize) -> [f32; 3] {
    [le_f32(b, o), le_f32(b, o + 4), le_f32(b, o + 8)]
}

fn cstr(b: &[u8]) -> String {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    latin1(&b[..end])
}

impl<'a> DemoFile<'a> {
    pub fn open(bytes: &'a [u8]) -> Result<Self, Error> {
        if bytes.len() < HEADER_SIZE || &bytes[..8] != b"HLDEMO\0\0" {
            return Err(Error::NotDemo);
        }
        let demo_protocol = le_i32(bytes, 8);
        let net_protocol = le_i32(bytes, 12);
        if demo_protocol != DEMO_PROTOCOL {
            return Err(Error::NotDemo);
        }
        let header = Header {
            demo_protocol,
            net_protocol,
            map: cstr(&bytes[16..16 + PATH_SIZE]),
            game_dir: cstr(&bytes[16 + PATH_SIZE..16 + 2 * PATH_SIZE]),
            map_crc: le_i32(bytes, 536) as u32,
        };
        let dir = le_i32(bytes, 540);
        let dir = usize::try_from(dir).map_err(|_| Error::NotDemo)?;
        let count = usize::try_from(le_i32(bytes, dir)).map_err(|_| Error::NotDemo)?;
        if dir + 4 > bytes.len() || count == 0 || count > MAX_ENTRIES || dir + 4 + count * ENTRY_SIZE > bytes.len() {
            return Err(Error::NotDemo);
        }
        let mut segments = Vec::with_capacity(count);
        for i in 0..count {
            let e = &bytes[dir + 4 + i * ENTRY_SIZE..dir + 4 + (i + 1) * ENTRY_SIZE];
            let offset = usize::try_from(le_i32(e, 84)).unwrap_or(usize::MAX);
            let length = usize::try_from(le_i32(e, 88)).unwrap_or(0);
            if offset > bytes.len() {
                continue;
            }
            segments.push(Segment {
                kind: le_i32(e, 0),
                description: cstr(&e[4..68]),
                track_time: le_f32(e, 76),
                frame_count: le_i32(e, 80),
                offset,
                length: length.min(bytes.len() - offset),
            });
        }
        Ok(Self {
            bytes,
            header,
            segments,
        })
    }

    /// The frames of one segment, up to its end marker. A frame cut short ends the list with an error.
    pub fn frames(&self, segment: &Segment) -> FrameIter<'a> {
        FrameIter {
            cur: Cursor {
                bytes: self.bytes,
                pos: segment.offset,
                end: segment.offset + segment.length,
            },
            done: false,
        }
    }
}

pub struct FrameIter<'a> {
    cur: Cursor<'a>,
    done: bool,
}

impl<'a> Iterator for FrameIter<'a> {
    type Item = Result<Frame<'a>, Error>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done || self.cur.pos >= self.cur.end {
            return None;
        }
        let r = read_frame(&mut self.cur);
        match &r {
            Ok(Frame {
                data: FrameData::NextSection,
                ..
            })
            | Err(_) => self.done = true,
            Ok(_) => {}
        }
        Some(r)
    }
}

fn read_frame<'a>(c: &mut Cursor<'a>) -> Result<Frame<'a>, Error> {
    let kind = c.u8()?;
    let time = c.f32()?;
    let number = c.i32()?;
    let data = match kind {
        2 => FrameData::DemoStart,
        3 => FrameData::ConsoleCommand(cstr(c.take(64)?)),
        4 => FrameData::ClientData {
            origin: c.vec3()?,
            viewangles: c.vec3()?,
            weapon_bits: c.i32()?,
            fov: c.f32()?,
        },
        5 => FrameData::NextSection,
        6 => {
            let flags = c.i32()?;
            let index = c.i32()?;
            let delay = c.f32()?;
            let a = c.take(72)?;
            FrameData::Event {
                flags,
                index,
                delay,
                args: EventArgs {
                    flags: le_i32(a, 0),
                    entindex: le_i32(a, 4),
                    origin: le_vec3(a, 8),
                    angles: le_vec3(a, 20),
                    velocity: le_vec3(a, 32),
                    ducking: le_i32(a, 44),
                    fparam1: le_f32(a, 48),
                    fparam2: le_f32(a, 52),
                    iparam1: le_i32(a, 56),
                    iparam2: le_i32(a, 60),
                    bparam1: le_i32(a, 64),
                    bparam2: le_i32(a, 68),
                },
            }
        }
        7 => FrameData::WeaponAnim {
            anim: c.i32()?,
            body: c.i32()?,
        },
        8 => {
            let channel = c.i32()?;
            let len = usize::try_from(c.i32()?).map_err(|_| Error::Truncated)?;
            let sample = cstr(c.take(len)?);
            FrameData::Sound {
                channel,
                sample,
                attenuation: c.f32()?,
                volume: c.f32()?,
                flags: c.i32()?,
                pitch: c.i32()?,
            }
        }
        9 => {
            let len = usize::try_from(c.i32()?).map_err(|_| Error::Truncated)?;
            FrameData::DemoBuffer(c.take(len)?)
        }
        _ => {
            let info = c.take(DEMO_INFO_SIZE)?;
            let seq = c.take(SEQUENCE_INFO_SIZE)?;
            let len = usize::try_from(c.i32()?).map_err(|_| Error::Truncated)?;
            if len > MAX_MESSAGE {
                return Err(Error::Truncated);
            }
            let msg = c.take(len)?;
            FrameData::Net {
                view: client_view(&info[4..236]),
                cmd: user_cmd(&info[236..288]),
                incoming_sequence: le_i32(seq, 0),
                msg,
            }
        }
    };
    Ok(Frame { time, number, data })
}

fn client_view(r: &[u8]) -> ClientView {
    ClientView {
        vieworg: le_vec3(r, 0),
        viewangles: le_vec3(r, 12),
        frametime: le_f32(r, 60),
        time: le_f32(r, 64),
        spectator: le_i32(r, 76),
        onground: le_i32(r, 80),
        waterlevel: le_i32(r, 84),
        simvel: le_vec3(r, 88),
        simorg: le_vec3(r, 100),
        viewheight: le_vec3(r, 112),
        cl_viewangles: le_vec3(r, 128),
        health: le_i32(r, 140),
        punchangle: le_vec3(r, 160),
        maxclients: le_i32(r, 172),
        viewentity: le_i32(r, 176),
        playernum: le_i32(r, 180),
    }
}

fn user_cmd(u: &[u8]) -> UserCmd {
    UserCmd {
        lerp_msec: i16::from_le_bytes([u[0], u[1]]),
        msec: u[2],
        viewangles: le_vec3(u, 4),
        forwardmove: le_f32(u, 16),
        sidemove: le_f32(u, 20),
        upmove: le_f32(u, 24),
        buttons: u16::from_le_bytes([u[30], u[31]]),
        impulse: u[32],
        weaponselect: u[33],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(dir: i32) -> Vec<u8> {
        let mut b = vec![0u8; HEADER_SIZE];
        b[..8].copy_from_slice(b"HLDEMO\0\0");
        b[8..12].copy_from_slice(&DEMO_PROTOCOL.to_le_bytes());
        b[12..16].copy_from_slice(&48i32.to_le_bytes());
        b[16..23].copy_from_slice(b"stalker");
        b[540..544].copy_from_slice(&dir.to_le_bytes());
        b
    }

    #[test]
    fn other_files_and_broken_directories_are_not_demos() {
        assert!(matches!(DemoFile::open(b"HL2DEMO\0 and more"), Err(Error::NotDemo)));
        assert!(matches!(DemoFile::open(&header(1 << 20)), Err(Error::NotDemo)));
        assert!(matches!(DemoFile::open(&header(-4)), Err(Error::NotDemo)));
    }

    #[test]
    fn the_directory_lists_its_segments() {
        let mut b = header(HEADER_SIZE as i32);
        b.extend_from_slice(&1i32.to_le_bytes());
        let mut e = vec![0u8; ENTRY_SIZE];
        e[4..12].copy_from_slice(b"LOADING\0");
        e[84..88].copy_from_slice(&(HEADER_SIZE as i32).to_le_bytes());
        e[88..92].copy_from_slice(&4i32.to_le_bytes());
        b.extend_from_slice(&e);
        let d = DemoFile::open(&b).unwrap();
        assert_eq!(d.header.map, "stalker");
        assert_eq!(d.segments.len(), 1);
        assert_eq!(d.segments[0].description, "LOADING");
    }

    #[test]
    fn a_segment_of_a_longer_file_ends_cut_short() {
        let demo = |len: usize| {
            let start = HEADER_SIZE + 4 + ENTRY_SIZE;
            let mut b = header(HEADER_SIZE as i32);
            b.extend_from_slice(&1i32.to_le_bytes());
            let mut e = vec![0u8; ENTRY_SIZE];
            e[84..88].copy_from_slice(&(start as i32).to_le_bytes());
            e[88..92].copy_from_slice(&(len as i32).to_le_bytes());
            b.extend_from_slice(&e);
            // A demo start frame: type, time, number.
            b.push(2);
            b.resize(start + len, 0);
            b
        };
        let (long, short) = (demo(300), demo(9));
        let long = DemoFile::open(&long).unwrap();
        let short = DemoFile::open(&short).unwrap();
        let frames: Vec<_> = short.frames(&long.segments[0]).collect();
        assert!(matches!(
            frames[0],
            Ok(Frame {
                data: FrameData::DemoStart,
                ..
            })
        ));
        assert!(matches!(frames.last(), Some(Err(Error::Truncated))));
    }
}
