<script setup lang="ts">
import { computed } from 'vue'

import { describe } from '../format'
import { useNav } from '../stores/nav'
import type { Outcome } from '../types'

const nav = useNav()

const overlay = computed(() => nav.info?.overlay.file ?? null)
const byHand = computed(() => (overlay.value?.nav?.patches?.length ?? 0) + (overlay.value?.places?.length ?? 0))

function outcome(i: number): Outcome | null {
  return nav.preview?.editor[i] ?? null
}

/** What a check came to, without the node and place it names (the row names them). */
function short(o: Outcome | null): string {
  if (!o) return '…'
  const i = o.message.lastIndexOf(': ')
  return i >= 0 && o.ok ? o.message.slice(i + 2) : o.message
}

function peek(o: Outcome | null) {
  nav.peek = o ? { nodes: o.nodes, links: o.links } : null
}
</script>

<template>
  <section class="changes">
    <header class="head">
      <h3>Changes</h3>
      <span class="muted">editor.yaml · {{ nav.patches.length + nav.places.length }}</span>
      <span v-if="nav.dirty" class="unsaved">unsaved</span>
    </header>
    <p v-if="nav.loading" class="muted small">Loading the graph… (making one takes a while on a big map)</p>
    <p v-if="nav.error" class="error small">{{ nav.error }}</p>
    <div v-if="nav.conflict" class="conflict">
      <p>editor.yaml changed on disk since it was read (saved in the game?).</p>
      <button @click="nav.save(true)">Keep mine</button>
      <button @click="nav.takeTheirs()">Take the file</button>
    </div>
    <p v-if="!nav.patches.length && !nav.places.length && !nav.loading" class="muted small empty">
      None yet. The tools are on the left of the view: Link (L) and Unlink (U) change links, Node (N) and Move (M)
      put nodes in and move them, Forbid (X) shuts an area off, Route (R) shows how a bot goes.
    </p>

    <ol class="list" @mouseleave="nav.peek = null">
      <li
        v-for="(p, i) in nav.patches"
        :key="i"
        :class="{ sel: nav.selPatch === i, bad: outcome(i) && !outcome(i)!.ok }"
        :title="outcome(i)?.message"
        @mouseenter="peek(outcome(i))"
        @click="nav.goPatch(i)"
      >
        <span class="mark" :aria-label="outcome(i)?.ok === false ? 'did nothing' : 'applied'">
          {{ outcome(i)?.ok === false ? '!' : '✓' }}
        </span>
        <span class="no">{{ i + 1 }}</span>
        <span class="what">{{ describe(p, outcome(i)) }}</span>
        <span class="res">{{ short(outcome(i)) }}</span>
        <button class="x" title="Remove (Delete)" :aria-label="`Remove change ${i + 1}`" @click.stop="nav.removePatch(i)">
          ×
        </button>
      </li>
      <li v-for="(pl, i) in nav.places" :key="`place${i}`" class="place">
        <span class="mark">◎</span>
        <span class="what">{{ pl.name }}</span>
        <span class="res">{{ pl.radius }} u{{ pl.tags?.length ? ` · ${pl.tags.join(', ')}` : '' }}</span>
        <button class="x" :aria-label="`Remove place ${pl.name}`" @click.stop="nav.removePlace(i)">×</button>
      </li>
    </ol>

    <details v-if="byHand" class="hand">
      <summary>
        By hand <span class="muted">overlay.yaml · {{ byHand }}, applied after these</span>
      </summary>
      <ol class="list fixed" @mouseleave="nav.peek = null">
        <li
          v-for="(p, i) in overlay?.nav?.patches ?? []"
          :key="`o${i}`"
          :class="{ bad: nav.preview?.overlay[i] && !nav.preview.overlay[i].ok }"
          :title="nav.preview?.overlay[i]?.message"
          @mouseenter="peek(nav.preview?.overlay[i] ?? null)"
        >
          <span class="mark">{{ nav.preview?.overlay[i]?.ok === false ? '!' : '✓' }}</span>
          <span class="what">{{ describe(p, nav.preview?.overlay[i]) }}</span>
          <span class="res">{{ short(nav.preview?.overlay[i] ?? null) }}</span>
        </li>
        <li v-for="pl in overlay?.places ?? []" :key="`op${pl.name}`" class="place">
          <span class="mark">◎</span>
          <span class="what">{{ pl.name }}</span>
          <span class="res">{{ pl.radius }} u</span>
        </li>
      </ol>
    </details>
    <p v-if="nav.info?.overlay.error" class="error small">overlay.yaml: {{ nav.info.overlay.error }}</p>
  </section>
</template>

<style scoped>
.head {
  display: flex;
  gap: 8px;
  align-items: baseline;
  margin-bottom: 6px;
}
h3 {
  margin: 0;
  font-size: 12px;
  text-transform: uppercase;
  letter-spacing: 0.05em;
  color: var(--muted);
}
.head .muted {
  font-size: 12px;
}
.unsaved {
  margin-left: auto;
  color: var(--accent);
  font-size: 12px;
}
.small {
  margin: 0 0 6px;
  font-size: 12px;
}
.empty {
  line-height: 1.5;
}
.error {
  color: var(--warn);
}
.conflict {
  margin-bottom: 8px;
  padding: 8px;
  border: 1px solid var(--warn);
  border-radius: 4px;
}
.conflict p {
  margin: 0 0 6px;
}
.list {
  list-style: none;
  margin: 0;
  padding: 0;
}
.list li {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 3px 4px 3px 6px;
  border-left: 2px solid #3ddc84;
  border-radius: 2px;
  font-size: 12px;
  cursor: pointer;
}
.list li + li {
  margin-top: 1px;
}
.list li:hover {
  background: #262a31;
}
.list li.sel {
  background: #2c2518;
  box-shadow: inset 0 0 0 1px var(--accent-dim);
}
.list li.bad {
  border-left-color: var(--warn);
}
.list li.place {
  border-left-color: #5aa0ff;
  cursor: default;
}
.list.fixed li {
  cursor: default;
  opacity: 0.8;
}
.mark {
  flex: none;
  width: 1em;
  color: #3ddc84;
  text-align: center;
}
.bad .mark {
  color: var(--warn);
}
.place .mark {
  color: #5aa0ff;
}
.no {
  flex: none;
  min-width: 2ch;
  color: var(--muted);
  font-variant-numeric: tabular-nums;
  text-align: right;
}
.what {
  flex: none;
  color: var(--text-strong);
  white-space: nowrap;
}
.res {
  flex: 1;
  min-width: 0;
  color: var(--muted);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.bad .res {
  color: var(--warn);
}
.x {
  flex: none;
  padding: 0 6px;
  line-height: 18px;
  background: transparent;
  border-color: transparent;
  color: var(--muted);
  visibility: hidden;
}
.list li:hover .x,
.x:focus-visible {
  visibility: visible;
}
.x:hover {
  color: var(--warn);
  border-color: var(--line);
}
.hand {
  margin-top: 12px;
}
.hand summary {
  cursor: pointer;
  font-size: 12px;
  text-transform: uppercase;
  letter-spacing: 0.05em;
  color: var(--muted);
}
.hand summary .muted {
  text-transform: none;
  letter-spacing: 0;
}
.hand .list {
  margin-top: 6px;
}
</style>
