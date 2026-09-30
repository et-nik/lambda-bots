<script setup lang="ts">
import { computed, ref } from 'vue'

import { kindName } from '../format'
import { useNav } from '../stores/nav'
import type { Problem } from '../types'

const nav = useNav()

/** Rows listed at most: a big map's weak jumps are many. */
const LISTED = 150

const LABELS: Record<Problem['kind'], string> = {
  refused: 'not put in',
  failed: 'failed in runs',
  missed: 'missed in runs',
  idle: 'does nothing',
  weak: 'weak',
  trusted: 'not checked',
  fall: 'falls',
}

const open = ref(true)
const counts = computed(() => {
  const errors = nav.problems.filter((p) => p.level === 'error').length
  return { errors, attention: nav.problems.length - errors }
})
const listed = computed(() => nav.problems.slice(0, LISTED))

function title(p: Problem): string {
  if (p.kind === 'idle' && p.patch) {
    return `${p.patch[0] === 'editor' ? 'Change' : 'overlay.yaml, change'} ${p.patch[1] + 1}: ${p.link}`
  }
  return `${p.from ?? '?'} → ${p.to ?? '?'} ${kindName(p.link)}`
}
</script>

<template>
  <section v-if="nav.problems.length" class="problems">
    <button class="head" :aria-expanded="open" @click="open = !open">
      <span class="caret">{{ open ? '▾' : '▸' }}</span>
      Problems
      <span v-if="counts.errors" class="count error" :title="`${counts.errors} errors`">{{ counts.errors }}</span>
      <span v-if="counts.attention" class="count attention" :title="`${counts.attention} to look at`">
        {{ counts.attention }}
      </span>
    </button>
    <ol v-if="open" class="list" @mouseleave="nav.peekProblem(null)">
      <li
        v-for="(p, i) in listed"
        :key="i"
        :class="p.level"
        :title="p.why"
        @mouseenter="nav.peekProblem(p)"
        @click="nav.goProblem(p)"
      >
        <span class="mark" :aria-label="p.level === 'error' ? 'error' : 'to look at'">{{ p.level === 'error' ? '●' : '▲' }}</span>
        <span class="what">{{ title(p) }}</span>
        <span class="kind">{{ LABELS[p.kind] }}</span>
        <span class="why">{{ p.why }}</span>
      </li>
    </ol>
    <p v-if="open && nav.problems.length > LISTED" class="more">
      {{ nav.problems.length - LISTED }} more; the view marks them all.
    </p>
  </section>
</template>

<style scoped>
.problems {
  margin-top: 12px;
  padding-top: 8px;
  border-top: 1px solid var(--line);
}
.head {
  display: flex;
  align-items: center;
  gap: 6px;
  width: 100%;
  padding: 2px 0;
  background: none;
  border: 0;
  font-weight: 600;
  color: var(--text-strong);
  text-align: left;
}
.caret {
  width: 10px;
  color: var(--muted);
}
.count {
  padding: 0 6px;
  border-radius: 8px;
  font-size: 11px;
  font-weight: 600;
  color: #15171b;
}
.count.error {
  background: var(--warn);
}
.count.attention {
  background: var(--attention);
}
.list {
  margin: 6px 0 0;
  padding: 0;
  list-style: none;
  max-height: 45vh;
  overflow: auto;
}
.list li {
  display: grid;
  grid-template-columns: 14px 1fr auto;
  column-gap: 6px;
  padding: 4px 4px;
  border-radius: 4px;
  cursor: pointer;
  font-size: 12px;
}
.list li:hover {
  background: #262a32;
}
.mark {
  grid-row: span 2;
  line-height: 18px;
}
.error .mark {
  color: var(--warn);
}
.attention .mark {
  color: var(--attention);
}
.what {
  color: var(--text-strong);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.kind {
  font-size: 11px;
  color: var(--muted);
}
.why {
  grid-column: 2 / span 2;
  color: var(--muted);
  overflow-wrap: anywhere;
}
.more {
  margin: 4px 0 0;
  font-size: 11px;
  color: var(--muted);
}
</style>
