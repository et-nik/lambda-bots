<script setup lang="ts">
import type { IconName } from '../tools'

defineProps<{ name: IconName }>()

/** Strokes on a 16×16 grid; `fill` for the solid shapes. */
const ICONS: Record<IconName, { d: string; fill?: boolean }[]> = {
  select: [{ d: 'M4 2v11l3-3 2.5 5 2-1-2.5-5H13z', fill: true }],
  link: [{ d: 'M5.6 10.4l4.8-4.8M12 2.5a1.5 1.5 0 1 0 .01 0M4 10.5a1.5 1.5 0 1 0 .01 0' }],
  unlink: [{ d: 'M5.6 10.4l1.6-1.6M8.8 7.2l1.6-1.6M12 2.5a1.5 1.5 0 1 0 .01 0M4 10.5a1.5 1.5 0 1 0 .01 0M6 4l1 2M10 12l-1-2' }],
  node: [{ d: 'M7 6a3 3 0 1 0 .01 0M13 1.5v4M11 3.5h4' }],
  move: [{ d: 'M8 1.5v13M1.5 8h13M6 3.5l2-2 2 2M6 12.5l2 2 2-2M3.5 6l-2 2 2 2M12.5 6l2 2-2 2' }],
  forbid: [{ d: 'M8 2a6 6 0 1 0 .01 0M3.8 3.8l8.4 8.4' }],
  place: [{ d: 'M8 14.5S3.5 9.8 3.5 6.5a4.5 4.5 0 0 1 9 0c0 3.3-4.5 8-4.5 8zM8 5a1.5 1.5 0 1 0 .01 0' }],
  route: [{ d: 'M3 13.5c4 0 2-5.5 5-5.5s1-5.5 5-5.5M3 12a1.5 1.5 0 1 0 .01 0M13 1a1.5 1.5 0 1 0 .01 0' }],
  undo: [{ d: 'M5.5 3L2.5 6l3 3M2.5 6H10a3.5 3.5 0 0 1 0 7H6' }],
  redo: [{ d: 'M10.5 3l3 3-3 3M13.5 6H6a3.5 3.5 0 0 0 0 7h4' }],
}
</script>

<template>
  <svg viewBox="0 0 16 16" width="16" height="16" aria-hidden="true">
    <path
      v-for="(p, i) in ICONS[name]"
      :key="i"
      :d="p.d"
      :fill="p.fill ? 'currentColor' : 'none'"
      stroke="currentColor"
      stroke-width="1.4"
      stroke-linecap="round"
      stroke-linejoin="round"
    />
  </svg>
</template>
