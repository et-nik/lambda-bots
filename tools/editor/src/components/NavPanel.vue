<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted } from 'vue'

import { LINK_KIND_CHOICES, useNav } from '../stores/nav'
import type { Outcome, PatchOp, Tool, Vec3 } from '../types'
import { KIND_COLORS, type Link, linkValid, LINK, nodeFlagNames, OFF_COLOR } from '../viewer/graph'

const nav = useNav()

const TOOLS: { id: Tool; label: string; key: string; code: string; hint: string }[] = [
  { id: 'select', label: 'Select', key: 'V', code: 'KeyV', hint: 'Click a node, a link or an entity.' },
  {
    id: 'link',
    label: 'Link',
    key: 'L',
    code: 'KeyL',
    hint: 'Click the node a link starts at, then the one it goes to; the next link starts there. Esc stops.',
  },
  { id: 'unlink', label: 'Unlink', key: 'U', code: 'KeyU', hint: 'Click a link to take it out.' },
  {
    id: 'node',
    label: 'Node',
    key: 'N',
    code: 'KeyN',
    hint: 'Click the floor where a node is missing: it is linked with the nodes around that check out.',
  },
  {
    id: 'move',
    label: 'Move',
    key: 'M',
    code: 'KeyM',
    hint: 'Drag a node to where it should stand: its links are checked again there. Esc while dragging leaves it.',
  },
  { id: 'forbid', label: 'Forbid', key: 'X', code: 'KeyX', hint: 'Click where bots must never plan through.' },
  { id: 'place', label: 'Place', key: 'P', code: 'KeyP', hint: 'Click where the named place is.' },
  {
    id: 'route',
    label: 'Route',
    key: 'R',
    code: 'KeyR',
    hint: 'Click where a bot starts, then where it goes: the way it plans with the changes.',
  },
]

const tool = computed(() => TOOLS.find((t) => t.id === nav.tool)!)
const model = computed(() => nav.model)

/** Link kinds in the graph with their counts, for the legend; and how many links are off. */
const legend = computed(() => {
  const counts = new Map<string, number>()
  let off = 0
  for (const l of model.value?.links ?? []) {
    counts.set(l.kind, (counts.get(l.kind) ?? 0) + 1)
    if (!linkValid(l)) off++
  }
  const kinds = (nav.info?.kinds ?? [])
    .filter((k) => counts.has(k))
    .map((k) => ({ kind: k, count: counts.get(k)!, color: `#${(KIND_COLORS[k] ?? 0xffffff).toString(16).padStart(6, '0')}` }))
  return { kinds, off }
})
const offColor = `#${OFF_COLOR.toString(16).padStart(6, '0')}`

function describe(p: PatchOp): string {
  switch (p.op) {
    case 'forbid':
      return `Forbid, ${p.radius} u`
    case 'add_link':
      return `Link${p.kind ? `: ${p.kind.replace('_', ' ')}` : ''}${p.both ? ', both ways' : ''}${p.trust ? ', trusted' : ''}`
    case 'remove_link':
      return `Unlink${p.both ? ', both ways' : ''}`
    case 'add_node':
      return 'Node'
    case 'move_node':
      return `Move, ${Math.round(Math.hypot(p.to[0] - p.from[0], p.to[1] - p.from[1], p.to[2] - p.from[2]))} u`
  }
}

function spot(p: Vec3): string {
  return p.map((v) => Math.round(v)).join(' ')
}

function outcome(i: number): Outcome | null {
  return nav.preview?.editor[i] ?? null
}

function fmtLink(l: Link): string {
  const bits = [l.kind.replace('_', ' '), `${l.cost.toFixed(2)} s`]
  if (!linkValid(l)) bits.push('off')
  if (l.flags & LINK.TRUSTED) bits.push('trusted')
  if (l.added) bits.push('changed')
  return bits.join(' · ')
}

const selectedLink = computed(() => {
  const m = model.value
  const s = nav.selLink
  return m && s ? (m.find(s[0], s[1]) ?? null) : null
})

const routeKinds = computed(() => {
  const counts = new Map<string, number>()
  for (const l of nav.route?.legs ?? []) counts.set(l.kind, (counts.get(l.kind) ?? 0) + 1)
  return [...counts].map(([k, n]) => `${k.replace('_', ' ')} ${n}`).join(', ')
})

function pickPatch(i: number) {
  nav.selPatch = nav.selPatch === i ? null : i
  nav.selNode = null
  nav.selLink = null
  if (nav.selPatch !== null) nav.focusRequest++
}

function editPatch(i: number, change: Partial<PatchOp>) {
  nav.updatePatch(i, { ...nav.patches[i], ...change } as PatchOp)
}

function linkFrom(n: number) {
  nav.setTool('link')
  nav.pending = n
}

/** Ctrl lowers the camera, so shortcuts are ⌘ on a Mac; elsewhere Ctrl pressed just before the key. */
const MAC = navigator.userAgent.includes('Mac')
const MOD = MAC ? '⌘' : 'Ctrl+'
/** Ctrl held longer than this before the key is flying down, not a shortcut (ms). */
const CTRL_SHORTCUT = 600
let ctrlAt = 0

function shortcut(e: KeyboardEvent): boolean {
  return MAC ? e.metaKey : e.ctrlKey && performance.now() - ctrlAt < CTRL_SHORTCUT
}

function onKey(e: KeyboardEvent) {
  if ((e.code === 'ControlLeft' || e.code === 'ControlRight') && !e.repeat) {
    ctrlAt = performance.now()
  }
  const t = e.target
  if (t instanceof HTMLInputElement || t instanceof HTMLSelectElement || t instanceof HTMLTextAreaElement) {
    return
  }
  const mod = shortcut(e)
  if (mod && e.code === 'KeyZ') {
    e.preventDefault()
    if (e.shiftKey) nav.redo()
    else nav.undo()
    return
  }
  if (mod && e.code === 'KeyY') {
    e.preventDefault()
    nav.redo()
    return
  }
  if (mod && e.code === 'KeyS') {
    e.preventDefault()
    void nav.save()
    return
  }
  if (e.metaKey || e.ctrlKey || e.altKey) return
  if (e.code === 'Escape') {
    nav.cancel()
    nav.clearSelection()
    return
  }
  if (e.code === 'Delete' || e.code === 'Backspace') {
    if (nav.selPatch !== null) nav.removePatch(nav.selPatch)
    else if (nav.selLink) nav.unlink(nav.selLink)
    return
  }
  const t2 = TOOLS.find((x) => x.code === e.code)
  if (t2) nav.setTool(t2.id)
}

onMounted(() => window.addEventListener('keydown', onKey))
onBeforeUnmount(() => window.removeEventListener('keydown', onKey))
</script>

<template>
  <div class="nav">
    <header class="head">
      <label class="show"><input v-model="nav.show" type="checkbox" /> Graph</label>
      <span v-if="nav.loading" class="muted">Loading the graph… (making one takes a while on a big map)</span>
      <span v-else-if="model" class="muted">
        {{ model.nodes }} nodes, {{ model.links.length }} links ·
        <span v-if="nav.info?.origin === 'server'" title="The graph the server made and plays on">the server's graph</span>
        <span v-else title="The server has not played this build of the map: the graph is made here, with the default physics">made here</span>
      </span>
    </header>
    <p v-if="nav.error" class="error">{{ nav.error }}</p>
    <div v-if="legend.kinds.length" class="legend" aria-label="Link kinds: click to hide or show">
      <button
        v-for="k in legend.kinds"
        :key="k.kind"
        :class="{ hidden: nav.hiddenKinds.includes(k.kind) }"
        :aria-pressed="!nav.hiddenKinds.includes(k.kind)"
        :title="`${k.kind.replace('_', ' ')}: ${k.count} links; click to ${nav.hiddenKinds.includes(k.kind) ? 'show' : 'hide'}`"
        @click="nav.toggleKind(k.kind)"
      >
        <i :style="{ background: k.color }" />{{ k.kind.replace('_', ' ') }} {{ k.count }}
      </button>
      <button
        v-if="legend.off"
        :class="{ hidden: nav.hideOff }"
        :aria-pressed="!nav.hideOff"
        title="Links that are off: the check failed, or the live server disagreed"
        @click="nav.hideOff = !nav.hideOff"
      >
        <i :style="{ background: offColor }" />off {{ legend.off }}
      </button>
    </div>

    <div class="tools" role="toolbar" aria-label="Tools">
      <button
        v-for="t in TOOLS"
        :key="t.id"
        :class="{ on: nav.tool === t.id }"
        :title="`${t.hint} (${t.key})`"
        :aria-label="`${t.label} (${t.key})`"
        :aria-pressed="nav.tool === t.id"
        @click="nav.setTool(t.id)"
      >
        {{ t.label }}
      </button>
    </div>
    <p class="hint">{{ tool.hint }}</p>

    <div v-if="nav.tool === 'link'" class="options">
      <label>
        Kind
        <select v-model="nav.linkKind">
          <option v-for="k in LINK_KIND_CHOICES" :key="k.id" :value="k.id">{{ k.label }}</option>
        </select>
      </label>
      <label><input v-model="nav.linkBoth" type="checkbox" /> both ways</label>
      <label title="Put the link in even if the check fails"><input v-model="nav.linkTrust" type="checkbox" /> trust</label>
      <span v-if="nav.pending !== null" class="pending">from node {{ nav.pending }}</span>
    </div>
    <div v-else-if="nav.tool === 'unlink'" class="options">
      <label><input v-model="nav.unlinkBoth" type="checkbox" /> both ways</label>
    </div>
    <div v-else-if="nav.tool === 'forbid'" class="options">
      <label>Radius <input v-model.number="nav.forbidRadius" type="number" min="8" max="2048" step="8" /></label>
    </div>
    <div v-else-if="nav.tool === 'place'" class="options">
      <label>Name <input v-model="nav.placeName" type="text" size="10" /></label>
      <label>Radius <input v-model.number="nav.placeRadius" type="number" min="16" max="4096" step="16" /></label>
      <label>Tags <input v-model="nav.placeTags" type="text" size="12" placeholder="shelter, sniper" /></label>
    </div>
    <div v-else-if="nav.tool === 'route'" class="options">
      <label><input v-model="nav.routeLongjump" type="checkbox" /> long jump</label>
      <label><input v-model="nav.routeGauss" type="checkbox" /> gauss</label>
      <span v-if="nav.routeFrom" class="pending">now where to</span>
    </div>

    <div class="actions">
      <button :disabled="!nav.history.length" :title="`Undo (${MOD}Z)`" @click="nav.undo()">Undo</button>
      <button :disabled="!nav.future.length" :title="`Redo (${MOD}Shift+Z)`" @click="nav.redo()">Redo</button>
      <button class="primary" :disabled="!nav.dirty || !nav.draft" :title="`Save editor.yaml and the graph with the changes, editor.lbnav (${MOD}S)`" @click="nav.save()">
        Save
      </button>
      <button
        :disabled="nav.dirty || !nav.info?.apply.available"
        :title="nav.dirty ? 'Save first' : nav.info?.apply.detail"
        @click="nav.apply()"
      >
        Apply on server
      </button>
      <span v-if="nav.dirty" class="dirty">unsaved</span>
    </div>
    <p v-if="nav.info && !nav.info.apply.available" class="muted small">{{ nav.info.apply.detail }}</p>
    <p v-if="nav.notice" class="notice">{{ nav.notice }}</p>

    <div v-if="nav.conflict" class="conflict">
      <p>editor.yaml changed on disk since it was read (saved in the game?).</p>
      <button @click="nav.save(true)">Keep mine</button>
      <button @click="nav.takeTheirs()">Take the file</button>
    </div>

    <section v-if="model && nav.selNode !== null && nav.selNode < model.nodes" class="card">
      <h3>
        Node {{ nav.selNode }}
        <span class="muted">{{ nodeFlagNames(model.flags[nav.selNode]).join(', ') }}</span>
      </h3>
      <p class="muted small">
        {{ spot(model.origin(nav.selNode)) }}
        <template v-if="model.movedFrom.has(nav.selNode)">
          · moved from {{ spot(model.baseOrigin(nav.selNode)) }}
        </template>
      </p>
      <table class="links">
        <tbody>
          <tr v-for="l in model.linksOut(nav.selNode)" :key="`o${l.to}`" :class="{ off: !linkValid(l) }">
            <th>→ {{ l.to }}</th>
            <td>{{ fmtLink(l) }}</td>
          </tr>
          <tr v-for="l in model.linksIn(nav.selNode)" :key="`i${l.from}`" :class="{ off: !linkValid(l) }">
            <th>← {{ l.from }}</th>
            <td>{{ fmtLink(l) }}</td>
          </tr>
        </tbody>
      </table>
      <button @click="linkFrom(nav.selNode)">Link from here</button>
    </section>

    <section v-if="nav.selLink" class="card">
      <h3>Link {{ nav.selLink[0] }} → {{ nav.selLink[1] }}</h3>
      <p v-if="selectedLink" class="muted small">{{ fmtLink(selectedLink) }}</p>
      <button @click="nav.unlink(nav.selLink!)">Unlink{{ nav.unlinkBoth ? ' (both ways)' : '' }}</button>
      <button @click="nav.selectNode(nav.selLink![0])">Node {{ nav.selLink[0] }}</button>
      <button @click="nav.selectNode(nav.selLink![1])">Node {{ nav.selLink[1] }}</button>
    </section>

    <section v-if="nav.route" class="card">
      <h3>Route {{ nav.route.start }} → {{ nav.route.goal }}</h3>
      <p>
        {{ nav.route.time.toFixed(1) }} s over {{ nav.route.legs.length }} links
        <span class="muted">({{ routeKinds }})</span>
      </p>
      <p v-if="nav.route.plain !== null" class="muted small">without tricks {{ nav.route.plain.toFixed(1) }} s</p>
      <button @click="nav.route = null">Clear</button>
    </section>

    <h3 class="section">Changes <span class="muted">editor.yaml</span></h3>
    <p v-if="!nav.patches.length && !nav.places.length" class="muted small">None yet.</p>
    <ul class="patches">
      <li
        v-for="(p, i) in nav.patches"
        :key="i"
        :class="{ sel: nav.selPatch === i, bad: outcome(i) && !outcome(i)!.ok }"
        @click="pickPatch(i)"
      >
        <div class="row">
          <span class="what">{{ i + 1 }}. {{ describe(p) }}</span>
          <button class="x" title="Remove (Delete)" @click.stop="nav.removePatch(i)">×</button>
        </div>
        <div class="result">{{ outcome(i)?.message ?? '…' }}</div>
        <div v-if="nav.selPatch === i" class="edit" @click.stop>
          <label v-if="p.op === 'forbid'">
            Radius
            <input
              type="number"
              min="8"
              step="8"
              :value="p.radius"
              @change="editPatch(i, { radius: Number(($event.target as HTMLInputElement).value) })"
            />
          </label>
          <template v-if="p.op === 'add_link'">
            <select
              :value="p.kind ?? 'auto'"
              @change="
                editPatch(i, {
                  kind: (($event.target as HTMLSelectElement).value === 'auto'
                    ? undefined
                    : ($event.target as HTMLSelectElement).value) as string | undefined,
                })
              "
            >
              <option v-for="k in LINK_KIND_CHOICES" :key="k.id" :value="k.id">{{ k.label }}</option>
            </select>
            <label><input type="checkbox" :checked="p.both" @change="editPatch(i, { both: !p.both })" /> both</label>
            <label><input type="checkbox" :checked="p.trust" @change="editPatch(i, { trust: !p.trust })" /> trust</label>
          </template>
          <label v-if="p.op === 'remove_link'">
            <input type="checkbox" :checked="p.both" @change="editPatch(i, { both: !p.both })" /> both ways
          </label>
          <label class="note">
            Note
            <input
              type="text"
              :value="p.note ?? ''"
              @change="editPatch(i, { note: ($event.target as HTMLInputElement).value || undefined })"
            />
          </label>
        </div>
      </li>
      <li v-for="(pl, i) in nav.places" :key="`place${i}`">
        <div class="row">
          <span class="what">Place {{ pl.name }}, {{ pl.radius }} u{{ pl.tags?.length ? ` · ${pl.tags.join(', ')}` : '' }}</span>
          <button class="x" title="Remove" @click.stop="nav.removePlace(i)">×</button>
        </div>
      </li>
    </ul>

    <template v-if="nav.info?.overlay.file?.nav?.patches?.length || nav.info?.overlay.file?.places?.length">
      <h3 class="section">By hand <span class="muted">overlay.yaml, applied last</span></h3>
      <ul class="patches fixed">
        <li
          v-for="(p, i) in nav.info!.overlay.file!.nav?.patches ?? []"
          :key="`o${i}`"
          :class="{ bad: nav.preview?.overlay[i] && !nav.preview.overlay[i].ok }"
        >
          <div class="what">{{ describe(p) }}</div>
          <div class="result">{{ nav.preview?.overlay[i]?.message ?? '…' }}</div>
        </li>
        <li v-for="pl in nav.info!.overlay.file!.places ?? []" :key="`op${pl.name}`">
          <div class="what">Place {{ pl.name }}, {{ pl.radius }} u</div>
        </li>
      </ul>
    </template>
    <p v-if="nav.info?.overlay.error" class="error small">overlay.yaml: {{ nav.info.overlay.error }}</p>
  </div>
</template>

<style scoped>
.nav {
  display: grid;
  gap: 8px;
  align-content: start;
}
.head {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
  align-items: baseline;
}
.show {
  display: flex;
  gap: 6px;
  align-items: center;
  font-weight: 600;
  color: var(--text-strong);
}
.legend {
  display: flex;
  flex-wrap: wrap;
  gap: 3px;
}
.legend button {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  padding: 1px 7px;
  font-size: 11px;
  border-radius: 10px;
}
.legend button.hidden {
  opacity: 0.4;
  text-decoration: line-through;
}
.legend i {
  width: 8px;
  height: 8px;
  border-radius: 50%;
}
.tools {
  display: flex;
  flex-wrap: wrap;
  gap: 4px;
}
.tools button {
  padding: 3px 8px;
}
.on {
  background: var(--accent-dim);
  color: var(--text-strong);
}
.hint {
  margin: 0;
  color: var(--muted);
  font-size: 12px;
}
.options {
  display: flex;
  flex-wrap: wrap;
  gap: 6px 12px;
  align-items: center;
  padding: 6px 8px;
  background: #22262d;
  border-radius: 4px;
}
.options label {
  display: flex;
  gap: 4px;
  align-items: center;
}
.options input[type='number'] {
  width: 64px;
}
input[type='text'],
input[type='number'] {
  padding: 2px 6px;
  background: #1a1d22;
  border: 1px solid var(--line);
  border-radius: 3px;
}
.pending {
  color: #ff4df0;
}
.actions {
  display: flex;
  flex-wrap: wrap;
  gap: 4px;
  align-items: center;
}
.primary:not(:disabled) {
  background: var(--accent-dim);
  color: var(--text-strong);
}
button:disabled {
  opacity: 0.45;
  cursor: default;
}
.dirty {
  color: var(--accent);
  font-size: 12px;
}
.notice {
  margin: 0;
  padding: 6px 8px;
  background: #1f2a33;
  border-radius: 4px;
  font-size: 12px;
}
.error {
  margin: 0;
  color: var(--warn);
}
.conflict {
  padding: 8px;
  border: 1px solid var(--warn);
  border-radius: 4px;
}
.conflict p {
  margin: 0 0 6px;
}
.card {
  padding: 8px;
  background: #22262d;
  border-radius: 4px;
}
.card h3 {
  margin: 0 0 4px;
  font-size: 13px;
  color: var(--text-strong);
}
.card p {
  margin: 0 0 6px;
}
.card button {
  margin: 4px 4px 0 0;
}
.links {
  width: 100%;
  font-size: 12px;
  border-collapse: collapse;
}
.links th {
  text-align: left;
  font-weight: normal;
  color: var(--muted);
  padding: 1px 6px 1px 0;
  white-space: nowrap;
}
.links td {
  padding: 1px 0;
}
.links .off td {
  color: var(--warn);
}
.section {
  margin: 8px 0 0;
  font-size: 12px;
  text-transform: uppercase;
  letter-spacing: 0.05em;
  color: var(--muted);
}
.section .muted {
  text-transform: none;
  letter-spacing: 0;
}
.patches {
  list-style: none;
  margin: 0;
  padding: 0;
  display: grid;
  gap: 4px;
}
.patches li {
  padding: 5px 8px;
  background: #22262d;
  border-left: 3px solid #3ddc84;
  border-radius: 3px;
  cursor: pointer;
}
.patches li.bad {
  border-left-color: var(--warn);
}
.patches li.sel {
  outline: 1px solid var(--accent);
}
.patches.fixed li {
  cursor: default;
  opacity: 0.75;
}
.row {
  display: flex;
  justify-content: space-between;
  gap: 8px;
}
.what {
  color: var(--text-strong);
}
.result {
  font-size: 12px;
  color: var(--muted);
  word-break: break-word;
}
.bad .result {
  color: var(--warn);
}
.x {
  padding: 0 6px;
  line-height: 1.2;
}
.edit {
  display: flex;
  flex-wrap: wrap;
  gap: 6px 10px;
  align-items: center;
  margin-top: 6px;
  cursor: default;
}
.edit label {
  display: flex;
  gap: 4px;
  align-items: center;
}
.edit input[type='number'] {
  width: 64px;
}
.note input {
  width: 150px;
}
.muted {
  color: var(--muted);
}
.small {
  font-size: 12px;
  margin: 0;
}
</style>
