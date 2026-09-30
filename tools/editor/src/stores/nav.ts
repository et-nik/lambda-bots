import { defineStore } from 'pinia'
import { markRaw } from 'vue'

import { ApiError, applyOnServer, navInfo, navPreview, navRoute, saveEditor } from '../api'
import type { NavInfo, OnDisk, OverlayFile, PatchOp, Pick, Preview, Route, Tool, Vec3 } from '../types'
import { GraphModel, STAND } from '../viewer/graph'
import { forbiddenNodes } from '../viewer/navlayer'
import { useEditor } from './editor'

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

/** Radius of the zone that shuts off one node (nodes stand further apart). */
const SHUT_RADIUS = 16
/** Selections kept for ‹ back. */
const TRAIL = 50

/** A node or a link selected. */
export type Selected = { node: number } | { link: [number, number] }

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
    /** Nodes and links the panel points at (under the pointer there), lit up in the view. */
    peek: null as { nodes: number[]; links: [number, number][] } | null,
    /** Selections before the current one, for ‹ back. */
    trail: [] as Selected[],
    /** The file on disk when a save found it changed. */
    conflict: null as OnDisk | null,
    notice: null as string | null,
    /** Bumped with every notice, so the same words show again. */
    noticeSeq: 0,
    /** A change put in from the panel whose outcome is told once the preview has it. */
    reportPatch: null as number | null,
    show: true,
    /** Link kinds not drawn, and whether links that are off are hidden. */
    hiddenKinds: [] as string[],
    hideOff: false,
    /** Bumped when the panel asks the view to bring the selection close. */
    focusRequest: 0,
    /** Bumped when the panel asks the view to show the selection if it is out of sight. */
    revealRequest: 0,
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
    /** What ‹ goes back to. */
    backLabel(): string | null {
      const prev = this.trail[this.trail.length - 1]
      if (!prev) return null
      return 'node' in prev ? `${prev.node}` : `${prev.link[0]} → ${prev.link[1]}`
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
          this.notify('Unsaved changes from the last visit are back.')
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
          const i = this.reportPatch
          if (i !== null && p.editor[i]) {
            this.reportPatch = null
            this.notify(`Change ${i + 1}: ${p.editor[i].message}`)
          }
        }
      } catch (e) {
        if (seq === previewSeq) this.error = e instanceof Error ? e.message : String(e)
      }
    },

    /**
     * Puts a change in at the end. From a tool it becomes the selection; from the panel (`keep`) the selection stays
     * and a notice tells what the change did.
     */
    addPatch(p: PatchOp, keep = false) {
      this.change((f) => {
        f.nav = { patches: [...(f.nav?.patches ?? []), p] }
      })
      if (keep) {
        this.reportPatch = this.patches.length - 1
        return
      }
      this.selNode = null
      this.selLink = null
      this.selPatch = this.patches.length - 1
    },

    notify(text: string) {
      this.notice = text
      this.noticeSeq++
    },

    /** Takes the links between `a` and `b` out, both ways (those there are). */
    unlinkPair(a: number, b: number) {
      const m = this.model
      if (!m) return
      const there = m.find(a, b) !== undefined
      const back = m.find(b, a) !== undefined
      if (!there && !back) return
      const [from, to] = there ? [a, b] : [b, a]
      this.addPatch({ op: 'remove_link', from: at(m.origin(from)), to: at(m.origin(to)), both: there && back }, true)
    },

    /** Takes the link `a → b` out, and the one back with `both`. */
    unlinkOne(a: number, b: number, both: boolean) {
      const m = this.model
      if (!m) return
      this.addPatch({ op: 'remove_link', from: at(m.origin(a)), to: at(m.origin(b)), both }, true)
    },

    /** Puts the link `a → b` in again, checked as `kind` (or trusted). */
    relink(a: number, b: number, kind: LinkKindChoice, trust: boolean) {
      const m = this.model
      if (!m) return
      this.addPatch(
        {
          op: 'add_link',
          from: at(m.origin(a)),
          to: at(m.origin(b)),
          ...(kind === 'auto' ? {} : { kind }),
          both: false,
          trust,
        },
        true,
      )
    },

    /** A forbidden zone about node `n` alone. */
    shutOff(n: number) {
      const m = this.model
      if (!m) return
      this.addPatch({ op: 'forbid', at: at(m.origin(n)), radius: SHUT_RADIUS }, true)
    },

    routeFromNode(n: number) {
      const m = this.model
      if (!m) return
      this.setTool('route')
      this.routeFrom = at(m.origin(n))
      this.route = null
    },

    /** The last change of the editor file that shut node `n` off. */
    shutBy(n: number): number | null {
      const outcomes = this.preview?.editor ?? []
      for (let i = this.patches.length - 1; i >= 0; i--) {
        if (this.patches[i].op === 'forbid' && outcomes[i]?.nodes.includes(n)) return i
      }
      return null
    },

    /** The last change of the editor file that put the link `a → b` in (a link, or a node put in or moved). */
    linkedBy(a: number, b: number): number | null {
      const outcomes = this.preview?.editor ?? []
      for (let i = this.patches.length - 1; i >= 0; i--) {
        const op = this.patches[i].op
        if (op !== 'remove_link' && op !== 'forbid' && outcomes[i]?.links.some(([x, y]) => x === a && y === b)) {
          return i
        }
      }
      return null
    },

    /** The node or link selected now, kept for ‹ back before another is selected from the panel. */
    remember() {
      const now: Selected | null =
        this.selNode !== null ? { node: this.selNode } : this.selLink ? { link: this.selLink } : null
      if (!now) return
      this.trail.push(now)
      if (this.trail.length > TRAIL) this.trail.shift()
    },

    /** Selects node `n` from the panel: ‹ comes back, and the view shows it if it is out of sight. */
    goNode(n: number) {
      if (this.selNode !== n) {
        this.remember()
        this.selectNode(n)
        useEditor().select(null)
      }
      this.revealRequest++
    },

    goLink(l: [number, number]) {
      if (String(this.selLink) !== String(l)) {
        this.remember()
        this.selectLink(l)
        useEditor().select(null)
      }
      this.revealRequest++
    },

    goPatch(i: number) {
      this.selPatch = i
      this.selNode = null
      this.selLink = null
      useEditor().select(null)
      this.revealRequest++
    },

    back() {
      const prev = this.trail.pop()
      if (!prev) return
      if ('node' in prev) this.selectNode(prev.node)
      else this.selectLink(prev.link)
      this.revealRequest++
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

    /** The last change of the editor file that put node `n` in or moved it (and so says where it stands). */
    placedBy(n: number): number | null {
      const outcomes = this.preview?.editor ?? []
      for (let i = this.patches.length - 1; i >= 0; i--) {
        const op = this.patches[i].op
        if ((op === 'add_node' || op === 'move_node') && outcomes[i]?.nodes[0] === n) return i
      }
      return null
    },

    /** Whether node `n` under a press can be dragged (the Move tool); says why not when it cannot. */
    grab(p: Pick): boolean {
      const m = this.model
      if (this.tool !== 'move' || p.node === null || !m) return false
      const n = p.node
      this.selectNode(n)
      if (this.forbidden.has(n)) {
        this.notify(`Node ${n} is shut off by a forbidden zone: take the zone out to move it.`)
        return false
      }
      if (n >= m.baseNodes && this.placedBy(n) === null) {
        this.notify(`Node ${n} is put in by overlay.yaml: move it there.`)
        return false
      }
      this.notice = null
      return true
    },

    /**
     * Sets node `n` down at `to`. The change that put it in or moved it last takes the new spot, and the links
     * changed after it that name the node follow it; a node the changes have not placed gets a move.
     */
    moveNode(n: number, to: Vec3) {
      const m = this.model
      if (!m) return
      const spot = at(to)
      const i = this.placedBy(n)
      if (i === null) {
        this.addPatch({ op: 'move_node', from: at(m.baseOrigin(n)), to: spot })
        return
      }
      const outcomes = this.preview?.editor ?? []
      this.change((f) => {
        const list = f.nav?.patches ?? []
        const p = list[i]
        if (p.op === 'add_node') list[i] = { ...p, at: spot }
        else if (p.op === 'move_node') list[i] = { ...p, to: spot }
        for (let j = i + 1; j < list.length; j++) {
          const q = list[j]
          const ends = outcomes[j]?.nodes ?? []
          if ((q.op === 'add_link' || q.op === 'remove_link') && ends.length === 2) {
            if (ends[0] === n) q.from = spot
            if (ends[1] === n) q.to = spot
          }
        }
      })
      this.selNode = null
      this.selLink = null
      this.selPatch = i
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
        this.notify(e instanceof ApiError ? e.message : String(e))
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
            this.notify('Click a node.')
          } else if (this.pending === null || this.pending === p.node) {
            this.pending = this.pending === p.node ? null : p.node
          } else {
            this.linkNodes(this.pending, p.node)
            this.pending = p.node
          }
          return
        case 'unlink':
          if (p.link) this.unlink(p.link)
          else this.notify('Click a link.')
          return
        case 'node':
          if (p.point) this.addPatch({ op: 'add_node', at: up(p.point) })
          return
        case 'move':
          if (p.node !== null) {
            this.selectNode(p.node)
            editor.select(null)
          } else {
            this.clearSelection()
          }
          return
        case 'forbid': {
          // Clicked on a node: the zone is about it, not about the floor behind it.
          const point = p.node !== null && m ? at(m.origin(p.node)) : p.point ? up(p.point) : null
          if (point) this.addPatch({ op: 'forbid', at: point, radius: this.forbidRadius })
          return
        }
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
      const wanted = this.tool === 'select' || this.tool === 'link' || this.tool === 'unlink' || this.tool === 'move'
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
      this.trail = []
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
      const map = this.map
      const sent = normal(this.draft)
      const sentText = text(this.draft)
      const base = overwrite && this.conflict ? this.conflict.version : this.version
      try {
        const r = await saveEditor(map, sent, base)
        if (this.map !== map) return
        if (!r.saved) {
          this.conflict = r.now
          return
        }
        this.version = r.version
        this.savedText = sentText
        this.conflict = null
        // Changes made while it was saving are not saved: they stay in the browser.
        storeDraft(map, this.version, this.dirty ? this.draft : null)
        const saved = r.graph.written
          ? 'Saved editor.yaml and the graph with the changes (editor.lbnav).'
          : `Saved editor.yaml; no editor.lbnav: ${r.graph.detail}.`
        this.notify(
          this.info?.apply.available
            ? `${saved} “Apply on server” makes the server read them.`
            : `${saved} The server takes them with \`lb overlay reload\`.`,
        )
      } catch (e) {
        this.notify(e instanceof Error ? e.message : String(e))
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
        this.notify(`Sent “${r.sent}” to ${r.to}: the server reads the overlays of the map it is on; its console shows what it did.`)
      } catch (e) {
        this.notify(e instanceof Error ? e.message : String(e))
      }
    },
  },
})
