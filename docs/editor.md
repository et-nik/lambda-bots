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

Where things are:
- **The top bar**: the map, the views, *Layers* (the world, doors and lifts and buttons, breakables, walls such as
  `func_wall` and `func_illusionary`, water, point entities; hidden at first: ladders, triggers, lights, the sky), the
  light, wireframe, and in the 2D views *Cut*, which hides what is nearer than a height or depth to see the floors
  under the roofs (looking down it starts over the heads at the first spawn). On the right: undo, redo, *Save* and
  *Apply on server*; when *Apply* is off, its tooltip says why.
- **On the view**: the tools down the left edge, with their keys; the chosen tool's options along the top; the graph's
  legend at the bottom (a click hides or shows the graph, a kind of link, or the links that are off; the tooltips
  count them). *Selected only* (I) draws the links of what is selected alone, as thin arrows in their colours (the
  same size on screen however near) seen through walls (as lines while the Move tool's gizmo is on the node: its
  arrows are the only ones then): a node's, both ends' of a link, a change's nodes', the node a link is being drawn
  from; with nothing selected, all of them. What a change or a save came to pops up at the bottom for a few seconds.
- **The status bar**: *Changes* with their count (and a red ! when one did nothing or something went wrong with
  them), which opens and closes the list of changes under the view (C); where the camera is, what is under the
  pointer, what the tool does and how to fly.
- **The panel**: *Navigation* shows what is selected (a node, a link, a change, a route or an entity); *Map* shows the
  map's faces, textures, lightmaps, the WADs it names (missing ones in red), the graph's counts by kind and the
  entities by class.
- The map opened last is kept in the address (`#map=crossfire`).

Point entities are boxes: spawns are green hull-sized boxes, weapons and ammo orange, items blue, monsters red, the
rest small gray cubes.

## Editing the navigation graph

The graph is the one the server plays on: the newest graph in its cache (`nav/<map>/`) made from this build of the
map. When the server has not played the map, the editor makes one the same way, with the default physics (the panel
says which). The changes go into `maps/<map>/editor.yaml`, the file the in-game editor writes too (`docs/overlays.md`);
`overlay.yaml`, written by hand, is shown and applied after them but not changed.

Every change is checked at once as the server will check it, and the list of changes shows what it did: the links it
put in and their kinds, or why it did nothing (marked red); what is wrong in the graph is listed under *Problems*.

| Tool   | Key | Click                                  | Does                                                              |
|--------|-----|----------------------------------------|-------------------------------------------------------------------|
| Select | V   | a node, a link, an entity              | shows it in the panel, with what can be done to it                |
| Link   | L   | a node, then another; on from there    | a link of the kind chosen, both ways or one, trusted or checked   |
| Unlink | U   | a link                                 | takes it out, both ways or one                                    |
| Node   | N   | the floor where a node is missing      | a node, linked with the nodes around wherever the links check out |
| Move   | M   | a node, dragged; or the gizmo on it    | the node set down there, its links checked again                  |
| Forbid | X   | where bots must never plan through     | a zone of the radius chosen                                       |
| Place  | P   | where the named place is               | a place with a name, a radius and tags                            |
| Route  | R   | where a bot starts, then where it goes | the way a bot plans with the changes, its time and its links      |

- **Link kinds.** *As it checks out*: the check finds what the link is (a walk, a drop, a door, a jump, ...). *Jump*:
  a jump is tried first. *Crouch*: walked crouched. *Long jump* and *gauss boost*: the trick is planned (only bots
  with the module, or with the gauss and its uranium, take them). A link that does not check out is not put in
  unless *trust* is on; a trusted trick gets a contract that makes a bot try it.
- **Move** moves a node. Pressed on a node, it drags it: in 3D over the surface under the pointer. A click selects the
  node and puts the gizmo on it, drawn over everything: an arrow for each axis, lettered at its tip (X red, Y green, Z
  blue), moves it along that axis only, a square between two arrows in that plane, the ball in the middle freely. The top, front and side
  views show the two arrows of their plane, and the ball moves in the plane without following the surfaces, for
  moves to the unit. Moves snap to the grid (8 units at first; `[` and `]` change the step, shown along the top;
  Alt held moves off it), and the 2D views draw their grid at that step, or coarser where the lines would crowd.
  While dragging, the server sets the node down where it would stand and checks its links, as often as it keeps up:
  the node and its links follow, and a dashed line runs from where it stood. Letting go makes one change; Esc puts it back. The server sets a node down on the
  floor under the spot, so Z picks the floor it stands on. Its links are checked again there: those that still check
  out stay (a walk may turn into a drop), the others go unless they were trusted, and it is linked with the nodes
  around like a node put in. A node put in by the page moves in its own change, and the links drawn to it after it
  follow; a node of the graph gets a `move_node` change, which moving it again changes. A node in a forbidden zone
  does not move.
- **Forbid** clicked on a node puts the zone about the node. A zone holds for the changes after it too: nothing is
  linked into it, and no node is put in or moved into it. A link taken out stays out when a node put in or moved
  links itself with the nodes around.
- **Node** with *auto-link* off (along the top) puts the node in without links, to be linked by hand with Link; the
  change's own *auto-link* turns it either way later.
- **Route** takes long jumps and gauss boosts when those boxes are on, and tells the time without tricks too.
- The list of changes, under the view (*Changes* in the status bar, or C; it opens by itself when a save finds the
  file changed or the changes cannot be checked), is `editor.yaml` in order, one line each; the pointer on one lights
  its nodes and links up in the view, a click selects it: its fields (radius, kind, both ways, trust, auto-link, a
  note) and the nodes it came to open in the panel, and the view turns to it. × removes it. `overlay.yaml`'s changes
  are folded under *By hand*. Undo and redo: ⌘Z and ⌘⇧Z (Ctrl+Z and Ctrl+Shift+Z off a Mac). Delete removes the
  selected change or unlinks the selected link; Esc drops a link or a route half drawn and the selection.

**What is selected.** A node lists its links by the node at the other end: → a link out, ← a link in, ⇄ both alike,
each with its kind (in the legend's colours) and cost. The pointer on a line lights the links and that node up in the
view; a click on the number selects that node, on a link selects the link, and × takes the links with that node out.
The view turns to what is selected from the panel when it is out of sight (and flies to it when it is far); ‹ goes back
to what was selected before, ⌖ brings it close. Its *Position* can be typed, or stepped with ↑ and ↓ by the grid's step
(Shift: ten times): the node moves at once, and all the moves until the fields are left are one change and one step
of undo (Esc in a field takes them back). The node says which change put it in, moved it or shut it off (a click
selects the change), and it can be linked from (*Link from here*), routed from (*Route from here*) or shut off alone
(*Shut off*: a zone of 16 units about it). A link shows its kind, cost and the link back, and can be taken out one way
or both, or put in again checked as another kind or trusted. What is done from the panel keeps the selection and pops
up what it came to.
- What changed is drawn through walls: links put in bright, links taken out dashed red, nodes put in or moved pink
  (a dashed line from where a moved node stood), forbidden nodes dark red.

**Problems.** The page flags what is wrong in the graph with the changes, or worth a look, every time it checks them:

| Kind           | Level     | What it is                                                                                                                 |
|----------------|-----------|----------------------------------------------------------------------------------------------------------------------------|
| not put in     | error     | a link a change asked for does not check out, with why: what the check found                                               |
| failed in runs | error     | a link bots failed in the map's last test run (`lb test`) or its last commands (`lb do`), how often, the last time and why |
| missed in runs | error     | a trick found for a run came down elsewhere: where from, and how far off                                                   |
| does nothing   | attention | a change that does nothing, like taking out a link that is not there any more (error for the others)                       |
| falls          | attention | a walk that falls off a ledge on the way (more than a step down); where it lands is not checked as a drop's landing is     |
| weak           | attention | a trick link that lands in fewer than 80% of the tries a little off: the takeoff, the speed or the aim                     |
| not checked    | attention | a link put in trusted, without the check                                                                                   |

- **Why a link is not put in** is what its check found: "the long jump comes down 95 u past the landing", "taking off
  12 u further on is off the floor: the takeoff is at an edge", "looking 42° down with a push of 610: only 3 of 8 tries
  a little off land", "a running jump, ducking, comes down 88 u short of the landing, 274 u below it".
- **In the view**, seen through walls: a band under a link in the graph, red for an error and yellow otherwise (the
  worst of the link's problems); a dashed line where a link is asked for or failed and is not in the graph. *Problems
  N* in the legend shows or hides them.
- **In the Navigation tab**, under what is selected: the list, errors first. The pointer on a row lights its nodes and
  link up; a click selects the link, or, when it is not in the graph, the change it comes of.
- **Links and changes.** A link's card lists its problems, and the node's card marks its links that have some. A change
  that put a link in one way only is `!` in yellow in the list of changes, and its card tells why the other way does
  not check out.
- A link a change asked for and a later change put in after all is no problem. Test runs are read from
  `logs/tests/<map>-*.json` (the last one) and `logs/tests/<map>-orders.jsonl` (the last 50 commands); their places are
  matched to the nodes within 24 units.

**Saving and the server.** *Save* (⌘S, or Ctrl+S) writes `editor.yaml` unless it changed on disk since the page read it
(saved from the game meanwhile): then the page asks whether to keep its changes or take the file. Changes not saved
yet stay in the browser and come back when the map is opened again.

Save also writes `maps/<map>/editor.lbnav`: the server's graph with `editor.yaml` and `overlay.yaml` applied, as the
server reads them. The server plays on it as it is, with the node numbers the page shows, while it goes with what the
server would load: the same build of the map, generator and physics, and exactly the overlays on disk. Otherwise
(`overlay.yaml` changed by hand, `editor.yaml` saved from the game, a new generator) the server leaves it and applies
the overlays itself. It is written only over the server's own graph: when the server has not played this build of the
map with this version of the bots, the line under *Changes* says so after saving, and one saved before is removed.
Once the server has loaded the map, open it again in the page (or reload the page): the editor takes the server's new
graph then. `lb edit save` and `lb nav regen` remove `editor.lbnav` too.

Changes are checked, and `editor.lbnav` made, with the player physics the server's graph was made with (`sv_gravity`,
`sv_maxspeed`, `mp_bunnyhop`, `mp_falldamage`): the graph files keep it. Graphs kept by builds before that are made
again the first time the server loads the map.

*Apply on server* sends `lb overlay reload` over the server's command channel (the telemetry port + 1 on loopback,
signed with the channel's secret): the server reads the overlays of the map it is on again. Without a secret the
button is off and says why; `lb overlay reload` in the server console does the same.

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
`stats.lightmaps.mismatched` and shown in the Map tab.

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
