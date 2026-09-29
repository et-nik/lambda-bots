import { defineStore } from 'pinia'
import { markRaw } from 'vue'

import { ApiError, applyOnServer, navInfo, navPreview, navRoute, saveEditor } from '../api'
import type { NavInfo, OnDisk, OverlayFile, PatchOp, Pick, Preview, Route, Tool, Vec3 } from '../types'
import { GraphModel } from '../viewer/graph'
import { forbiddenNodes } from '../viewer/navlayer'
import { useEditor } from './editor'

/** A player's origin is this far over the floor clicked. */
const STAND = 36
/** Milliseconds a preview waits for more changes. */
const PREVIEW_DELAY = 120

export type LinkKindChoice = 'auto' | 'jump' | 'crouch' | 'longjump' | 'gauss_boost'

export const LINK_KIND_CHOICES: { id: LinkKindChoice; label: string }[] = [
  { id: 'auto', label: 'as it checks out' },
  { id: 'jump', label: 'jump' },
  { id: 'crouch', label: 'crouch' },
  { id: 'longjump', label: 'long jump' },
  { id: 'gauss_boost', label: 'gauss boost' },
]

function emptyFile(map: string): OverlayFile {
  return { schema: 'lambdabots/overlay@1', map }
}

/** The file as the server writes it: no empty lists. */
function normal(f: OverlayFile): OverlayFile {
  const out: OverlayFile = { schema: f.schema, map: f.map }
  if (f.bsp_size !== undefined) out.bsp_size = f.bsp_size
  if (f.places?.length) out.places = f.places
  if (f.nav?.patches?.length) out.nav = { patches: f.nav.patches }
  return out
}

function text(f: OverlayFile | null): string {
  return f ? JSON.stringify(normal(f)) : ''
}

function copy(f: OverlayFile): OverlayFile {
  return JSON.parse(JSON.stringify(f))
}

function up(p: Vec3): Vec3 {
  return [Math.round(p[0]), Math.round(p[1]), Math.round(p[2] + STAND)]
}

function at(p: Vec3): Vec3 {
  return [Math.round(p[0]), Math.round(p[1]), Math.round(p[2])]
}

const draftKey = (map: string) => `lb-editor:draft:${map}`

function storedDraft(map: string, version: string): OverlayFile | null {
  try {
    const raw = localStorage.getItem(draftKey(map))
    const d = raw ? (JSON.parse(raw) as { version: string; file: OverlayFile }) : null
    return d && d.version === version ? d.file : null
  } catch {
    return null
  }
}

function storeDraft(map: string, version: string, file: OverlayFile | null) {
  try {
    if (file) {
      localStorage.setItem(draftKey(map), JSON.stringify({ version, file }))
    } else {
      localStorage.removeItem(draftKey(map))
    }
  } catch {
    // A private window or blocked storage: the draft lives only in the page.
  }
}

let previewTimer = 0
let previewSeq = 0

export const useNav = defineStore('nav', {
  state: () => ({
    map: null as string | null,
    info: null as NavInfo | null,
    loading: false,
    error: null as string | null,
    /** `editor.yaml` as the page changes it. */
    draft: null as OverlayFile | null,
    /** The version on disk the draft was made from. */
    version: 'none',
    /** The draft as last read or saved, to tell unsaved changes. */
    savedText: '',
    history: [] as string[],
    future: [] as string[],
    preview: null as Preview | null,
    tool: 'select' as Tool,
    linkKind: 'auto' as LinkKindChoice,
    linkBoth: true,
    linkTrust: false,
    unlinkBoth: true,
    forbidRadius: 64,
    placeName: 'spot',
    placeRadius: 128,
    placeTags: '',
    routeLongjump: true,
    routeGauss: false,
    /** The first node of a link being drawn. */
    pending: null as number | null,
    routeFrom: null as Vec3 | null,
    route: null as Route | null,
    selNode: null as number | null,
    selLink: null as [number, number] | null,
    selPatch: null as number | null,
    hover: null as { node: number | null; link: [number, number] | null } | null,
    /** The file on disk when a save found it changed. */
    conflict: null as OnDisk | null,
    notice: null as string | null,
    show: true,
    /** Link kinds not drawn, and whether links that are off are hidden. */
    hiddenKinds: [] as string[],
    hideOff: false,
    /** Bumped when the panel asks the view to bring the selection close. */
    focusRequest: 0,
  }),
  getters: {
    model(): GraphModel | null {
      return this.info ? markRaw(new GraphModel(this.info, this.preview)) : null
    },
    forbidden(): Set<number> {
      return forbiddenNodes(this.preview?.editor ?? [], this.draft)
    },
    dirty(): boolean {
      return text(this.draft) !== this.savedText
    },
    patches(): PatchOp[] {
      return this.draft?.nav?.patches ?? []
    },
    places(): NonNullable<OverlayFile['places']> {
      return this.draft?.places ?? []
    },
  },
  actions: {
    async load(map: string) {
      this.map = map
      this.info = null
      this.preview = null
      this.draft = null
      this.error = null
      this.cancel()
      this.clearSelection()
      this.route = null
      this.conflict = null
      this.notice = null
      this.loading = true
      try {
        const info = await navInfo(map)
        if (this.map !== map) return
        this.info = markRaw(info)
        this.take(info.editor)
        const kept = storedDraft(map, info.editor.version)
        if (kept && text(kept) !== this.savedText) {
          this.draft = kept
          this.notice = 'Unsaved changes from the last visit are back.'
        }
        if (info.editor.error) {
          this.error = `editor.yaml does not read: ${info.editor.error}. Saving writes it anew.`
        }
        await this.refresh()
      } catch (e) {
        this.error = e instanceof Error ? e.message : String(e)
      } finally {
        if (this.map === map) this.loading = false
      }
    },

    /** Starts over from the file on disk. */
    take(disk: OnDisk) {
      const file = disk.file ? copy(disk.file) : emptyFile(this.map ?? '')
      this.draft = file
      this.version = disk.version
      this.savedText = text(file)
      this.history = []
      this.future = []
    },

    change(mutate: (f: OverlayFile) => void) {
      if (!this.draft || !this.map) return
      this.history.push(JSON.stringify(this.draft))
      this.future = []
      const next = copy(this.draft)
      mutate(next)
      this.draft = next
      storeDraft(this.map, this.version, this.dirty ? next : null)
      this.schedule()
    },

    undo() {
      const prev = this.history.pop()
      if (prev === undefined || !this.draft) return
      this.future.push(JSON.stringify(this.draft))
      this.draft = JSON.parse(prev)
      this.selPatch = null
      if (this.map) storeDraft(this.map, this.version, this.dirty ? this.draft : null)
      this.schedule()
    },

    redo() {
      const next = this.future.pop()
      if (next === undefined || !this.draft) return
      this.history.push(JSON.stringify(this.draft))
      this.draft = JSON.parse(next)
      if (this.map) storeDraft(this.map, this.version, this.dirty ? this.draft : null)
      this.schedule()
    },

    schedule() {
      clearTimeout(previewTimer)
      previewTimer = window.setTimeout(() => void this.refresh(), PREVIEW_DELAY)
    },

    async refresh() {
      if (!this.map || !this.draft) return
      const seq = ++previewSeq
      try {
        const p = await navPreview(this.map, normal(this.draft))
        if (seq === previewSeq) {
          this.preview = markRaw(p)
          this.error = null
        }
      } catch (e) {
        if (seq === previewSeq) this.error = e instanceof Error ? e.message : String(e)
      }
    },

    addPatch(p: PatchOp) {
      this.change((f) => {
        f.nav = { patches: [...(f.nav?.patches ?? []), p] }
      })
      this.selNode = null
      this.selLink = null
      this.selPatch = this.patches.length - 1
    },

    toggleKind(kind: string) {
      this.hiddenKinds = this.hiddenKinds.includes(kind)
        ? this.hiddenKinds.filter((k) => k !== kind)
        : [...this.hiddenKinds, kind]
    },

    removePatch(i: number) {
      this.change((f) => {
        f.nav?.patches?.splice(i, 1)
      })
      this.selPatch = null
    },

    updatePatch(i: number, p: PatchOp) {
      this.change((f) => {
        if (f.nav?.patches) f.nav.patches[i] = p
      })
    },

    removePlace(i: number) {
      this.change((f) => {
        f.places?.splice(i, 1)
      })
    },

    linkNodes(a: number, b: number) {
      const m = this.model
      if (!m) return
      const kind = this.linkKind === 'auto' ? undefined : this.linkKind
      this.addPatch({
        op: 'add_link',
        from: at(m.origin(a)),
        to: at(m.origin(b)),
        ...(kind ? { kind } : {}),
        both: this.linkBoth,
        trust: this.linkTrust,
      })
    },

    unlink(link: [number, number]) {
      const m = this.model
      if (!m) return
      const [a, b] = link
      const both = this.unlinkBoth && m.find(b, a) !== undefined
      this.addPatch({ op: 'remove_link', from: at(m.origin(a)), to: at(m.origin(b)), both })
      this.selLink = null
    },

    async runRoute(from: Vec3, to: Vec3) {
      if (!this.map || !this.draft) return
      try {
        this.route = markRaw(await navRoute(this.map, normal(this.draft), from, to, this.routeLongjump, this.routeGauss))
        this.notice = null
      } catch (e) {
        this.route = null
        this.notice = e instanceof ApiError ? e.message : String(e)
      }
    },

    click(p: Pick) {
      const editor = useEditor()
      const m = this.model
      this.notice = null
      switch (this.tool) {
        case 'select':
          if (p.node !== null) {
            this.selectNode(p.node)
            editor.select(null)
          } else if (p.link) {
            this.selectLink(p.link)
            editor.select(null)
          } else {
            this.clearSelection()
            editor.select(p.entity)
          }
          return
        case 'link':
          if (p.node === null) {
            this.notice = 'Click a node.'
          } else if (this.pending === null || this.pending === p.node) {
            this.pending = this.pending === p.node ? null : p.node
          } else {
            this.linkNodes(this.pending, p.node)
            this.pending = p.node
          }
          return
        case 'unlink':
          if (p.link) this.unlink(p.link)
          else this.notice = 'Click a link.'
          return
        case 'node':
          if (p.point) this.addPatch({ op: 'add_node', at: up(p.point) })
          return
        case 'forbid':
          if (p.point) this.addPatch({ op: 'forbid', at: up(p.point), radius: this.forbidRadius })
          return
        case 'place': {
          if (!p.point) return
          const taken = new Set(this.places.map((pl) => pl.name))
          let name = this.placeName.trim() || 'spot'
          for (let k = 2; taken.has(name); k++) {
            name = `${this.placeName.trim() || 'spot'}-${k}`
          }
          const tags = this.placeTags
            .split(/[\s,]+/)
            .map((t) => t.trim())
            .filter(Boolean)
          this.change((f) => {
            f.places = [
              ...(f.places ?? []),
              { name, at: up(p.point!), radius: this.placeRadius, ...(tags.length ? { tags } : {}) },
            ]
          })
          return
        }
        case 'route': {
          const point = p.node !== null && m ? m.origin(p.node) : p.point ? up(p.point) : null
          if (!point) return
          if (this.routeFrom === null) {
            this.routeFrom = point
            this.route = null
          } else {
            const from = this.routeFrom
            this.routeFrom = null
            void this.runRoute(from, point)
          }
          return
        }
      }
    },

    onHover(p: Pick | null) {
      const wanted = this.tool === 'select' || this.tool === 'link' || this.tool === 'unlink'
      const h = wanted && p ? { node: p.node, link: this.tool === 'link' ? null : p.link } : null
      const same =
        (h === null && this.hover === null) ||
        (h !== null &&
          this.hover !== null &&
          h.node === this.hover.node &&
          String(h.link) === String(this.hover.link))
      if (!same) this.hover = h
    },

    selectNode(n: number) {
      this.selNode = n
      this.selLink = null
      this.selPatch = null
    },

    selectLink(l: [number, number]) {
      this.selLink = l
      this.selNode = null
      this.selPatch = null
    },

    clearSelection() {
      this.selNode = null
      this.selLink = null
      this.selPatch = null
    },

    setTool(t: Tool) {
      this.tool = t
      this.cancel()
    },

    /** Drops a link or a route half drawn. */
    cancel() {
      this.pending = null
      this.routeFrom = null
    },

    async save(overwrite = false) {
      if (!this.map || !this.draft) return
      const base = overwrite && this.conflict ? this.conflict.version : this.version
      try {
        const r = await saveEditor(this.map, normal(this.draft), base)
        if (!r.saved) {
          this.conflict = r.now
          return
        }
        this.version = r.version
        this.savedText = text(this.draft)
        this.conflict = null
        storeDraft(this.map, this.version, null)
        this.notice = this.info?.apply.available
          ? 'Saved. “Apply on server” makes the server read it.'
          : 'Saved. The server takes it with `lb overlay reload`.'
      } catch (e) {
        this.notice = e instanceof Error ? e.message : String(e)
      }
    },

    /** Drops the page's changes for the file on disk the save found. */
    takeTheirs() {
      if (!this.conflict || !this.map) return
      this.take(this.conflict)
      this.conflict = null
      storeDraft(this.map, this.version, null)
      void this.refresh()
    },

    async apply() {
      if (!this.map) return
      try {
        const r = await applyOnServer(this.map)
        this.notice = `Sent “${r.sent}” to ${r.to}: the server reads the overlays of the map it is on; its console shows what it did.`
      } catch (e) {
        this.notice = e instanceof Error ? e.message : String(e)
      }
    },
  },
})
