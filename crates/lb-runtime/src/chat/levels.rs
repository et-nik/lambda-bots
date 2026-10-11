//! The weapons of a map's GunGame levels for the prompt, read as the Half-Life GunGame plugin (AMXX) reads them when
//! the map starts: the `<level>` blocks of `<map>.ini` in its config directory, else those in the `<sets>` of
//! `gungame.ini` there (the plugin's `gg_cfg_file` unless the server names another). A level's weapon is the first
//! `weapon_` it equips, without the prefix (`9mmAR`, `crowbar`); a level that equips nothing is none, for the plugin
//! too.

use std::path::{Path, PathBuf};

/// The levels of `map` in `dir` (`<game>/addons/amxmodx/configs/gungame`) and the file they come from; `None`
/// without either file.
pub fn load(dir: &Path, map: &str) -> Option<(Vec<String>, PathBuf)> {
    // The plugin takes the map's name into 32 bytes, the last for the end of the string.
    let name = map.get(..31).unwrap_or(map);
    let own = dir.join(format!("{name}.ini"));
    if own.is_file() {
        return Some((read(&own).map_or_else(Vec::new, |t| parse(&t, false)), own));
    }
    let main = dir.join("gungame.ini");
    let text = read(&main)?;
    Some((parse(&text, true), main))
}

/// The plugin's files are not always UTF-8: a comment in a Windows code page must not cost the levels.
fn read(path: &Path) -> Option<String> {
    std::fs::read(path)
        .ok()
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
}

#[derive(Clone, Copy, PartialEq)]
enum Block {
    Outside,
    Cvars,
    Sets,
    Mods,
}

enum At {
    Outside,
    Level,
    Equip,
    /// An item with settings of its own (`<weapon_9mmAR>`): it counts at its end tag.
    Item(String),
    Inflictors,
}

/// The levels of a file's `text`; `main`: the plugin's main file, whose levels are those in `<sets>`.
fn parse(text: &str, main: bool) -> Vec<String> {
    let mut levels = Vec::new();
    let mut block = if main { Block::Outside } else { Block::Sets };
    let mut at = At::Outside;
    let mut items: Vec<String> = Vec::new();
    for line in text.lines() {
        // What the plugin's trim takes off: ASCII spaces alone (a no-break space or a byte order mark stays, and a tag
        // with one is no tag).
        let line = line.trim_matches(|c: char| matches!(c, '\t'..='\r' | ' '));
        if line.is_empty() || line.starts_with(';') || line.starts_with("//") {
            continue;
        }
        match block {
            Block::Outside => {
                block = match line {
                    "<cvars>" => Block::Cvars,
                    "<sets>" => Block::Sets,
                    "<mods>" => Block::Mods,
                    _ => Block::Outside,
                };
                continue;
            }
            Block::Cvars | Block::Mods => {
                let end = if block == Block::Cvars { "</cvars>" } else { "</mods>" };
                if line == end {
                    block = Block::Outside;
                }
                continue;
            }
            Block::Sets if main && line == "</sets>" => {
                block = Block::Outside;
                continue;
            }
            Block::Sets => {}
        }
        at = match at {
            At::Outside if line == "<level>" => {
                items.clear();
                At::Level
            }
            At::Level => match line {
                "</level>" => {
                    let weapon = items.iter().find(|i| i.starts_with("weapon_")).or(items.first());
                    if let Some(weapon) = weapon {
                        levels.push(weapon.strip_prefix("weapon_").unwrap_or(weapon).to_string());
                    }
                    At::Outside
                }
                "<equip>" => At::Equip,
                "<inflictors>" => At::Inflictors,
                _ => At::Level,
            },
            At::Equip if line == "</equip>" => At::Level,
            At::Equip if line.len() >= 2 && line.starts_with('<') && line.ends_with('>') => {
                At::Item(line[1..line.len() - 1].to_string())
            }
            At::Equip => {
                items.push(line.to_string());
                At::Equip
            }
            At::Item(item) if line.strip_prefix("</").and_then(|l| l.strip_suffix('>')) == Some(item.as_str()) => {
                items.push(item);
                At::Equip
            }
            At::Inflictors if line == "</inflictors>" => At::Level,
            at => at,
        };
    }
    levels
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A map's own file: levels at the top, an item with settings, a level with two weapons, a commented-out level
    /// and one that equips nothing.
    const MAP: &str = "\
; levels of the test map
<level>
\tkills = 3
\ticon = d_9mmAR

\t<equip>
\t\t<weapon_9mmAR>
\t\t\tclip = 50
\t\t\trefil_time_2 = 20
\t\t</weapon_9mmAR>
\t</equip>

\t<inflictors>
\t\tweapon_9mmAR
\t\tgrenade
\t</inflictors>
</level>

;<level>
;\t<equip>
;\t\tweapon_rpg
;\t</equip>
;</level>

<level>
\tkills = 2
\t<equip>
\t\tweapon_tripmine
\t\tweapon_9mmhandgun
\t</equip>
</level>

<level>
\tkills = 1
</level>

<level>
\t<equip>
\t\titem_longjump
\t\tweapon_crowbar
\t\titem_battery
\t</equip>
</level>
";

    /// The plugin's main file: cvars, the levels in `<sets>`, a warm-up in `<mods>`.
    const MAIN: &str = "\
<cvars>
\t\"gg_enabled\" \"1\"
</cvars>
<level>
\t<equip>
\t\tweapon_egon
\t</equip>
</level>
<sets>
\t<level>
\t\t<equip>
\t\t\tweapon_hornetgun
\t\t</equip>
\t</level>
\t<level>
\t\t<equip>
\t\t\tweapon_crowbar
\t\t</equip>
\t</level>
</sets>
<mods>
\t<valve>
\t\t<warmup>
\t\t\t<equip>
\t\t\t\tweapon_crowbar
\t\t\t</equip>
\t\t</warmup>
\t\t<level>
\t\t\t<equip>
\t\t\t\tweapon_gauss
\t\t\t</equip>
\t\t</level>
\t</valve>
</mods>
";

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lb-gg-levels-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_map_s_file_gives_its_levels() {
        assert_eq!(parse(MAP, false), ["9mmAR", "tripmine", "crowbar"]);
        let crlf = MAP.replace('\n', "\r\n");
        assert_eq!(parse(&crlf, false), ["9mmAR", "tripmine", "crowbar"]);
    }

    #[test]
    fn the_main_file_s_levels_are_in_its_sets() {
        assert_eq!(parse(MAIN, true), ["hornetgun", "crowbar"]);
    }

    #[test]
    fn a_tag_with_a_byte_order_mark_or_a_no_break_space_is_lost_as_the_plugin_loses_it() {
        let text = format!("\u{feff}{MAP}");
        assert_eq!(parse(&text, false), ["9mmAR", "tripmine", "crowbar"]);
        let rpg = "<level>\n<equip>\nweapon_rpg\n</equip>\n</level>\n";
        let text = format!("\u{feff}{rpg}{MAP}");
        assert_eq!(parse(&text, false), ["9mmAR", "tripmine", "crowbar"]);
        let text = format!("{}{MAP}", rpg.replace("<level>", "\u{a0}<level>"));
        assert_eq!(parse(&text, false), ["9mmAR", "tripmine", "crowbar"]);
        let text = format!("{}{MAP}", rpg.replace("<level>", "\x0b<level>\x0c"));
        assert_eq!(parse(&text, false), ["rpg", "9mmAR", "tripmine", "crowbar"]);
    }

    #[test]
    fn the_map_s_file_comes_first_then_the_main_one() {
        let d = dir("load");
        assert_eq!(load(&d, "gg_frost"), None);
        std::fs::write(d.join("gungame.ini"), MAIN).unwrap();
        assert_eq!(
            load(&d, "gg_frost"),
            Some((vec!["hornetgun".into(), "crowbar".into()], d.join("gungame.ini")))
        );
        let mut bytes = b"; \xcf\xf0\xe8\xec\xe5\xf0\n".to_vec();
        bytes.extend_from_slice(MAP.as_bytes());
        std::fs::write(d.join("gg_frost.ini"), bytes).unwrap();
        assert_eq!(
            load(&d, "gg_frost"),
            Some((
                vec!["9mmAR".into(), "tripmine".into(), "crowbar".into()],
                d.join("gg_frost.ini")
            )),
            "a comment in Windows-1251"
        );
        assert_eq!(load(&d, "gg_other").map(|(l, _)| l.len()), Some(2));

        let long = "gg_a_map_with_a_name_much_too_long_b2";
        std::fs::write(d.join(format!("{long}.ini")), MAP).unwrap();
        assert_eq!(
            load(&d, long).map(|(_, file)| file),
            Some(d.join("gungame.ini")),
            "the plugin looks for the name's first 31 bytes"
        );
        std::fs::write(d.join(format!("{}.ini", &long[..31])), MAP).unwrap();
        assert_eq!(load(&d, long).map(|(l, _)| l.len()), Some(3));
        let _ = std::fs::remove_dir_all(&d);
    }
}
