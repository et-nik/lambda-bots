<script setup lang="ts">
import { computed } from 'vue'

import { useEditor } from '../stores/editor'
import { useNav } from '../stores/nav'
import ChangeCard from './ChangeCard.vue'
import EntityCard from './EntityCard.vue'
import LinkCard from './LinkCard.vue'
import NodeCard from './NodeCard.vue'
import RouteCard from './RouteCard.vue'

const nav = useNav()
const store = useEditor()

const node = computed(() => {
  const n = nav.selNode
  return n !== null && nav.model && n < nav.model.nodes ? n : null
})
const patch = computed(() => (nav.selPatch !== null && nav.patches[nav.selPatch] ? nav.selPatch : null))
</script>

<template>
  <NodeCard v-if="node !== null" :key="`n${node}`" :n="node" />
  <LinkCard v-else-if="nav.selLink" :key="`l${nav.selLink.join()}`" :link="nav.selLink" />
  <ChangeCard v-else-if="patch !== null" :key="`c${patch}`" :i="patch" />
  <EntityCard v-else-if="store.entity" />
  <RouteCard v-if="nav.route" class="route" />
</template>

<style scoped>
.route:not(:first-child) {
  margin-top: 10px;
  padding-top: 10px;
  border-top: 1px solid var(--line);
}
</style>
