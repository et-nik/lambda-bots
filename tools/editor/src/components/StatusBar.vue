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
</script>

<template>
  <footer class="status">
    <span class="num">{{ store.camera.join(' ') }}</span>
    <span v-if="hovered" class="num">{{ hovered }}</span>
    <span class="tool">{{ tool }}</span>
    <span class="hint">{{ hint }}</span>
  </footer>
</template>

<style scoped>
.status {
  display: flex;
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
</style>
