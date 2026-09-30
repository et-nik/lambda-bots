<script setup lang="ts">
import { computed } from 'vue'

import { useEditor } from '../stores/editor'
import { useNav } from '../stores/nav'
import ChangeList from './ChangeList.vue'
import SelectionCard from './SelectionCard.vue'

const nav = useNav()
const store = useEditor()

const selected = computed(
  () => nav.selNode !== null || nav.selLink !== null || nav.selPatch !== null || store.entity !== null || nav.route !== null,
)
</script>

<template>
  <div class="nav">
    <div v-if="selected" class="selection" aria-label="Selected">
      <SelectionCard />
    </div>
    <div class="changes">
      <ChangeList />
    </div>
  </div>
</template>

<style scoped>
.nav {
  display: flex;
  flex-direction: column;
  height: 100%;
  min-height: 0;
}
.selection {
  flex: 0 1 auto;
  max-height: 60%;
  overflow: auto;
  padding: 10px 12px;
  border-bottom: 1px solid var(--line);
  background: #20242a;
}
.changes {
  flex: 1 1 auto;
  min-height: 0;
  overflow: auto;
  padding: 10px 12px;
}
</style>
