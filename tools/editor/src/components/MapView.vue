<script setup lang="ts">
import * as THREE from 'three'
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'

import { meshBuffer } from '../api'
import { useEditor } from '../stores/editor'
import { useNav } from '../stores/nav'
import type { PatchOp } from '../types'
import type { Highlight } from '../viewer/navlayer'
import { Viewer } from '../viewer/Viewer'
import Legend from './Legend.vue'
import Toast from './Toast.vue'
import ToolBar from './ToolBar.vue'
import ToolOptions from './ToolOptions.vue'

const store = useEditor()
const nav = useNav()
const host = ref<HTMLDivElement>()
let viewer: Viewer | null = null
let loads = 0

async function load() {
  const m = store.manifest
  if (!m || !viewer) {
    return
  }
  const mine = ++loads
  store.loading = m.map
  try {
    const buffer = await meshBuffer(m)
    if (mine !== loads) {
      return
    }
    viewer.load(m, buffer)
    viewer.setBrightness(store.brightness)
    viewer.setWireframe(store.wireframe)
    viewer.select(store.selected)
  } catch (e) {
    store.error = `${m.map}: ${e instanceof Error ? e.message : String(e)}`
  } finally {
    if (mine === loads) {
      store.loading = null
    }
  }
}

function syncNav() {
  if (!viewer) return
  viewer.nav.setVisible(nav.show)
  viewer.nav.setGraph(nav.model, nav.forbidden, new Set(nav.hiddenKinds), nav.hideOff, nav.focusNodes, !gizmo.value)
  viewer.nav.setProblems(nav.problems)
  viewer.nav.setProblemsVisible(nav.problemsShown)
  viewer.nav.setMarkup(nav.draft, nav.info?.overlay.file ?? null)
  viewer.nav.setHighlight(highlight.value)
  viewer.setFocusTarget(focusBox.value)
}

/** The node the move gizmo stands on: the Move tool's selection, when it can be moved. */
const gizmo = computed(() => {
  const n = nav.selNode
  const m = nav.model
  if (nav.tool !== 'move' || n === null || !m || n >= m.nodes || nav.unmovable(n)) return null
  return { node: n, at: m.origin(n) }
})

const highlight = computed<Highlight>(() => ({
  // The gizmo marks the node it stands on.
  node: gizmo.value ? null : nav.selNode,
  link: nav.selLink,
  hover: nav.hover,
  pending: nav.pending,
  routeFrom: nav.routeFrom,
  route: nav.route,
  patch: nav.selPatch === null ? null : (nav.preview?.editor[nav.selPatch] ?? null),
  peek: nav.peek,
}))

function patchPoints(p: PatchOp): [number, number, number][] {
  return p.op === 'add_link' || p.op === 'remove_link' || p.op === 'move_node' ? [p.from, p.to] : [p.at]
}

/** What F and the panel's list bring into view: the selected node, link, change or route. */
const focusBox = computed<THREE.Box3 | null>(() => {
  const m = nav.model
  const points: [number, number, number][] = []
  if (m && nav.selNode !== null && nav.selNode < m.nodes) points.push(m.origin(nav.selNode))
  if (m && nav.selLink) points.push(m.origin(nav.selLink[0]), m.origin(nav.selLink[1]))
  if (nav.selPatch !== null && nav.patches[nav.selPatch]) points.push(...patchPoints(nav.patches[nav.selPatch]))
  if (!points.length && m && nav.route) points.push(...nav.route.nodes.filter((n) => n < m.nodes).map((n) => m.origin(n)))
  if (!points.length) return null
  const box = new THREE.Box3().setFromPoints(points.map((p) => new THREE.Vector3(...p)))
  return box.expandByScalar(48)
})

onMounted(() => {
  viewer = new Viewer(
    host.value!,
    {
      click: (p) => nav.click(p),
      hover: (p) => nav.onHover(p),
      camera: (p) => (store.camera = p),
      grab: (p) => nav.grab(p),
      moveStart: (n) => nav.moveStart(n),
      moveLive: (n, to) => nav.moveLive(n, to),
      drop: (n, to) => nav.moveEnd(n, to),
    },
    store.layers,
  )
  if (import.meta.env.DEV) {
    ;(window as unknown as { viewer: Viewer }).viewer = viewer
  }
  void load()
  syncNav()
  viewer.setSnap(nav.snapStep)
  viewer.setGizmo(gizmo.value)
})

onBeforeUnmount(() => viewer?.dispose())

watch(
  () => store.manifest,
  (m) => {
    void load()
    if (m && m.map !== nav.map) void nav.load(m.map)
  },
)
watch(() => store.selected, (s) => viewer?.select(s))
watch(() => ({ ...store.layers }), (l) => viewer?.setLayers(l))
watch(
  () => store.view,
  (v) => {
    store.slice = null
    viewer?.setView(v)
  },
)
watch(() => store.slice, (s) => viewer?.setSlice(s))
watch(() => store.brightness, (b) => viewer?.setBrightness(b))
watch(() => store.wireframe, (w) => viewer?.setWireframe(w))

watch(() => nav.show, (s) => viewer?.nav.setVisible(s))
watch(
  [() => nav.model, () => nav.forbidden, () => nav.hiddenKinds, () => nav.hideOff, () => nav.focusNodes, () => !gizmo.value],
  ([m, f, hidden, off, focus, arrows]) => {
    viewer?.nav.setGraph(m, f, new Set(hidden), off, focus, arrows)
    // What is lit up and marked stands where the nodes stand now.
    viewer?.nav.setProblems(nav.problems)
    viewer?.nav.setHighlight(highlight.value)
  },
)
watch(
  () => nav.problems,
  (p) => viewer?.nav.setProblems(p),
)
watch(
  () => nav.problemsShown,
  (on) => viewer?.nav.setProblemsVisible(on),
)
watch([() => nav.draft, () => nav.info], () => viewer?.nav.setMarkup(nav.draft, nav.info?.overlay.file ?? null))
watch(highlight, (h) => viewer?.nav.setHighlight(h))
watch(focusBox, (b) => viewer?.setFocusTarget(b))
watch(
  () => nav.focusRequest,
  () => {
    if (focusBox.value) viewer?.show(focusBox.value)
  },
)
watch(gizmo, (g) => viewer?.setGizmo(g))
watch(
  () => nav.snapStep,
  (s) => viewer?.setSnap(s),
)
watch(
  () => nav.ghost,
  (g) => {
    const m = nav.model
    viewer?.nav.setDrag(g && m && g.node < m.nodes ? { from: m.origin(g.node), to: g.to, marker: true } : null)
  },
)
watch(
  () => nav.revealRequest,
  () => {
    if (focusBox.value) viewer?.reveal(focusBox.value)
  },
)
</script>

<template>
  <div ref="host" class="viewport" :class="`tool-${nav.tool}`">
    <div v-if="store.loading" class="overlay">Loading {{ store.loading }}…</div>
    <ToolBar />
    <ToolOptions />
    <Legend />
    <Toast />
  </div>
</template>

<style scoped>
.viewport {
  position: relative;
  min-width: 0;
  min-height: 0;
  overflow: hidden;
}
.viewport :deep(canvas) {
  display: block;
  outline: none;
}
.viewport:not(.tool-select) :deep(canvas) {
  cursor: crosshair;
}
.viewport.tool-move :deep(canvas) {
  cursor: grab;
}
.overlay {
  position: absolute;
  inset: 0;
  display: grid;
  place-items: center;
  color: var(--muted);
  background: rgb(21 23 27 / 0.6);
  pointer-events: none;
}
</style>
