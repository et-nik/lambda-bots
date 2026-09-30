<script setup lang="ts">
import { useNav } from '../stores/nav'
import { TOOLS } from '../tools'
import Icon from './Icon.vue'

const nav = useNav()
</script>

<template>
  <div class="toolbar" role="toolbar" aria-label="Tools" aria-orientation="vertical">
    <button
      v-for="t in TOOLS"
      :key="t.id"
      :class="{ on: nav.tool === t.id }"
      :title="`${t.label} (${t.key}): ${t.hint}`"
      :aria-label="`${t.label} (${t.key})`"
      :aria-pressed="nav.tool === t.id"
      @click="nav.setTool(t.id)"
    >
      <Icon :name="t.id" />
      <kbd>{{ t.key }}</kbd>
    </button>
  </div>
</template>

<style scoped>
.toolbar {
  position: absolute;
  z-index: 5;
  left: 8px;
  top: 8px;
  display: grid;
  gap: 2px;
  padding: 3px;
  background: rgb(29 32 38 / 0.92);
  border: 1px solid var(--line);
  border-radius: 6px;
}
button {
  position: relative;
  display: grid;
  place-items: center;
  width: 34px;
  height: 32px;
  padding: 0;
  background: transparent;
  border: 1px solid transparent;
}
button:hover {
  background: #2a2f37;
  border-color: var(--line);
}
button.on {
  background: var(--accent-dim);
  border-color: var(--accent);
  color: var(--text-strong);
}
kbd {
  position: absolute;
  right: 2px;
  bottom: 1px;
  font: 600 8px/1 system-ui, sans-serif;
  color: var(--muted);
}
.on kbd {
  color: var(--text-strong);
}
</style>
