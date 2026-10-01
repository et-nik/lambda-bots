<script setup lang="ts">
import { computed } from 'vue'

import { hex, kindColor, kindName } from '../format'
import { useNav } from '../stores/nav'
import { linkValid, OFF_COLOR } from '../viewer/graph'

const nav = useNav()

/** Link kinds in the graph, with their counts, and how many links are off. */
const legend = computed(() => {
  const counts = new Map<string, number>()
  let off = 0
  for (const l of nav.model?.links ?? []) {
    counts.set(l.kind, (counts.get(l.kind) ?? 0) + 1)
    if (!linkValid(l)) off++
  }
  const kinds = (nav.info?.kinds ?? []).filter((k) => counts.has(k)).map((k) => ({ kind: k, count: counts.get(k)! }))
  return { kinds, off }
})

const problems = computed(() => {
  const errors = nav.problems.filter((p) => p.level === 'error').length
  return { all: nav.problems.length, errors }
})

const graphTitle = computed(() => {
  const m = nav.model
  if (!m) return nav.loading ? 'The graph is loading' : 'No graph'
  const whose = nav.info?.origin === 'server' ? "the server's graph" : 'made here, with the default physics'
  return `${m.nodes} nodes, ${m.links.length} links · ${whose}. Click to ${nav.show ? 'hide' : 'show'} it.`
})
</script>

<template>
  <div class="legend" aria-label="The graph and its link kinds: click to hide or show">
    <button class="graph" :class="{ hidden: !nav.show }" :aria-pressed="nav.show" :title="graphTitle" @click="nav.show = !nav.show">
      Graph<span v-if="nav.loading" class="muted"> loading…</span>
    </button>
    <template v-if="nav.show">
      <button
        v-if="problems.all"
        class="problems"
        :class="{ hidden: !nav.problemsShown }"
        :aria-pressed="nav.problemsShown"
        :title="`${problems.errors} errors, ${problems.all - problems.errors} to look at: red and yellow in the view, listed in the Navigation tab; click to ${nav.problemsShown ? 'hide' : 'show'} them in the view`"
        @click="nav.showProblems(!nav.problemsShown)"
      >
        <i :class="problems.errors ? 'error' : 'attention'" />Problems {{ problems.all }}
      </button>
      <button
        class="only"
        :class="{ on: nav.onlySelected }"
        :aria-pressed="nav.onlySelected"
        title="Only the links of what is selected (I); with nothing selected, all of them"
        @click="nav.onlySelected = !nav.onlySelected"
      >
        Selected only
      </button>
      <button
        v-for="k in legend.kinds"
        :key="k.kind"
        :class="{ hidden: nav.hiddenKinds.includes(k.kind) }"
        :aria-pressed="!nav.hiddenKinds.includes(k.kind)"
        :title="`${kindName(k.kind)}: ${k.count} links; click to ${nav.hiddenKinds.includes(k.kind) ? 'show' : 'hide'}`"
        @click="nav.toggleKind(k.kind)"
      >
        <i :style="{ background: kindColor(k.kind) }" />{{ kindName(k.kind) }}
      </button>
      <button
        v-if="legend.off"
        :class="{ hidden: nav.hideOff }"
        :aria-pressed="!nav.hideOff"
        :title="`${legend.off} links are off: the check failed, or the live server disagreed; click to ${nav.hideOff ? 'show' : 'hide'}`"
        @click="nav.hideOff = !nav.hideOff"
      >
        <i :style="{ background: hex(OFF_COLOR) }" />off
      </button>
    </template>
  </div>
</template>

<style scoped>
.legend {
  position: absolute;
  z-index: 5;
  left: 8px;
  bottom: 8px;
  max-width: calc(100% - 16px);
  display: flex;
  flex-wrap: wrap;
  gap: 3px;
  padding: 4px;
  background: rgb(29 32 38 / 0.88);
  border: 1px solid var(--line);
  border-radius: 6px;
}
button {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  padding: 1px 7px;
  font-size: 11px;
  border-radius: 10px;
  background: #262a32;
}
.graph {
  font-weight: 600;
  color: var(--text-strong);
}
.only.on {
  background: var(--accent-dim);
  border-color: var(--accent);
  color: var(--text-strong);
}
button.hidden {
  opacity: 0.45;
  text-decoration: line-through;
}
i {
  width: 8px;
  height: 8px;
  border-radius: 50%;
}
i.error {
  background: var(--warn);
}
i.attention {
  background: var(--attention);
}
.muted {
  font-weight: normal;
}
</style>
