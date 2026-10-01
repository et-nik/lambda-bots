<script setup lang="ts">
import { computed, onBeforeUnmount } from 'vue'

import { describe, spot } from '../format'
import { LINK_KIND_CHOICES, useNav } from '../stores/nav'
import type { PatchOp } from '../types'
import CardHead from './CardHead.vue'

const props = defineProps<{ i: number }>()
const nav = useNav()

const p = computed(() => nav.patches[props.i])
const o = computed(() => nav.preview?.editor[props.i] ?? null)

function edit(change: Partial<PatchOp>) {
  nav.updatePatch(props.i, { ...p.value, ...change } as PatchOp)
}

function numberOf(e: Event): number {
  return Number((e.target as HTMLInputElement).value)
}

onBeforeUnmount(() => (nav.peek = null))
</script>

<template>
  <section>
    <CardHead
      :title="`Change ${i + 1}: ${describe(p, o)}`"
      @focus="nav.focusRequest++"
      @close="nav.clearSelection()"
    />
    <p class="result" :class="{ bad: o && !o.ok }">{{ o?.message ?? 'checking…' }}</p>
    <ul v-if="o?.refused.length" class="refused">
      <li
        v-for="r in o.refused"
        :key="`${r.from}:${r.to}`"
        @mouseenter="nav.peek = { nodes: [r.from, r.to], links: [] }"
        @mouseleave="nav.peek = null"
      >
        <span class="mark">●</span><span><b>{{ r.from }} → {{ r.to }}</b> not put in: {{ r.why }}</span>
      </li>
    </ul>
    <p v-if="o?.nodes.length" class="nodes">
      <span class="muted">{{ o.nodes.length === 1 ? 'Node' : 'Nodes' }}</span>
      <button
        v-for="n in o.nodes"
        :key="n"
        class="linkish"
        :title="`Select node ${n}`"
        @mouseenter="nav.peek = { nodes: [n], links: [] }"
        @mouseleave="nav.peek = null"
        @click="nav.goNode(n)"
      >
        {{ n }}
      </button>
    </p>

    <div class="fields">
      <template v-if="p.op === 'forbid'">
        <label>
          Radius
          <input type="number" min="8" step="8" :value="p.radius" @change="edit({ radius: numberOf($event) })" />
        </label>
        <span class="muted">at {{ spot(p.at) }}</span>
      </template>
      <template v-else-if="p.op === 'add_link'">
        <label>
          Kind
          <select
            :value="p.kind ?? 'auto'"
            @change="
              edit({
                kind: (($event.target as HTMLSelectElement).value === 'auto'
                  ? undefined
                  : ($event.target as HTMLSelectElement).value) as string | undefined,
              })
            "
          >
            <option v-for="k in LINK_KIND_CHOICES" :key="k.id" :value="k.id">{{ k.label }}</option>
          </select>
        </label>
        <label><input type="checkbox" :checked="p.both" @change="edit({ both: !p.both })" /> both ways</label>
        <label><input type="checkbox" :checked="p.trust" @change="edit({ trust: !p.trust })" /> trust</label>
      </template>
      <label v-else-if="p.op === 'remove_link'">
        <input type="checkbox" :checked="p.both" @change="edit({ both: !p.both })" /> both ways
      </label>
      <template v-else-if="p.op === 'add_node'">
        <label title="Link the node with the nodes around wherever the links check out">
          <input
            type="checkbox"
            :checked="p.link !== false"
            @change="edit({ link: p.link === false ? undefined : false })"
          />
          auto-link
        </label>
        <span class="muted">at {{ spot(p.at) }}; the Move tool moves it</span>
      </template>
      <span v-else-if="p.op === 'move_node'" class="muted">from {{ spot(p.from) }} to {{ spot(p.to) }}</span>
      <label class="note">
        Note
        <input
          type="text"
          :value="p.note ?? ''"
          @change="edit({ note: ($event.target as HTMLInputElement).value || undefined })"
        />
      </label>
    </div>
    <button @click="nav.removePatch(i)">Remove the change</button>
  </section>
</template>

<style scoped>
.refused {
  margin: 0 0 8px;
  padding: 0;
  list-style: none;
  font-size: 12px;
}
.refused li {
  display: flex;
  gap: 6px;
  margin: 2px 0;
}
.refused .mark {
  flex: none;
  color: var(--warn);
}
.refused b {
  color: var(--text-strong);
  font-weight: 600;
}
.result {
  margin: 0 0 6px;
  font-size: 12px;
  overflow-wrap: anywhere;
}
.bad {
  color: var(--warn);
}
.nodes {
  display: flex;
  flex-wrap: wrap;
  gap: 2px 8px;
  margin: 0 0 6px;
  font-size: 12px;
}
.fields {
  display: flex;
  flex-wrap: wrap;
  gap: 6px 12px;
  align-items: center;
  margin-bottom: 8px;
  font-size: 12px;
}
.fields label {
  display: flex;
  gap: 4px;
  align-items: center;
}
input[type='text'],
input[type='number'] {
  padding: 2px 6px;
  background: #1a1d22;
  border: 1px solid var(--line);
  border-radius: 3px;
}
input[type='number'] {
  width: 64px;
}
.note {
  flex-basis: 100%;
}
.note input {
  flex: 1;
}
select {
  padding: 1px 6px;
}
</style>
