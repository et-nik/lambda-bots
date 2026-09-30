<script setup lang="ts">
import { computed } from 'vue'

import { spot } from '../format'
import { useEditor } from '../stores/editor'
import CardHead from './CardHead.vue'

const store = useEditor()

const e = computed(() => store.entity!)
const model = computed(() => {
  const m = e.value.model
  return m == null ? null : (store.manifest?.models.find((x) => x.index === m) ?? null)
})
</script>

<template>
  <section>
    <CardHead :title="e.classname" no-focus @close="store.select(null)" />
    <p v-if="e.targetname" class="sub">{{ e.targetname }}</p>
    <dl>
      <dt>Entity</dt>
      <dd>#{{ e.index }}</dd>
      <template v-if="model">
        <dt>Model</dt>
        <dd>*{{ model.index }}, layer {{ model.layer }}</dd>
        <dt>Bounds</dt>
        <dd>{{ spot(model.mins) }} → {{ spot(model.maxs) }}</dd>
      </template>
      <template v-else>
        <dt>Origin</dt>
        <dd>{{ spot(e.origin) }}</dd>
      </template>
    </dl>
    <table class="kv">
      <tbody>
        <tr v-for="([k, v], i) in e.kv" :key="i">
          <th>{{ k }}</th>
          <td>{{ v }}</td>
        </tr>
      </tbody>
    </table>
  </section>
</template>

<style scoped>
.sub {
  margin: 0 0 6px;
  color: var(--accent);
}
dl {
  display: grid;
  grid-template-columns: auto 1fr;
  gap: 2px 10px;
  margin: 0 0 8px;
  font-size: 12px;
}
dt {
  color: var(--muted);
}
dd {
  margin: 0;
  overflow-wrap: anywhere;
}
.kv {
  width: 100%;
  border-collapse: collapse;
  font-size: 12px;
}
.kv th,
.kv td {
  padding: 3px 6px;
  border-bottom: 1px solid var(--line);
  text-align: left;
  vertical-align: top;
  word-break: break-all;
}
.kv th {
  color: var(--muted);
  font-weight: normal;
  width: 40%;
}
</style>
