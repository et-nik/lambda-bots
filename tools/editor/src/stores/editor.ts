import { defineStore } from 'pinia'

import { ApiError, listMaps, manifest } from '../api'
import type { Layer, Manifest, MapFile, View } from '../types'

export const LAYERS: { id: Layer; label: string; shown: boolean }[] = [
  { id: 'world', label: 'World', shown: true },
  { id: 'movers', label: 'Doors, lifts, buttons', shown: true },
  { id: 'breakables', label: 'Breakables', shown: true },
  { id: 'walls', label: 'Walls', shown: true },
  { id: 'water', label: 'Water', shown: true },
  { id: 'points', label: 'Point entities', shown: true },
  { id: 'ladders', label: 'Ladders', shown: false },
  { id: 'triggers', label: 'Triggers', shown: false },
  { id: 'lights', label: 'Lights', shown: false },
  { id: 'sky', label: 'Sky', shown: false },
]

function mapFromHash(): string | null {
  const m = /(?:^#|&)map=([^&]+)/.exec(location.hash)
  return m ? decodeURIComponent(m[1]) : null
}

export const useEditor = defineStore('editor', {
  state: () => ({
    maps: [] as MapFile[],
    manifest: null as Manifest | null,
    loading: null as string | null,
    error: null as string | null,
    selected: null as number | null,
    layers: Object.fromEntries(LAYERS.map((l) => [l.id, l.shown])) as Record<Layer, boolean>,
    view: '3d' as View,
    brightness: 1.3,
    wireframe: false,
    /** Where the 2D views cut the map, along the axis they look down; null: not cut. */
    slice: null as number | null,
    camera: [0, 0, 0] as [number, number, number],
  }),
  getters: {
    entity(state) {
      return state.selected === null ? null : (state.manifest?.entities[state.selected] ?? null)
    },
  },
  actions: {
    async start() {
      try {
        this.maps = await listMaps()
      } catch (e) {
        this.error = e instanceof ApiError && e.status === 403 ? e.message : String(e)
        return
      }
      const wanted = mapFromHash()
      const first = this.maps.find((m) => m.name === wanted) ?? this.maps.find((m) => m.name === 'crossfire')
      if (first) {
        await this.open(first.name)
      }
    },
    async open(name: string) {
      this.loading = name
      this.error = null
      this.selected = null
      try {
        this.manifest = await manifest(name)
        history.replaceState(null, '', `#map=${encodeURIComponent(name)}`)
      } catch (e) {
        this.error = `${name}: ${e instanceof Error ? e.message : String(e)}`
        this.loading = null
      }
    },
    select(entity: number | null) {
      this.selected = entity
    },
  },
})
