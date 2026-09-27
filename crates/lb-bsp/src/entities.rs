//! The entity lump: `{ "key" "value" ... }` blocks in file order.

use lb_core::Vec3;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Entity {
    /// Keys in file order; duplicates are kept (multi_manager uses them).
    pub kv: Vec<(String, String)>,
}

impl Entity {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.kv.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    pub fn classname(&self) -> &str {
        self.get("classname").unwrap_or("")
    }

    pub fn vec3(&self, key: &str) -> Option<Vec3> {
        let v = self.get(key)?;
        let mut it = v.split_whitespace().map(|p| p.parse::<f32>().ok());
        Some(Vec3::new(it.next()??, it.next()??, it.next()??))
    }

    pub fn origin(&self) -> Vec3 {
        self.vec3("origin").unwrap_or(Vec3::ZERO)
    }

    pub fn int(&self, key: &str) -> Option<i32> {
        self.get(key)?.trim().parse::<f32>().ok().map(|v| v as i32)
    }

    pub fn spawnflags(&self) -> i32 {
        self.int("spawnflags").unwrap_or(0)
    }

    /// Brush model index for `"model" "*N"`.
    pub fn brush_model(&self) -> Option<usize> {
        self.get("model")?.strip_prefix('*')?.parse().ok()
    }

    /// Yaw from `angle` or `angles`, degrees.
    pub fn yaw(&self) -> f32 {
        if let Some(a) = self.get("angle").and_then(|v| v.trim().parse::<f32>().ok()) {
            return a;
        }
        self.vec3("angles").map(|a| a.y).unwrap_or(0.0)
    }
}

/// Parses the entity lump text. Malformed trailing data is ignored, like the engine's `ED_ParseEdict` loop.
pub fn parse_entities(text: &str) -> Vec<Entity> {
    let mut out = Vec::new();
    let mut tokens = Tokens {
        s: text.as_bytes(),
        i: 0,
    };
    loop {
        match tokens.next() {
            Some(t) if t == "{" => {}
            _ => break,
        }
        let mut ent = Entity::default();
        loop {
            let Some(key) = tokens.next() else { return out };
            if key == "}" {
                break;
            }
            let Some(value) = tokens.next() else { return out };
            if value == "}" {
                break;
            }
            ent.kv.push((key, value));
        }
        out.push(ent);
    }
    out
}

struct Tokens<'a> {
    s: &'a [u8],
    i: usize,
}

impl Tokens<'_> {
    fn next(&mut self) -> Option<String> {
        while self.i < self.s.len() && self.s[self.i].is_ascii_whitespace() {
            self.i += 1;
        }
        if self.i >= self.s.len() {
            return None;
        }
        match self.s[self.i] {
            b'{' | b'}' => {
                self.i += 1;
                Some((self.s[self.i - 1] as char).to_string())
            }
            b'"' => {
                self.i += 1;
                let start = self.i;
                while self.i < self.s.len() && self.s[self.i] != b'"' {
                    self.i += 1;
                }
                let v = String::from_utf8_lossy(&self.s[start..self.i]).into_owned();
                self.i = (self.i + 1).min(self.s.len());
                Some(v)
            }
            _ => {
                let start = self.i;
                while self.i < self.s.len() && !self.s[self.i].is_ascii_whitespace() {
                    self.i += 1;
                }
                Some(String::from_utf8_lossy(&self.s[start..self.i]).into_owned())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_blocks_and_keys() {
        let text = r#"{
"classname" "worldspawn"
"wad" "\halflife.wad"
}
{
"origin" "-624 1168 -1560"
"angle" "90"
"classname" "info_player_deathmatch"
}
{
"model" "*12"
"classname" "func_door"
"spawnflags" "256"
}"#;
        let ents = parse_entities(text);
        assert_eq!(ents.len(), 3);
        assert_eq!(ents[1].origin(), Vec3::new(-624.0, 1168.0, -1560.0));
        assert_eq!(ents[1].yaw(), 90.0);
        assert_eq!(ents[2].brush_model(), Some(12));
        assert_eq!(ents[2].spawnflags(), 256);
    }
}
