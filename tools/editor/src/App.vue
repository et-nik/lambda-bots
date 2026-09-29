<script setup lang="ts">
import { onMounted, ref, watch } from 'vue'

import Inspector from './components/Inspector.vue'
import MapView from './components/MapView.vue'
import NavPanel from './components/NavPanel.vue'
import StatusBar from './components/StatusBar.vue'
import TopBar from './components/TopBar.vue'
import { useEditor } from './stores/editor'
import { useNav } from './stores/nav'

const store = useEditor()
const nav = useNav()
const tab = ref<'nav' | 'inspector'>('nav')

// The panel follows what was clicked: an entity opens the inspector, a node or a link the navigation.
watch(
  () => store.selected,
  (s) => {
    if (s !== null) tab.value = 'inspector'
  },
)
watch([() => nav.selNode, () => nav.selLink], ([n, l]) => {
  if (n !== null || l !== null) tab.value = 'nav'
})

onMounted(() => store.start())
</script>

<template>
  <div class="app">
    <TopBar />
    <main class="main">
      <MapView />
      <section class="side">
        <nav class="tabs" role="tablist">
          <button role="tab" :aria-selected="tab === 'nav'" :class="{ on: tab === 'nav' }" @click="tab = 'nav'">
            Navigation<span v-if="nav.dirty" class="dot" title="Unsaved changes">•</span>
          </button>
          <button
            role="tab"
            :aria-selected="tab === 'inspector'"
            :class="{ on: tab === 'inspector' }"
            @click="tab = 'inspector'"
          >
            Inspector
          </button>
        </nav>
        <div class="pane">
          <NavPanel v-show="tab === 'nav'" />
          <Inspector v-show="tab === 'inspector'" />
        </div>
      </section>
      <div v-if="store.error" class="error" role="alert">
        <p>{{ store.error }}</p>
        <button @click="store.error = null">Close</button>
      </div>
    </main>
    <StatusBar />
  </div>
</template>

<style scoped>
.app {
  display: grid;
  grid-template-rows: auto 1fr auto;
  height: 100vh;
}
.main {
  position: relative;
  display: grid;
  grid-template-columns: 1fr 360px;
  min-height: 0;
}
.side {
  display: grid;
  grid-template-rows: auto 1fr;
  min-height: 0;
  background: var(--panel);
  border-left: 1px solid var(--line);
}
.tabs {
  display: flex;
  border-bottom: 1px solid var(--line);
}
.tabs button {
  flex: 1;
  border: none;
  border-radius: 0;
  background: transparent;
  padding: 7px 10px;
  color: var(--muted);
}
.tabs button.on {
  color: var(--text-strong);
  box-shadow: inset 0 -2px 0 var(--accent);
}
.dot {
  color: var(--accent);
  margin-left: 4px;
}
.pane {
  overflow: auto;
  padding: 12px;
  min-height: 0;
}
.error {
  position: absolute;
  left: 50%;
  top: 24px;
  transform: translateX(-50%);
  max-width: min(640px, 90%);
  padding: 12px 16px;
  background: var(--panel);
  border: 1px solid var(--warn);
  border-radius: 6px;
  box-shadow: 0 8px 24px rgb(0 0 0 / 0.5);
  white-space: pre-wrap;
}
.error p {
  margin: 0 0 8px;
}
@media (max-width: 800px) {
  .main {
    grid-template-columns: 1fr;
    grid-template-rows: 1fr 40%;
  }
}
</style>
