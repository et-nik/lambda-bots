<script setup lang="ts">
import { onBeforeUnmount, ref, watch } from 'vue'

import { useNav } from '../stores/nav'

/** How long a notice stays unless the pointer is on it (ms). */
const STAYS = 7000

const nav = useNav()
const shown = ref(false)
let timer = 0

function hold() {
  clearTimeout(timer)
}

function hideLater(ms = STAYS) {
  clearTimeout(timer)
  timer = window.setTimeout(() => (shown.value = false), ms)
}

watch(
  () => nav.noticeSeq,
  () => {
    if (!nav.notice) return
    shown.value = true
    hideLater()
  },
)
watch(
  () => nav.notice,
  (n) => {
    if (!n) shown.value = false
  },
)
onBeforeUnmount(() => clearTimeout(timer))
</script>

<template>
  <div
    v-if="shown && nav.notice"
    class="toast"
    role="status"
    aria-live="polite"
    @mouseenter="hold"
    @mouseleave="hideLater(2500)"
  >
    <span>{{ nav.notice }}</span>
    <button aria-label="Close" @click="shown = false">×</button>
  </div>
</template>

<style scoped>
.toast {
  position: absolute;
  z-index: 6;
  left: 50%;
  bottom: 44px;
  transform: translateX(-50%);
  max-width: min(560px, calc(100% - 32px));
  display: flex;
  gap: 10px;
  align-items: flex-start;
  padding: 8px 10px 8px 12px;
  background: #1f2a33;
  border: 1px solid #34506a;
  border-radius: 6px;
  box-shadow: 0 6px 18px rgb(0 0 0 / 0.45);
  font-size: 12px;
  word-break: break-word;
}
button {
  padding: 0 6px;
  line-height: 1.3;
}
</style>
