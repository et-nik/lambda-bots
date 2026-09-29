# Map overlays and the editor

An overlay is what a map needs besides what the generator finds in it: named places, and patches to its navigation
graph — a link the generator missed, one it made that does not work, an area bots should never go. Each map may have
two overlay files in `addons/lambdabots/maps/<map>/`:

| File           | Written by                                                     | Applied |
|----------------|----------------------------------------------------------------|---------|
| `editor.yaml`  | the editors: in the browser (`docs/editor.md`) and in the game | first   |
| `overlay.yaml` | a person, by hand                                              | last    |

Both have the same schema. They are read with the map's graph (`lb overlay reload` reads them again at any time), and
their patches are applied to the graph in order. A patch that does not apply is reported in the server log and by
`lb-cli nav validate-overlay`; the others still apply. A file that does not parse is left out whole, with the error
and the line in the log.

## The file

```yaml
schema: lambdabots/overlay@1
map: crossfire
bsp_size: 1034112          # optional: not applied to another build of the map
places:
  - name: bunker
    at: [0, -2300, -1820]
    radius: 300
    tags: [shelter]
nav:
  patches:
    - op: forbid           # bots never plan through the nodes within `radius`
      at: [100, 200, -1820]
      radius: 64
      note: the airstrike's kill zone
    - op: add_link         # checked like a generated link; `trust: true` adds it even if the check fails
      from: [-8, 456, -1820]
      to: [-40, 472, -1775]
      kind: jump           # optional hint: try a jump first
      both: false          # also the way back
    - op: remove_link      # a link the generator made that does not work
      from: [69, 305, -1660]
      to: [8, 304, -1660]
      both: true
    - op: add_node         # a node where the generator put none, linked with the nodes around
      at: [-203, 660, -1813]
      note: the crate top
```

Points are player origins (the centre of the standing hull) or near them: each end of a patch is the node nearest to
its point, within 64 units. `lb edit info` and `lb-cli nav path` print nodes with their origins.

A link put in is classified by the same code as a generated one: a walk is hull-checked, a jump is simulated, a door
needs its opener, a ladder its mount point. `kind` steers the check:
- none (or another kind): the check finds what the link is;
- `jump`: a jump is tried first;
- `crouch`: walked crouched;
- `longjump`, `gauss_boost`: the trick is planned as the generator plans it; only bots with the long jump module, or
  with the gauss and its uranium, take such a link.

If the check fails the link is not added, unless the patch says `trust: true`; a trusted link is marked so in
`lb nav` and the live check still checks it. A trusted trick gets a contract that makes a bot try it.

A node put in (`add_node`) is set down on the floor under `at` (crouched where standing does not fit) and linked both
ways with the nodes within 384 units wherever the links check out; the patches after it may use it as an end.

Places are named spots for behavior (shelters, sniper nests) and for people reading logs; `lb overlay` lists them.

## Checking an overlay offline

```
lb-cli nav validate-overlay maps/crossfire.bsp addons/lambdabots/maps/crossfire/overlay.yaml
```

makes the graph of the map, applies the overlay and reports what did not apply. The exit code is 0 only when every
patch applied. `lb-cli config check` checks the schema of any overlay file without the map.

## The editor

The editor lets an admin walk the map with the graph drawn around them and record changes into `editor.yaml`. It is
off unless the server allows it: `lb_editor 1` in the server console (or `rcon lb_editor 1` from the game, or
`access.editor_enabled: true` in the main config). It only takes commands from a player with `lb` access: a SteamID
in `access.admins`, or `access.password` set on the server and the same value given with `setinfo _lbpw <password>`
in the game console (on a listen server the host always has access). Run the `lb edit` commands from the game console
(`cmd lb edit on` if the client does not pass the command on by itself):

| Command                                   | Does                                                                            |
|-------------------------------------------|---------------------------------------------------------------------------------|
| `lb edit on` / `off`                      | starts or stops editing (changes not saved are dropped)                         |
| `lb edit show nodes\|links\|off`          | what is drawn: nodes within 512 units, and links from those within 256          |
| `lb edit mark`                            | marks the node nearest to you (drawn as a tall red beam)                        |
| `lb edit link [kind] [both] [trust]`      | a link from the marked node to the node nearest to you (kinds as in `add_link`) |
| `lb edit unlink [both]`                   | takes the link from the marked node to the nearest one out                      |
| `lb edit forbid [radius]`                 | bots never plan within `radius` (48) of where you stand                         |
| `lb edit place <name> [radius] [tags...]` | a named place where you stand                                                   |
| `lb edit info`                            | the nearest node: flags and every link with its kind, cost and whether it is on |
| `lb edit undo`                            | takes back the last change since the last save                                  |
| `lb edit save`                            | writes `editor.yaml` and applies the overlays to the graph at once              |

`lb edit save` does not write over `editor.yaml` saved meanwhile from the web editor: it says the file changed, and
editing starts again with `lb edit off` and `lb edit on`.

Nodes are drawn by what they are: items yellow, ladders cyan, water blue, crouch spots purple, buttons and lifts
orange, the rest green. Links are drawn by kind: walk grey, crouch purple, jump green, drop yellow, ladder cyan, swim
blue, door orange, lift violet, teleport white, breakable brown, push pink; a link that is off is red.

`lb nav regen` throws away the map's kept graphs and makes the graph again (for a changed generator or map); the
overlays are applied to the new graph the same way.
