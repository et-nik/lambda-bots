<script setup lang="ts">
import { computed } from 'vue'

import { useEditor } from '../stores/editor'
import { useNav } from '../stores/nav'
import ProblemList from './ProblemList.vue'
import SelectionCard from './SelectionCard.vue'

const nav = useNav()
const store = useEditor()

const selected = computed(
  () => nav.selNode !== null || nav.selLink !== null || nav.selPatch !== null || store.entity !== null || nav.route !== null,
)
</script>

<template>
  <div class="nav" aria-label="Selected">
    <SelectionCard v-if="selected" />
    <p v-else class="muted empty">
      Nothing is selected. Select (V) picks a node, a link or an entity in the view; the list of changes opens from
      the status bar (C).
    </p>
    <ProblemList />
  </div>
</template>

<style scoped>
.nav {
  height: 100%;
  overflow: auto;
  padding: 10px 12px;
}
.empty {
  margin: 0;
  font-size: 12px;
  line-height: 1.5;
}
</style>
