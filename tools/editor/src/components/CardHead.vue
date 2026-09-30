<script setup lang="ts">
defineProps<{
  title: string
  /** The selection ‹ goes back to, when there is one. */
  back?: string | null
  /** Nothing in the view to bring close. */
  noFocus?: boolean
}>()
defineEmits<{ back: []; focus: []; close: [] }>()
</script>

<template>
  <header class="head">
    <button v-if="back" class="icon back" :title="`Back to ${back}`" @click="$emit('back')">‹ {{ back }}</button>
    <h3>{{ title }} <slot /></h3>
    <button v-if="!noFocus" class="icon" title="Bring it into view (F)" aria-label="Bring it into view" @click="$emit('focus')">
      ⌖
    </button>
    <button class="icon" title="Close (Esc)" aria-label="Close" @click="$emit('close')">×</button>
  </header>
</template>

<style scoped>
.head {
  display: flex;
  gap: 6px;
  align-items: center;
  margin-bottom: 4px;
}
h3 {
  flex: 1;
  margin: 0;
  font-size: 14px;
  color: var(--text-strong);
  min-width: 0;
  overflow-wrap: anywhere;
}
.icon {
  padding: 0 7px;
  line-height: 20px;
  background: transparent;
  border-color: transparent;
  color: var(--muted);
}
.icon:hover {
  color: var(--text-strong);
  border-color: var(--line);
}
.back {
  font-size: 12px;
  font-variant-numeric: tabular-nums;
}
</style>
