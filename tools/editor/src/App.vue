<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref, watch } from 'vue'

import MapInfo from './components/MapInfo.vue'
import MapView from './components/MapView.vue'
import NavPanel from './components/NavPanel.vue'
import StatusBar from './components/StatusBar.vue'
import TopBar from './components/TopBar.vue'
import { installShortcuts } from './shortcuts'
import { useEditor } from './stores/editor'
import { useNav } from './stores/nav'

const store = useEditor()
const nav = useNav()
const tab = ref<'nav' | 'map'>('nav')

// What is selected shows in the navigation tab, an entity too; the one selected last is the selection.
watch([() => store.selected, () => nav.selNode, () => nav.selLink, () => nav.selPatch], (now) => {
  if (now.some((s) => s !== null)) tab.value = 'nav'
})
watch(
  () => store.selected,
  (s) => {
    if (s !== null) nav.clearSelection()
  },
)

let uninstall = () => {}
onMounted(() => {
  store.start()
  uninstall = installShortcuts()
})
onBeforeUnmount(() => uninstall())
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
          <button role="tab" :aria-selected="tab === 'map'" :class="{ on: tab === 'map' }" @click="tab = 'map'">
            Map
          </button>
        </nav>
        <div class="pane">
          <NavPanel v-show="tab === 'nav'" />
          <div v-show="tab === 'map'" class="scroll">
            <MapInfo />
          </div>
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
  min-height: 0;
  overflow: hidden;
}
.pane > * {
  height: 100%;
}
.scroll {
  overflow: auto;
  padding: 12px;
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
