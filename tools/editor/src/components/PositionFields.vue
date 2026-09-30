<script setup lang="ts">
import { computed, onBeforeUnmount, ref } from 'vue'

import { useNav } from '../stores/nav'
import type { Vec3 } from '../types'

/**
 * Where node `n` stands, to type or step (↑/↓, Shift ×10) somewhere else: each value is a move at once, and all of
 * them until the fields are left are one step of undo. Esc takes them back.
 */
const props = defineProps<{ n: number }>()
const nav = useNav()

const AXES = ['X', 'Y', 'Z'] as const

const editing = ref(false)
/** Where the node is being set down while the fields are in use. */
const target = ref<Vec3>([0, 0, 0])
let changed = false
let leaving = 0

const why = computed(() => nav.unmovable(props.n))
const here = computed<Vec3>(() => {
  const o = nav.model?.origin(props.n) ?? [0, 0, 0]
  return [Math.round(o[0]), Math.round(o[1]), Math.round(o[2])]
})
const shown = computed(() => (editing.value ? target.value : here.value))

function enter() {
  clearTimeout(leaving)
  if (editing.value) return
  editing.value = true
  changed = false
  target.value = [...here.value]
}

function finish() {
  clearTimeout(leaving)
  if (!editing.value) return
  editing.value = false
  nav.moveStop()
}

function leave() {
  // Focus may be going to the next of the three fields.
  leaving = window.setTimeout(finish, 0)
}

function set(i: number, v: number) {
  if (!Number.isFinite(v) || v === target.value[i]) return
  const t: Vec3 = [...target.value]
  t[i] = Math.round(v)
  target.value = t
  changed = true
  nav.moveTo(props.n, t)
}

function typed(e: Event, i: number) {
  enter()
  const input = e.target as HTMLInputElement
  const v = Number(input.value.trim())
  if (input.value.trim() === '' || !Number.isFinite(v)) {
    input.value = String(target.value[i])
    return
  }
  set(i, v)
}

function key(e: KeyboardEvent, i: number) {
  enter()
  if (e.key === 'ArrowUp' || e.key === 'ArrowDown') {
    e.preventDefault()
    const step = nav.snapStep * (e.shiftKey ? 10 : 1)
    set(i, target.value[i] + (e.key === 'ArrowUp' ? step : -step))
  } else if (e.key === 'Enter') {
    typed(e, i)
  } else if (e.key === 'Escape') {
    e.preventDefault()
    e.stopPropagation()
    if (changed) nav.undo()
    changed = false
    finish()
    ;(e.target as HTMLInputElement).blur()
  }
}

onBeforeUnmount(finish)
</script>

<template>
  <div class="pos" :title="why ?? 'Type or step with ↑/↓ (Shift: ×10); the node stands on the floor under the point'">
    <span class="muted">Position</span>
    <label v-for="(name, i) in AXES" :key="name">
      <span :class="`axis ${name.toLowerCase()}`">{{ name }}</span>
      <input
        type="text"
        inputmode="numeric"
        spellcheck="false"
        :value="shown[i]"
        :disabled="!!why"
        :aria-label="`${name} of node ${n}`"
        @focus="enter"
        @blur="leave"
        @keydown="key($event, i)"
        @change="typed($event, i)"
      />
    </label>
  </div>
  <p v-if="editing" class="hint">
    ↑/↓ {{ nav.snapStep }} · Shift ×10 · Enter sets a typed value · Esc takes it back · Z picks the floor it stands on
  </p>
</template>

<style scoped>
.pos {
  display: flex;
  gap: 8px;
  align-items: center;
  margin: 0 0 4px;
  font-size: 12px;
}
label {
  display: flex;
  gap: 3px;
  align-items: center;
}
input {
  width: 62px;
  padding: 1px 5px;
  background: #1a1d22;
  border: 1px solid var(--line);
  border-radius: 3px;
  font-variant-numeric: tabular-nums;
  text-align: right;
}
input:focus {
  border-color: var(--accent);
  outline: none;
}
input:disabled {
  opacity: 0.6;
}
.axis {
  font-weight: 600;
}
.x {
  color: #e8574f;
}
.y {
  color: #6cc44a;
}
.z {
  color: #4a8cf0;
}
.hint {
  margin: 0 0 6px;
  color: var(--muted);
  font-size: 11px;
}
</style>
