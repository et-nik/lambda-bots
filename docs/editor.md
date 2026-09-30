# Map editor in the browser

`lb-editor` is a small server that runs next to a game server, or next to the macOS stand, and serves a page with
the map in 3D, as Hammer shows it: textured and lit, with every brush entity and point entity, which can be selected
and inspected, and the bots' navigation graph over it, which can be edited. It reads the maps and WADs straight from
the game's mod directory, and the graph and the map's overlays from the bots' directory.

State: viewing (M6.1a) and editing the graph (M6.2): nodes put in and moved, links of every kind (long jumps and
gauss boosts included), forbidden zones and places, checked as the server checks them, saved to
`maps/<map>/editor.yaml` with the graph they make (`editor.lbnav`) and applied on the running server. Coming next: why a link was not taken, and live bots with their paths and commands
(M6.3).

## Running

On the macOS stand:

```sh
scripts/build-editor.sh
build/editor/host/lb-editor --game ~/Git/half-life/xash3d-fwgs-apple-arm64/valve
# lb-editor: open http://127.0.0.1:8090/?token=…
```

Next to a server on another machine, run a Linux build there as the user that runs the game server, and open the
page through an SSH tunnel:

```sh
scripts/build-editor.sh --linux                  # build/editor/x86_64-unknown-linux-musl/lb-editor, static
scp build/editor/x86_64-unknown-linux-musl/lb-editor server:
ssh server 'sudo -u gameap ./lb-editor --game /srv/…/valve'   # prints the link with the token
ssh -N -L 8090:127.0.0.1:8090 server             # on your machine, then open the printed link
```

Arguments:
- `--game <dir>`: the mod directory (`valve`). Maps and WADs are read from it and from `valve_addon` and
  `valve_downloads` next to it, in the engine's order.
- `--install <dir>`: the bots' directory, `<game>/addons/lambdabots` by default: the maps' overlays, the server's
  graphs (`nav/`) and its config.
- `--listen <addr>`: a loopback address and port, `127.0.0.1:8090` by default.
- `--token <token>`: the access token, also taken from `LB_EDITOR_TOKEN`; a random one by default, printed with the
  link.
- `--secret <secret>`, `--telemetry-port <n>`: the server's command channel, when `config/lambdabots.yaml` does not
  have it (it was set with `lb_telemetry_secret` or `lb_telemetry_port` in `server.cfg`); the secret also comes from
  `LB_TELEMETRY_SECRET`.

## Access

The page lets its user do what rcon does (from M6.2 on it saves map markup and sends commands to the server), and any
page open in the same browser could try to call it. So:
- the editor listens on loopback only and refuses other addresses;
- the printed link carries a token; the page turns it into an `HttpOnly`, `SameSite=Strict` cookie, and every
  request without the cookie gets 403. The cookie is named after the port the browser uses (`lb_editor_8090`):
  browsers keep cookies by host alone, and two editors, a local one and a tunnelled one, would take each other's;
- the `Host` header must be `127.0.0.1`, `localhost` or `[::1]` (any port, for tunnels), so a page whose domain is
  rebound to 127.0.0.1 gets nothing;
- a request that changes anything must carry `X-LB: 1`, which other origins cannot send without asking first.

## The page

| Control                     | 3D view                   | Top, front and side views |
|-----------------------------|---------------------------|---------------------------|
| right button, or the arrows | look around               | move the map              |
| W A S D                     | fly                       | —                         |
| Space or E, Ctrl or Q       | up, down                  | —                         |
| Shift                       | faster                    | faster (arrows)           |
| wheel                       | step forward and back     | zoom at the pointer       |
| click                       | select                    | select                    |
| F                           | bring the selection close | center the selection      |
| 1 2 3 4                     | 3D, top, front, side      | 3D, top, front, side      |

Buttons, lists, sliders and checkboxes give the keyboard back once used with the mouse, so Space, the arrows and the
letters keep flying the camera (Space does not press the last button clicked; an arrow does not switch the map). Ctrl
lowers the camera, so on a Mac the editor's shortcuts are ⌘Z, ⌘⇧Z and ⌘S; elsewhere they are Ctrl with the key,
pressed together (Ctrl held longer is flying down). On a Mac, Ctrl with an arrow is the system's: it switches desktops.

- **Layers**: the world, doors and lifts and buttons, breakables, walls (`func_wall`, `func_illusionary`, …), water,
  point entities; hidden at first: ladders, triggers, lights, the sky.
- **Cut** (2D views): hides what is nearer than a height or depth, to see the floors under the roofs; looking down it
  starts over the heads at the first spawn.
- **The inspector** shows the selected entity's keys, or the map's: faces, textures, lightmaps, the WADs it names
  (missing ones in red) and the entities by class.
- The map opened last is kept in the address (`#map=crossfire`).

Point entities are boxes: spawns are green hull-sized boxes, weapons and ammo orange, items blue, monsters red, the
rest small gray cubes.

## Editing the navigation graph

The graph is the one the server plays on: the newest graph in its cache (`nav/<map>/`) made from this build of the
map. When the server has not played the map, the editor makes one the same way, with the default physics (the panel
says which). The changes go into `maps/<map>/editor.yaml`, the file the in-game editor writes too (`docs/overlays.md`);
`overlay.yaml`, written by hand, is shown and applied after them but not changed.

The **Navigation** tab holds the tools, the changes and what is selected. Every change is checked at once as the
server will check it, and the list shows what it did: the links it put in and their kinds, or why it did nothing.

| Tool   | Key | Click                                    | Does                                                              |
|--------|-----|------------------------------------------|-------------------------------------------------------------------|
| Select | V   | a node, a link, an entity                | shows it: a node's links in and out with kinds and costs          |
| Link   | L   | a node, then another; on from there      | a link of the kind chosen, both ways or one, trusted or checked   |
| Unlink | U   | a link                                   | takes it out, both ways or one                                    |
| Node   | N   | the floor where a node is missing        | a node, linked with the nodes around wherever the links check out |
| Move   | M   | a node, dragged to where it should stand | the node set down there, its links checked again                  |
| Forbid | X   | where bots must never plan through       | a zone of the radius chosen                                       |
| Place  | P   | where the named place is                 | a place with a name, a radius and tags                            |
| Route  | R   | where a bot starts, then where it goes   | the way a bot plans with the changes, its time and its links      |

- **Link kinds.** *As it checks out*: the check finds what the link is (a walk, a drop, a door, a jump, ...). *Jump*:
  a jump is tried first. *Crouch*: walked crouched. *Long jump* and *gauss boost*: the trick is planned (only bots
  with the module, or with the gauss and its uranium, take them). A link that does not check out is not put in
  unless *trust* is on; a trusted trick gets a contract that makes a bot try it.
- **Move** drags a node: in 3D and from the top it slides over the surface under the pointer, in the front and side
  views it moves in the view's plane at its depth; the server sets it down on the floor. Its links are checked again
  there: those that still check out stay (a walk may turn into a drop), the others go unless they were trusted, and it
  is linked with the nodes around like a node put in. A node put in by the page moves in its own change, and the
  links drawn to it after it follow; a node of the graph gets a `move_node` change, which dragging it again changes.
  Esc while dragging leaves it where it was; a node in a forbidden zone does not move.
- **Forbid** clicked on a node puts the zone about the node. A zone holds for the changes after it too: nothing is
  linked into it, and no node is put in or moved into it. A link taken out stays out when a node put in or moved
  links itself with the nodes around.
- **Route** takes long jumps and gauss boosts when those boxes are on, and tells the time without tricks too.
- **The legend** under the header counts the links of each kind; a click hides or shows a kind, or the links that are
  off.
- The changes list is `editor.yaml` in order; a click brings a change into view and opens its fields (radius, kind,
  both ways, trust, a note); × removes it. Undo and redo: ⌘Z and ⌘⇧Z (Ctrl+Z and Ctrl+Shift+Z off a Mac). Delete
  removes the selected change or unlinks the selected link; Esc drops a link or a route half drawn.
- What changed is drawn through walls: links put in bright, links taken out dashed red, nodes put in or moved pink
  (a dashed line from where a moved node stood), forbidden nodes dark red.

**Saving and the server.** *Save* (⌘S, or Ctrl+S) writes `editor.yaml` unless it changed on disk since the page read it
(saved from the game meanwhile): then the page asks whether to keep its changes or take the file. Changes not saved
yet stay in the browser and come back when the map is opened again.

Save also writes `maps/<map>/editor.lbnav`: the server's graph with `editor.yaml` and `overlay.yaml` applied, as the
server reads them. The server plays on it as it is, with the node numbers the page shows, while it goes with what the
server would load: the same build of the map, generator and physics, and exactly the overlays on disk. Otherwise
(`overlay.yaml` changed by hand, `editor.yaml` saved from the game, a new generator) the server leaves it and applies
the overlays itself. It is written only over the server's own graph checked with the default physics; when it is not,
the notice after saving says why and one saved before is removed. `lb edit save` and `lb nav regen` remove it too. *Apply on server* sends `lb overlay reload` over
the server's command channel (the telemetry port + 1 on loopback, signed with the channel's secret): the server reads
the overlays of the map it is on again. Without a secret the button is off and says why; `lb overlay reload` in the
server console does the same.

The in-game editor (`lb edit`) does not save over changes saved from the page either: it says the file changed and
asks to start editing again.

## How the map is drawn

`lb-mapmesh` turns a BSP into what the page draws: every model's faces in triangles grouped by texture and lightmap
page, the textures (the map's own, else from the WADs the worldspawn names) and the lightmaps packed into pages. The
server keeps the last four maps built and sends everything addressed by the map's fingerprint, so the browser caches
it until the file changes.

The lightmap of a face is sized from its texture coordinates, as the engine does. Compilers sum them in doubles,
engines in floats, and a face whose corners fall on a luxel line comes out one luxel larger or smaller in one of them.
The size that fits the gap to the next face's lightmap in the lighting lump wins: all 116 maps of the stand fit
(crossfire needs float precision for 280 faces, rapidcore for 403); a face nothing fits is counted in the manifest's
`stats.lightmaps.mismatched` and shown in the inspector.

Not drawn as the game does: only the first light style; brush entities where the compiler left them (trains before
they move to their first stop, doors closed); models and sprites; the sky (a flat color when its layer is on).

## Building

```sh
scripts/build-editor.sh            # this machine: npm ci, the page, then lb-editor with the page in it
scripts/build-editor.sh --linux    # static Linux x86_64 (musl), cross-built with rust-lld
```

Without `tools/editor/dist` the binary still builds (the Rust side never needs npm) and serves only the API and a
note. While working on the page, `npm run dev` in `tools/editor` serves it on 127.0.0.1:5173 and passes `/api` to a
running `lb-editor`; open the editor's own link once first, for the cookie.

| Path                | Contents                                                         |
|---------------------|------------------------------------------------------------------|
| `crates/lb-mapmesh` | render lumps, WAD3, mip textures, lightmap sizes and pages, mesh |
| `crates/lb-editor`  | the server: routes, access guard, map and WAD caches             |
| `tools/editor`      | the page: Vue 3, TypeScript, three.js, Pinia (Vite)              |
