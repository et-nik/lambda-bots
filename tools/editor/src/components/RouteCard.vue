<script setup lang="ts">
import { computed } from 'vue'

import { kindName } from '../format'
import { useNav } from '../stores/nav'
import CardHead from './CardHead.vue'

const nav = useNav()

const r = computed(() => nav.route!)
const kinds = computed(() => {
  const counts = new Map<string, number>()
  for (const l of r.value.legs) counts.set(l.kind, (counts.get(l.kind) ?? 0) + 1)
  return [...counts].map(([k, n]) => `${kindName(k)} ${n}`).join(', ')
})
</script>

<template>
  <section>
    <CardHead :title="`Route ${r.start} → ${r.goal}`" no-focus @close="nav.route = null" />
    <p class="what">
      {{ r.time.toFixed(1) }} s over {{ r.legs.length }} links <span class="muted">({{ kinds }})</span>
    </p>
    <p v-if="r.plain !== null" class="muted small">without tricks {{ r.plain.toFixed(1) }} s</p>
    <p class="small">
      <button class="linkish" @click="nav.goNode(r.start)">node {{ r.start }}</button>
      <span class="muted"> → </span>
      <button class="linkish" @click="nav.goNode(r.goal)">node {{ r.goal }}</button>
    </p>
  </section>
</template>

<style scoped>
p {
  margin: 0 0 4px;
}
.small {
  font-size: 12px;
}
</style>
