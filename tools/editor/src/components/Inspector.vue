<script setup lang="ts">
import { computed } from 'vue'

import { useEditor } from '../stores/editor'

const store = useEditor()

const classes = computed(() => {
  const counts = new Map<string, number>()
  for (const e of store.manifest?.entities ?? []) {
    counts.set(e.classname, (counts.get(e.classname) ?? 0) + 1)
  }
  return [...counts].sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]))
})

const model = computed(() => {
  const e = store.entity
  return e?.model == null ? null : (store.manifest?.models.find((m) => m.index === e.model) ?? null)
})

function fmt(v: number[]): string {
  return v.map((x) => Math.round(x)).join(' ')
}
</script>

<template>
  <aside class="inspector">
    <template v-if="store.entity">
      <h2>{{ store.entity.classname }}</h2>
      <p v-if="store.entity.targetname" class="sub">{{ store.entity.targetname }}</p>
      <dl>
        <dt>Entity</dt>
        <dd>#{{ store.entity.index }}</dd>
        <template v-if="model">
          <dt>Model</dt>
          <dd>*{{ model.index }}, layer {{ model.layer }}</dd>
          <dt>Bounds</dt>
          <dd>{{ fmt(model.mins) }} → {{ fmt(model.maxs) }}</dd>
        </template>
        <template v-else>
          <dt>Origin</dt>
          <dd>{{ fmt(store.entity.origin) }}</dd>
        </template>
      </dl>
      <table class="kv">
        <tbody>
          <tr v-for="([k, v], i) in store.entity.kv" :key="i">
            <th>{{ k }}</th>
            <td>{{ v }}</td>
          </tr>
        </tbody>
      </table>
      <button class="clear" @click="store.select(null)">Clear selection</button>
    </template>

    <template v-else-if="store.manifest">
      <h2>{{ store.manifest.map }}</h2>
      <dl>
        <dt>Faces</dt>
        <dd>{{ store.manifest.stats.faces }} ({{ store.manifest.stats.triangles }} triangles)</dd>
        <dt>Textures</dt>
        <dd>
          {{ store.manifest.textures.length }}
          <span v-if="store.manifest.stats.missing_textures" class="warn">
            , {{ store.manifest.stats.missing_textures }} missing
          </span>
        </dd>
        <dt>Lightmaps</dt>
        <dd>
          {{ store.manifest.lightmaps.pages }} × {{ store.manifest.lightmaps.size }}²
          <span v-if="store.manifest.stats.lightmaps.mismatched" class="warn">
            , {{ store.manifest.stats.lightmaps.mismatched }} faces off
          </span>
        </dd>
        <dt>WADs</dt>
        <dd>
          <span v-for="w in store.manifest.wads" :key="w.name" :class="{ warn: !w.found }" class="wad">
            {{ w.name }}
          </span>
        </dd>
      </dl>
      <h3>Worldspawn</h3>
      <table class="kv">
        <tbody>
          <tr v-for="([k, v], i) in store.manifest.entities[0]?.kv.filter(([k]) => k !== 'classname') ?? []" :key="i">
            <th>{{ k }}</th>
            <td>{{ v }}</td>
          </tr>
        </tbody>
      </table>
      <h3>Entities</h3>
      <table class="kv">
        <tbody>
          <tr v-for="[c, n] in classes" :key="c">
            <th>{{ c }}</th>
            <td class="num">{{ n }}</td>
          </tr>
        </tbody>
      </table>
    </template>
  </aside>
</template>

<style scoped>
.inspector {
  min-width: 0;
}
h2 {
  margin: 0 0 2px;
  font-size: 15px;
  color: var(--text-strong);
  word-break: break-all;
}
h3 {
  margin: 16px 0 6px;
  font-size: 12px;
  text-transform: uppercase;
  letter-spacing: 0.06em;
  color: var(--muted);
}
.sub {
  margin: 0 0 8px;
  color: var(--accent);
}
dl {
  display: grid;
  grid-template-columns: auto 1fr;
  gap: 4px 10px;
  margin: 10px 0;
}
dt {
  color: var(--muted);
}
dd {
  margin: 0;
  word-break: break-word;
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
.num {
  text-align: right;
  font-variant-numeric: tabular-nums;
}
.warn {
  color: var(--warn);
}
.wad {
  display: inline-block;
  margin-right: 8px;
}
.clear {
  margin-top: 12px;
}
</style>
