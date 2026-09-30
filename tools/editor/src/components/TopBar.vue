<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted } from 'vue'

import { MOD } from '../shortcuts'
import { LAYERS, useEditor } from '../stores/editor'
import { useNav } from '../stores/nav'
import type { View } from '../types'
import { viewAxes } from '../viewer/controls'
import Icon from './Icon.vue'

const store = useEditor()
const nav = useNav()

const applyTitle = computed(() => {
  if (nav.dirty) return 'Save first'
  const a = nav.info?.apply
  if (!a) return ''
  return a.available ? `Send \`lb overlay reload\`: ${a.detail}` : a.detail
})

const VIEWS: { id: View; label: string; key: string }[] = [
  { id: '3d', label: '3D', key: '1' },
  { id: 'top', label: 'Top', key: '2' },
  { id: 'front', label: 'Front', key: '3' },
  { id: 'side', label: 'Side', key: '4' },
]

/** The range the cut moves over in a 2D view: the map's extent along the axis the view looks down. */
const sliceRange = computed(() => {
  const m = store.manifest
  if (!m || store.view === '3d') {
    return null
  }
  const d = viewAxes(store.view).depth
  return { min: Math.floor(m.mins[d]), max: Math.ceil(m.maxs[d]) }
})

/** Where a cut starts: over the heads at the first spawn when looking down, else through the middle. */
function firstCut(r: { min: number; max: number }): number {
  const spawn = store.manifest?.entities.find(
    (e) => e.classname === 'info_player_deathmatch' || e.classname === 'info_player_start',
  )
  if (store.view === 'top' && spawn) {
    return Math.min(Math.round(spawn.origin[2] + 96), r.max)
  }
  return Math.round((r.min + r.max) / 2)
}

function toggleSlice(on: boolean) {
  const r = sliceRange.value
  store.slice = on && r ? firstCut(r) : null
}

function onKey(e: KeyboardEvent) {
  if (e.target instanceof HTMLInputElement || e.target instanceof HTMLSelectElement) {
    return
  }
  const v = VIEWS.find((v) => v.key === e.key)
  if (v) {
    store.view = v.id
  }
}
onMounted(() => window.addEventListener('keydown', onKey))
onBeforeUnmount(() => window.removeEventListener('keydown', onKey))
</script>

<template>
  <header class="bar">
    <strong class="title">lambdabots</strong>
    <select
      class="maps"
      :value="store.manifest?.map ?? ''"
      :disabled="store.maps.length === 0"
      @change="store.open(($event.target as HTMLSelectElement).value)"
    >
      <option v-if="!store.manifest" value="" disabled>map…</option>
      <option v-for="m in store.maps" :key="m.name" :value="m.name">{{ m.name }}</option>
    </select>

    <div class="group" role="radiogroup" aria-label="View">
      <button
        v-for="v in VIEWS"
        :key="v.id"
        :class="{ on: store.view === v.id }"
        :title="`${v.label} view (${v.key})`"
        @click="store.view = v.id"
      >
        {{ v.label }}
      </button>
    </div>

    <details class="layers">
      <summary>Layers</summary>
      <div class="menu">
        <label v-for="l in LAYERS" :key="l.id">
          <input v-model="store.layers[l.id]" type="checkbox" />
          {{ l.label }}
        </label>
      </div>
    </details>

    <label class="field" title="Lightmap brightness">
      Light
      <input v-model.number="store.brightness" type="range" min="0.5" max="3" step="0.05" />
    </label>
    <label class="field">
      <input v-model="store.wireframe" type="checkbox" />
      Wireframe
    </label>

    <template v-if="sliceRange">
      <label class="field" title="Hide what is nearer than the cut">
        <input type="checkbox" :checked="store.slice !== null" @change="toggleSlice(($event.target as HTMLInputElement).checked)" />
        Cut
      </label>
      <input
        v-if="store.slice !== null"
        v-model.number="store.slice"
        class="slice"
        type="range"
        :min="sliceRange.min"
        :max="sliceRange.max"
        step="8"
      />
      <span v-if="store.slice !== null" class="num">{{ store.slice }}</span>
    </template>

    <div v-if="nav.draft" class="doc" aria-label="Changes">
      <button class="icon" :disabled="!nav.history.length" :title="`Undo (${MOD}Z)`" aria-label="Undo" @click="nav.undo()">
        <Icon name="undo" />
      </button>
      <button class="icon" :disabled="!nav.future.length" :title="`Redo (${MOD}Shift+Z)`" aria-label="Redo" @click="nav.redo()">
        <Icon name="redo" />
      </button>
      <button
        class="save"
        :class="{ due: nav.dirty }"
        :disabled="!nav.dirty"
        :title="`Save editor.yaml and the graph with the changes, editor.lbnav (${MOD}S)`"
        @click="nav.save()"
      >
        Save<span v-if="nav.dirty" class="dot">•</span>
      </button>
      <span :title="applyTitle">
        <button :disabled="nav.dirty || !nav.info?.apply.available" @click="nav.apply()">Apply on server</button>
      </span>
    </div>
  </header>
</template>

<style scoped>
.bar {
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 6px 12px;
  background: var(--panel);
  border-bottom: 1px solid var(--line);
  flex-wrap: wrap;
}
.title {
  color: var(--accent);
  letter-spacing: 0.02em;
}
.maps {
  min-width: 180px;
}
.group {
  display: flex;
}
.group button {
  border-radius: 0;
}
.group button:first-child {
  border-radius: 4px 0 0 4px;
}
.group button:last-child {
  border-radius: 0 4px 4px 0;
}
.group button + button {
  border-left: none;
}
.on {
  background: var(--accent-dim);
  color: var(--text-strong);
}
.layers {
  position: relative;
}
.layers summary {
  cursor: pointer;
  user-select: none;
  padding: 3px 8px;
  border: 1px solid var(--line);
  border-radius: 4px;
}
.menu {
  position: absolute;
  z-index: 10;
  top: calc(100% + 4px);
  left: 0;
  display: grid;
  gap: 4px;
  padding: 8px 10px;
  min-width: 190px;
  background: var(--panel);
  border: 1px solid var(--line);
  border-radius: 4px;
  box-shadow: 0 6px 18px rgb(0 0 0 / 0.4);
}
.field {
  display: flex;
  align-items: center;
  gap: 6px;
  color: var(--muted);
}
.slice {
  width: 160px;
}
.num {
  font-variant-numeric: tabular-nums;
  color: var(--muted);
  min-width: 4ch;
}
.doc {
  display: flex;
  gap: 4px;
  align-items: center;
  margin-left: auto;
}
.doc .icon {
  display: grid;
  place-items: center;
  padding: 3px 7px;
}
.doc button:disabled {
  opacity: 0.45;
  cursor: default;
}
.save.due {
  background: var(--accent-dim);
  border-color: var(--accent);
  color: var(--text-strong);
}
.dot {
  margin-left: 3px;
  color: var(--accent);
}
</style>
