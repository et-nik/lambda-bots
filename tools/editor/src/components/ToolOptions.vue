<script setup lang="ts">
import { computed } from 'vue'

import { LINK_KIND_CHOICES, SNAP_STEPS, useNav } from '../stores/nav'
import { toolInfo } from '../tools'

const nav = useNav()

const WITH_OPTIONS = new Set(['link', 'unlink', 'node', 'move', 'forbid', 'place', 'route'])
const shown = computed(() => WITH_OPTIONS.has(nav.tool))
</script>

<template>
  <div v-if="shown" class="options" role="group" :aria-label="`${toolInfo(nav.tool).label} options`">
    <strong>{{ toolInfo(nav.tool).label }}</strong>
    <template v-if="nav.tool === 'link'">
      <label>
        kind
        <select v-model="nav.linkKind">
          <option v-for="k in LINK_KIND_CHOICES" :key="k.id" :value="k.id">{{ k.label }}</option>
        </select>
      </label>
      <label><input v-model="nav.linkBoth" type="checkbox" /> both ways</label>
      <label title="Put the link in even if the check fails"><input v-model="nav.linkTrust" type="checkbox" /> trust</label>
      <span v-if="nav.pending !== null" class="state">from node {{ nav.pending }} · Esc stops</span>
    </template>
    <label v-else-if="nav.tool === 'unlink'"><input v-model="nav.unlinkBoth" type="checkbox" /> both ways</label>
    <label
      v-else-if="nav.tool === 'node'"
      title="Link the node with the nodes around wherever the links check out; off: it goes in without links, for Link (L)"
    >
      <input v-model="nav.nodeLink" type="checkbox" /> auto-link
    </label>
    <template v-else-if="nav.tool === 'move'">
      <label title="Moves snap to the grid; [ and ] change its step">
        grid
        <select v-model.number="nav.snapStep">
          <option v-for="s in SNAP_STEPS" :key="s" :value="s">{{ s }}</option>
        </select>
      </label>
      <span class="muted">[ ] step · Alt: off the grid · arrows of the gizmo: one axis, squares: a plane</span>
    </template>
    <label v-else-if="nav.tool === 'forbid'">
      radius <input v-model.number="nav.forbidRadius" type="number" min="8" max="2048" step="8" />
    </label>
    <template v-else-if="nav.tool === 'place'">
      <label>name <input v-model="nav.placeName" type="text" size="9" /></label>
      <label>radius <input v-model.number="nav.placeRadius" type="number" min="16" max="4096" step="16" /></label>
      <label>tags <input v-model="nav.placeTags" type="text" size="12" placeholder="shelter, sniper" /></label>
    </template>
    <template v-else-if="nav.tool === 'route'">
      <label><input v-model="nav.routeLongjump" type="checkbox" /> long jump</label>
      <label><input v-model="nav.routeGauss" type="checkbox" /> gauss</label>
      <span v-if="nav.routeFrom" class="state">now where to · Esc stops</span>
    </template>
  </div>
</template>

<style scoped>
.options {
  position: absolute;
  z-index: 5;
  left: 56px;
  top: 8px;
  right: 8px;
  width: max-content;
  max-width: calc(100% - 64px);
  display: flex;
  flex-wrap: wrap;
  gap: 6px 12px;
  align-items: center;
  padding: 5px 10px;
  background: rgb(29 32 38 / 0.92);
  border: 1px solid var(--line);
  border-radius: 6px;
  font-size: 12px;
}
strong {
  color: var(--text-strong);
}
label {
  display: flex;
  gap: 4px;
  align-items: center;
  color: var(--muted);
}
input[type='number'] {
  width: 64px;
}
input[type='text'],
input[type='number'] {
  padding: 2px 6px;
  background: #1a1d22;
  border: 1px solid var(--line);
  border-radius: 3px;
  color: var(--text);
}
select {
  padding: 1px 6px;
}
.state {
  color: #ff4df0;
}
</style>
