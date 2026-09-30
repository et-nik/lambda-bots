<script setup lang="ts">
import { computed, onBeforeUnmount, ref } from 'vue'

import { kindColor, kindName, seconds } from '../format'
import { LINK_KIND_CHOICES, type LinkKindChoice, useNav } from '../stores/nav'
import { LINK, linkValid } from '../viewer/graph'
import CardHead from './CardHead.vue'
import LinkSeg from './LinkSeg.vue'

const props = defineProps<{ link: [number, number] }>()
const nav = useNav()

const a = computed(() => props.link[0])
const b = computed(() => props.link[1])
const l = computed(() => nav.model?.find(a.value, b.value))
const back = computed(() => nav.model?.find(b.value, a.value))
const linked = computed(() => nav.linkedBy(a.value, b.value))
const kind = ref<LinkKindChoice>('auto')
const trust = ref(false)

onBeforeUnmount(() => (nav.peek = null))
</script>

<template>
  <section>
    <CardHead
      :title="`Link ${a} → ${b}`"
      :back="nav.backLabel"
      @back="nav.back()"
      @focus="nav.focusRequest++"
      @close="nav.clearSelection()"
    />
    <template v-if="l">
      <p class="what">
        <i :style="{ background: kindColor(l.kind, linkValid(l)) }" />
        {{ kindName(l.kind) }} · {{ seconds(l.cost) }}
        <span v-if="!linkValid(l)" class="tag off">off</span>
        <span v-if="l.flags & LINK.TRUSTED" class="tag">trusted</span>
        <span v-if="l.added" class="tag changed">changed</span>
      </p>
      <p v-if="linked !== null" class="note">
        Put in by <button class="linkish" @click="nav.goPatch(linked)">change {{ linked + 1 }}</button>
      </p>
    </template>
    <p v-else class="note">No such link now: a change took it out.</p>

    <dl>
      <dt>From</dt>
      <dd>
        <button class="linkish" @mouseenter="nav.peek = { nodes: [a], links: [] }" @mouseleave="nav.peek = null" @click="nav.goNode(a)">
          node {{ a }}
        </button>
      </dd>
      <dt>To</dt>
      <dd>
        <button class="linkish" @mouseenter="nav.peek = { nodes: [b], links: [] }" @mouseleave="nav.peek = null" @click="nav.goNode(b)">
          node {{ b }}
        </button>
      </dd>
      <dt>Back</dt>
      <dd @mouseenter="back && (nav.peek = { nodes: [], links: [[b, a]] })" @mouseleave="nav.peek = null">
        <LinkSeg v-if="back" way="←" :link="back" @pick="nav.goLink([b, a])" />
        <span v-else class="muted">no link back</span>
      </dd>
    </dl>

    <div v-if="l" class="acts">
      <button @click="nav.unlinkOne(a, b, false)">Unlink this way</button>
      <button v-if="back" @click="nav.unlinkOne(a, b, true)">Unlink both ways</button>
    </div>
    <div class="again">
      <span class="muted">Put in again as</span>
      <select v-model="kind" aria-label="Kind">
        <option v-for="k in LINK_KIND_CHOICES" :key="k.id" :value="k.id">{{ k.label }}</option>
      </select>
      <label title="Put it in even if the check fails"><input v-model="trust" type="checkbox" /> trust</label>
      <button @click="nav.relink(a, b, kind, trust)">Put in</button>
    </div>
  </section>
</template>

<style scoped>
.what {
  display: flex;
  gap: 6px;
  align-items: center;
  margin: 0 0 6px;
}
.what i {
  width: 9px;
  height: 9px;
  border-radius: 50%;
}
.note {
  margin: 0 0 6px;
  font-size: 12px;
}
.off {
  color: var(--warn);
}
.changed {
  color: #ff4df0;
}
dl {
  display: grid;
  grid-template-columns: auto 1fr;
  gap: 2px 10px;
  align-items: center;
  margin: 0 0 8px;
  font-size: 12px;
}
dt {
  color: var(--muted);
}
dd {
  margin: 0;
}
.acts,
.again {
  display: flex;
  flex-wrap: wrap;
  gap: 4px 6px;
  align-items: center;
  margin-bottom: 6px;
}
.again {
  font-size: 12px;
}
.again label {
  display: flex;
  gap: 4px;
  align-items: center;
}
select {
  padding: 1px 6px;
}
</style>
