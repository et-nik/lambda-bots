<script setup lang="ts">
import { computed } from 'vue'

import { kindColor, kindName } from '../format'
import { LINK, type Link, linkValid } from '../viewer/graph'

/** One link as a button: its way (→ out, ← in, ⇄ both), kind and cost; a click selects it. */
const props = defineProps<{ way: '→' | '←' | '⇄'; link: Link }>()
defineEmits<{ pick: [] }>()

const valid = computed(() => linkValid(props.link))
const trusted = computed(() => (props.link.flags & LINK.TRUSTED) !== 0)
</script>

<template>
  <button
    class="seg"
    :class="{ off: !valid }"
    :title="`Select the link ${link.from} → ${link.to}${valid ? '' : ' (off: the check failed, or the live server disagreed)'}`"
    @click="$emit('pick')"
  >
    <span class="way">{{ way }}</span>
    <i :style="{ background: kindColor(link.kind, valid) }" />
    <span>{{ kindName(link.kind) }}</span>
    <span class="cost">{{ link.cost.toFixed(2) }} s</span>
    <span v-if="!valid" class="tag">off</span>
    <span v-if="trusted" class="tag">trusted</span>
    <span v-if="link.added" class="tag changed" title="Put in or changed by the overlays">changed</span>
  </button>
</template>

<style scoped>
.seg {
  display: inline-flex;
  gap: 5px;
  align-items: center;
  padding: 1px 6px;
  background: transparent;
  border-color: transparent;
  font-size: 12px;
  white-space: nowrap;
}
.seg:hover {
  background: #2a2f37;
  border-color: var(--line);
}
.way {
  color: var(--muted);
  width: 1em;
  text-align: center;
}
i {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  flex: none;
}
.cost {
  color: var(--muted);
  font-variant-numeric: tabular-nums;
}
.off {
  color: var(--warn);
}
.changed {
  color: #ff4df0;
}
</style>
