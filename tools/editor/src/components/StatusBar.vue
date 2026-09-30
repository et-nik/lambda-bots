<script setup lang="ts">
import { computed } from 'vue'

import { useEditor } from '../stores/editor'
import { useNav } from '../stores/nav'
import { toolInfo } from '../tools'

const store = useEditor()
const nav = useNav()

const hovered = computed(() => {
  const h = nav.hover
  if (h?.node != null) return `node ${h.node}`
  if (h?.link) return `link ${h.link[0]} → ${h.link[1]}`
  return ''
})

const tool = computed(() => {
  const t = toolInfo(nav.tool)
  return `${t.label}: ${t.hint}`
})

const hint = computed(() =>
  store.view === '3d'
    ? 'Right button or arrows: look · WASD: fly · Space/E up, Ctrl/Q down · Shift: faster · Wheel: step · F: focus'
    : 'Right button or arrows: move · Wheel: zoom · Click: select · F: focus · 1–4: views',
)

const changes = computed(() => nav.patches.length + nav.places.length)
/** What in the list wants a look: the file changed on disk, an error, changes that did nothing. */
const trouble = computed(() => {
  if (nav.conflict) return 'editor.yaml changed on disk'
  if (nav.error) return nav.error
  const failed = [...(nav.preview?.editor ?? []), ...(nav.preview?.overlay ?? [])].filter((o) => !o.ok).length
  return failed ? `${failed} did nothing` : ''
})
const changesTitle = computed(
  () => `${nav.changesShown ? 'Hide' : 'Show'} the changes (C)${trouble.value ? `: ${trouble.value}` : ''}`,
)
</script>

<template>
  <footer class="status">
    <button
      v-if="nav.draft"
      class="changes"
      :class="{ on: nav.changesShown }"
      :aria-expanded="nav.changesShown"
      :title="changesTitle"
      @click="nav.showChanges(!nav.changesShown)"
    >
      Changes <span class="count">{{ changes }}</span><span v-if="trouble" class="bad">!</span>
    </button>
    <span class="num">{{ store.camera.join(' ') }}</span>
    <span v-if="hovered" class="num">{{ hovered }}</span>
    <span class="tool">{{ tool }}</span>
    <span class="hint">{{ hint }}</span>
  </footer>
</template>

<style scoped>
.status {
  display: flex;
  align-items: center;
  gap: 24px;
  padding: 4px 12px;
  font-size: 12px;
  color: var(--muted);
  background: var(--panel);
  border-top: 1px solid var(--line);
  white-space: nowrap;
  overflow: hidden;
}
.num {
  font-variant-numeric: tabular-nums;
  min-width: 18ch;
}
.tool {
  color: var(--text);
  flex: none;
}
.hint {
  overflow: hidden;
  text-overflow: ellipsis;
}
.changes {
  flex: none;
  margin: -3px 0 -3px -8px;
  padding: 1px 8px;
  background: transparent;
  border-color: transparent;
  color: var(--text);
}
.changes:hover {
  border-color: var(--line);
}
.changes.on {
  background: var(--accent-dim);
  color: var(--text-strong);
}
.count {
  font-variant-numeric: tabular-nums;
}
.bad {
  margin-left: 4px;
  color: var(--warn);
  font-weight: 600;
}
</style>
