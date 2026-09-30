<script setup lang="ts">
import { computed, onBeforeUnmount } from 'vue'

import { spot } from '../format'
import { useNav } from '../stores/nav'
import { LINK, type Link, linkValid, nodeFlagNames } from '../viewer/graph'
import CardHead from './CardHead.vue'
import LinkSeg from './LinkSeg.vue'

const props = defineProps<{ n: number }>()
const nav = useNav()

/** A neighbor with the links to it and back; `same` when both read alike and show as one ⇄. */
interface Row {
  to: number
  out: Link | undefined
  back: Link | undefined
  same: boolean
}

const model = computed(() => nav.model!)

const rows = computed<Row[]>(() => {
  const m = model.value
  const byNode = new Map<number, Row>()
  for (const l of m.linksOut(props.n)) {
    if (!byNode.has(l.to)) byNode.set(l.to, { to: l.to, out: l, back: undefined, same: false })
  }
  for (const l of m.linksIn(props.n)) {
    const r = byNode.get(l.from)
    if (!r) byNode.set(l.from, { to: l.from, out: undefined, back: l, same: false })
    else if (!r.back) r.back = l
  }
  for (const r of byNode.values()) {
    const { out, back } = r
    r.same =
      !!out &&
      !!back &&
      out.kind === back.kind &&
      linkValid(out) === linkValid(back) &&
      out.added === back.added &&
      ((out.flags ^ back.flags) & LINK.TRUSTED) === 0 &&
      Math.abs(out.cost - back.cost) < 0.005
  }
  return [...byNode.values()]
})

const counts = computed(() => ({
  out: rows.value.filter((r) => r.out).length,
  in: rows.value.filter((r) => r.back).length,
}))
const flags = computed(() => nodeFlagNames(model.value.flags[props.n]))
const movedFrom = computed(() => model.value.movedFrom.get(props.n) ?? null)
const placed = computed(() => nav.placedBy(props.n))
const shut = computed(() => nav.shutBy(props.n))

function peek(r: Row) {
  const links: [number, number][] = []
  if (r.out) links.push([props.n, r.to])
  if (r.back) links.push([r.to, props.n])
  nav.peek = { nodes: [r.to], links }
}

function linkFrom() {
  nav.setTool('link')
  nav.pending = props.n
}

onBeforeUnmount(() => (nav.peek = null))
</script>

<template>
  <section class="node">
    <CardHead
      :title="`Node ${n}`"
      :back="nav.backLabel"
      @back="nav.back()"
      @focus="nav.focusRequest++"
      @close="nav.clearSelection()"
    >
      <span v-for="f in flags" :key="f" class="tag">{{ f }}</span>
    </CardHead>
    <p class="sub">
      {{ spot(model.origin(n)) }}<template v-if="movedFrom"> · moved from {{ spot(movedFrom) }}</template> ·
      {{ counts.out }} out · {{ counts.in }} in
    </p>
    <p v-if="shut !== null" class="note warn">
      Shut off by <button class="linkish" @click="nav.goPatch(shut)">change {{ shut + 1 }}</button>: bots never plan
      through it.
    </p>
    <p v-if="placed !== null" class="note">
      {{ nav.patches[placed].op === 'add_node' ? 'Put in' : 'Moved' }} by
      <button class="linkish" @click="nav.goPatch(placed)">change {{ placed + 1 }}</button>
    </p>

    <ul v-if="rows.length" class="rows" aria-label="Links: a node number selects that node, a link selects the link" @mouseleave="nav.peek = null">
      <li v-for="r in rows" :key="r.to" @mouseenter="peek(r)">
        <button class="to" :title="`Select node ${r.to}`" @click="nav.goNode(r.to)">{{ r.to }}</button>
        <span class="segs">
          <LinkSeg v-if="r.same" way="⇄" :link="r.out!" @pick="nav.goLink([n, r.to])" />
          <template v-else>
            <LinkSeg v-if="r.out" way="→" :link="r.out" @pick="nav.goLink([n, r.to])" />
            <LinkSeg v-if="r.back" way="←" :link="r.back" @pick="nav.goLink([r.to, n])" />
          </template>
        </span>
        <button
          class="x"
          :title="r.out && r.back ? `Take the links with ${r.to} out, both ways` : `Take the link with ${r.to} out`"
          :aria-label="`Unlink ${r.to}`"
          @click="nav.unlinkPair(n, r.to)"
        >
          ×
        </button>
      </li>
    </ul>
    <p v-else class="note warn">No links: bots can neither reach it nor leave it.</p>

    <div class="acts">
      <button title="The Link tool, starting here" @click="linkFrom">Link from here</button>
      <button title="The Route tool, starting here" @click="nav.routeFromNode(n)">Route from here</button>
      <button
        :disabled="shut !== null"
        title="A forbidden zone of 16 units about the node: bots never plan through it"
        @click="nav.shutOff(n)"
      >
        Shut off
      </button>
    </div>
  </section>
</template>

<style scoped>
.sub {
  margin: 0 0 6px;
  color: var(--muted);
  font-size: 12px;
  font-variant-numeric: tabular-nums;
}
.note {
  margin: 0 0 6px;
  font-size: 12px;
}
.warn {
  color: var(--warn);
}
.rows {
  list-style: none;
  margin: 0 0 8px;
  padding: 0;
}
.rows li {
  display: flex;
  align-items: flex-start;
  gap: 4px;
  padding: 1px 0;
  border-radius: 3px;
}
.rows li:hover {
  background: #262a31;
}
.to {
  flex: none;
  min-width: 5ch;
  padding: 1px 4px;
  background: transparent;
  border-color: transparent;
  color: var(--accent);
  font-size: 12px;
  font-variant-numeric: tabular-nums;
  text-align: right;
}
.to:hover {
  border-color: var(--line);
  text-decoration: underline;
}
.segs {
  flex: 1;
  display: flex;
  flex-wrap: wrap;
  min-width: 0;
}
.x {
  flex: none;
  padding: 0 6px;
  line-height: 20px;
  background: transparent;
  border-color: transparent;
  color: var(--muted);
  visibility: hidden;
}
.rows li:hover .x,
.x:focus-visible {
  visibility: visible;
}
.x:hover {
  color: var(--warn);
  border-color: var(--line);
}
.acts {
  display: flex;
  flex-wrap: wrap;
  gap: 4px;
}
button:disabled {
  opacity: 0.45;
  cursor: default;
}
</style>
