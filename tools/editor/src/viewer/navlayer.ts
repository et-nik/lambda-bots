import * as THREE from 'three'

import type { Outcome, OverlayFile, Place, Route, Vec3 } from '../types'
import { GraphModel, KIND_COLORS, type Link, linkValid, nodeColor, OFF_COLOR } from './graph'

/** Pixels from the pointer a node or a link is picked within. */
const PICK_NODE = 10
const PICK_LINK = 6
/** A node this much behind the map surface under the pointer still counts as seen (units). */
const SEEN_BEHIND = 48
const NODE_SIZE = 5
const INVALID = OFF_COLOR
const ADDED_NODE = 0xff4df0
const FORBIDDEN = 0x7a1010
const SELECT = 0xffd24a
const PENDING = 0xff4df0
const ROUTE = 0x3ae0ff

export interface Highlight {
  node?: number | null
  link?: [number, number] | null
  hover?: { node: number | null; link: [number, number] | null } | null
  /** A link being drawn: its first node. */
  pending?: number | null
  /** A route being picked: its first point. */
  routeFrom?: Vec3 | null
  /** Nodes and links a selected patch touched. */
  patch?: { nodes: number[]; links: [number, number][] } | null
  route?: Route | null
}

function segmentDistance(px: number, py: number, ax: number, ay: number, bx: number, by: number): number {
  const dx = bx - ax
  const dy = by - ay
  const len = dx * dx + dy * dy
  const t = len > 0 ? Math.max(0, Math.min(1, ((px - ax) * dx + (py - ay) * dy) / len)) : 0
  return Math.hypot(px - (ax + t * dx), py - (ay + t * dy))
}

function label(text: string, color: string): THREE.Sprite {
  const canvas = document.createElement('canvas')
  const ctx = canvas.getContext('2d')!
  const font = '600 28px system-ui, sans-serif'
  ctx.font = font
  canvas.width = Math.ceil(ctx.measureText(text).width) + 16
  canvas.height = 40
  ctx.font = font
  ctx.fillStyle = 'rgba(15, 17, 21, 0.75)'
  ctx.fillRect(0, 0, canvas.width, canvas.height)
  ctx.fillStyle = color
  ctx.textBaseline = 'middle'
  ctx.fillText(text, 8, 21)
  const tex = new THREE.CanvasTexture(canvas)
  const sprite = new THREE.Sprite(new THREE.SpriteMaterial({ map: tex, depthTest: false, transparent: true }))
  sprite.scale.set(canvas.width * 0.5, canvas.height * 0.5, 1)
  sprite.renderOrder = 20
  return sprite
}

function tube(points: THREE.Vector3[], radius: number, color: number, opacity = 1): THREE.Mesh {
  const path = new THREE.CurvePath<THREE.Vector3>()
  for (let i = 1; i < points.length; i++) {
    path.add(new THREE.LineCurve3(points[i - 1], points[i]))
  }
  const geo = new THREE.TubeGeometry(path, Math.max(1, (points.length - 1) * 2), radius, 6, false)
  const mesh = new THREE.Mesh(
    geo,
    new THREE.MeshBasicMaterial({ color, depthTest: false, transparent: true, opacity }),
  )
  mesh.renderOrder = 15
  return mesh
}

function ring(at: THREE.Vector3, radius: number, color: number): THREE.LineLoop {
  const pts: THREE.Vector3[] = []
  for (let i = 0; i < 48; i++) {
    const a = (i / 48) * Math.PI * 2
    pts.push(new THREE.Vector3(at.x + Math.cos(a) * radius, at.y + Math.sin(a) * radius, at.z))
  }
  const loop = new THREE.LineLoop(
    new THREE.BufferGeometry().setFromPoints(pts),
    new THREE.LineBasicMaterial({ color, depthTest: false, transparent: true }),
  )
  loop.renderOrder = 12
  return loop
}

function disposeTree(o: THREE.Object3D) {
  o.traverse((c) => {
    const m = c as THREE.Mesh
    m.geometry?.dispose()
    const mats = Array.isArray(m.material) ? m.material : m.material ? [m.material] : []
    for (const mat of mats) {
      ;(mat as THREE.SpriteMaterial).map?.dispose()
      mat.dispose()
    }
  })
}

/** The navigation graph drawn over the map, the overlays' zones and places, and what is selected. */
export class NavLayer {
  readonly group = new THREE.Group()
  model: GraphModel | null = null
  private graph = new THREE.Group()
  private markup = new THREE.Group()
  private marks = new THREE.Group()
  /** Links drawn (and picked): kinds shown, and whether links that are off are. */
  private shown: (l: Link) => boolean = () => true

  constructor(scene: THREE.Scene) {
    this.group.add(this.graph, this.markup, this.marks)
    scene.add(this.group)
  }

  get visible(): boolean {
    return this.group.visible
  }

  setVisible(on: boolean) {
    this.group.visible = on
  }

  private clear(g: THREE.Group) {
    for (const c of [...g.children]) {
      g.remove(c)
      disposeTree(c)
    }
  }

  setGraph(model: GraphModel | null, forbidden: Set<number>, hiddenKinds: Set<string> = new Set(), hideOff = false) {
    this.clear(this.graph)
    this.model = model
    this.shown = (l) => !hiddenKinds.has(l.kind) && (!hideOff || linkValid(l))
    if (!model) {
      return
    }
    const color = new THREE.Color()
    const mesh = new THREE.InstancedMesh(
      new THREE.OctahedronGeometry(NODE_SIZE),
      new THREE.MeshBasicMaterial(),
      model.nodes,
    )
    const m = new THREE.Matrix4()
    for (let n = 0; n < model.nodes; n++) {
      m.makeTranslation(model.pos[n * 3], model.pos[n * 3 + 1], model.pos[n * 3 + 2])
      mesh.setMatrixAt(n, m)
      const c = forbidden.has(n) ? FORBIDDEN : n >= model.baseNodes ? ADDED_NODE : nodeColor(model.flags[n])
      mesh.setColorAt(n, color.setHex(c))
    }
    mesh.instanceMatrix.needsUpdate = true
    if (mesh.instanceColor) {
      mesh.instanceColor.needsUpdate = true
    }
    this.graph.add(mesh)

    const lines = (links: Link[], colorOf: (l: Link) => number, over: boolean) => {
      const positions = new Float32Array(links.length * 6)
      const colors = new Float32Array(links.length * 6)
      links.forEach((l, i) => {
        positions.set(model.origin(l.from), i * 6)
        positions.set(model.origin(l.to), i * 6 + 3)
        color.setHex(colorOf(l))
        // Dim at the start, bright at the end: which way a link goes.
        colors.set([color.r * 0.35, color.g * 0.35, color.b * 0.35, color.r, color.g, color.b], i * 6)
      })
      const geo = new THREE.BufferGeometry()
      geo.setAttribute('position', new THREE.BufferAttribute(positions, 3))
      geo.setAttribute('color', new THREE.BufferAttribute(colors, 3))
      const seg = new THREE.LineSegments(
        geo,
        new THREE.LineBasicMaterial({ vertexColors: true, depthTest: !over, transparent: over }),
      )
      seg.renderOrder = over ? 11 : 0
      return seg
    }
    const plainColor = (l: Link) => (linkValid(l) ? (KIND_COLORS[l.kind] ?? 0xffffff) : INVALID)
    const drawn = model.links.filter(this.shown)
    this.graph.add(lines(drawn.filter((l) => !l.added), plainColor, false))
    // What the overlays change is drawn through walls.
    const added = drawn.filter((l) => l.added)
    if (added.length) {
      this.graph.add(lines(added, plainColor, true))
    }
    if (model.removed.length) {
      const positions = new Float32Array(model.removed.length * 6)
      model.removed.forEach(([a, b], i) => {
        positions.set(model.origin(a), i * 6)
        positions.set(model.origin(b), i * 6 + 3)
      })
      const geo = new THREE.BufferGeometry()
      geo.setAttribute('position', new THREE.BufferAttribute(positions, 3))
      const dashed = new THREE.LineSegments(
        geo,
        new THREE.LineDashedMaterial({ color: INVALID, dashSize: 8, gapSize: 6, depthTest: false, transparent: true }),
      )
      dashed.computeLineDistances()
      dashed.renderOrder = 11
      this.graph.add(dashed)
    }
  }

  /** Forbidden zones and places of the editor file (bright) and the hand-written overlay (dim). */
  setMarkup(editor: OverlayFile | null, overlay: OverlayFile | null) {
    this.clear(this.markup)
    const draw = (file: OverlayFile | null, dim: boolean) => {
      for (const p of file?.nav?.patches ?? []) {
        if (p.op !== 'forbid') {
          continue
        }
        const at = new THREE.Vector3(...p.at)
        const sphere = new THREE.Mesh(
          new THREE.SphereGeometry(p.radius, 24, 12),
          new THREE.MeshBasicMaterial({ color: 0xff3030, transparent: true, opacity: dim ? 0.06 : 0.14, depthWrite: false }),
        )
        sphere.position.copy(at)
        this.markup.add(sphere, ring(at, p.radius, dim ? 0x803030 : 0xff5050))
      }
      for (const place of file?.places ?? []) {
        this.markup.add(this.place(place, dim))
      }
    }
    draw(overlay, true)
    draw(editor, false)
  }

  private place(p: Place, dim: boolean): THREE.Object3D {
    const at = new THREE.Vector3(...p.at)
    const g = new THREE.Group()
    g.add(ring(at.clone().setZ(at.z - 30), p.radius, dim ? 0x3a5a80 : 0x5aa0ff))
    const tag = label(p.tags?.length ? `${p.name} · ${p.tags.join(', ')}` : p.name, dim ? '#7fa0c8' : '#9fd0ff')
    tag.position.copy(at).setZ(at.z + 24)
    g.add(tag)
    return g
  }

  setHighlight(h: Highlight) {
    this.clear(this.marks)
    const model = this.model
    if (!model) {
      return
    }
    const at = (n: number) => new THREE.Vector3(...model.origin(n))
    const ball = (n: number, color: number, radius: number) => {
      const s = new THREE.Mesh(
        new THREE.SphereGeometry(radius, 12, 8),
        new THREE.MeshBasicMaterial({ color, wireframe: true, depthTest: false, transparent: true }),
      )
      s.position.copy(at(n))
      s.renderOrder = 16
      this.marks.add(s)
    }
    const arrow = (a: number, b: number, color: number, radius: number) => {
      if (a >= model.nodes || b >= model.nodes) {
        return
      }
      const [pa, pb] = [at(a), at(b)]
      this.marks.add(tube([pa, pb], radius, color, 0.9))
      const cone = new THREE.Mesh(
        new THREE.ConeGeometry(radius * 3, radius * 8, 8),
        new THREE.MeshBasicMaterial({ color, depthTest: false, transparent: true }),
      )
      const dir = pb.clone().sub(pa).normalize()
      cone.position.copy(pb).addScaledVector(dir, -radius * 4 - NODE_SIZE)
      cone.quaternion.setFromUnitVectors(new THREE.Vector3(0, 1, 0), dir)
      cone.renderOrder = 16
      this.marks.add(cone)
    }
    if (h.route && h.route.nodes.length > 1) {
      this.marks.add(tube(h.route.nodes.filter((n) => n < model.nodes).map(at), 3, ROUTE, 0.85))
      ball(h.route.start, ROUTE, 12)
      ball(h.route.goal, ROUTE, 12)
    }
    if (h.routeFrom) {
      const s = new THREE.Mesh(
        new THREE.SphereGeometry(10, 12, 8),
        new THREE.MeshBasicMaterial({ color: ROUTE, wireframe: true, depthTest: false, transparent: true }),
      )
      s.position.set(...h.routeFrom)
      this.marks.add(s)
    }
    for (const n of h.patch?.nodes ?? []) {
      if (n < model.nodes) ball(n, SELECT, 9)
    }
    for (const [a, b] of h.patch?.links ?? []) {
      arrow(a, b, SELECT, 1.5)
    }
    if (h.hover?.node != null) ball(h.hover.node, 0xffffff, 9)
    if (h.hover?.link) arrow(h.hover.link[0], h.hover.link[1], 0xffffff, 1.2)
    if (h.pending != null) ball(h.pending, PENDING, 14)
    if (h.node != null && h.node < model.nodes) ball(h.node, SELECT, 14)
    if (h.link) arrow(h.link[0], h.link[1], SELECT, 2.2)
  }

  /**
   * The node or link drawn nearest the pointer (`px`, `py` in the canvas), not further behind the map surface
   * under it than `SEEN_BEHIND` (`surface`: its depth along the view, Infinity for none) nor cut away by `planes`.
   */
  pick(
    px: number,
    py: number,
    camera: THREE.Camera,
    width: number,
    height: number,
    surface: number,
    planes: THREE.Plane[],
  ): { node: number | null; link: [number, number] | null } {
    const model = this.model
    if (!model || !this.group.visible) {
      return { node: null, link: null }
    }
    const forward = new THREE.Vector3()
    camera.getWorldDirection(forward)
    const eye = camera.position
    const v = new THREE.Vector3()
    const screen = new Float32Array(model.nodes * 2)
    const shown = new Uint8Array(model.nodes)
    for (let n = 0; n < model.nodes; n++) {
      v.set(model.pos[n * 3], model.pos[n * 3 + 1], model.pos[n * 3 + 2])
      if (planes.some((p) => p.distanceToPoint(v) < 0)) {
        continue
      }
      const depth = v.clone().sub(eye).dot(forward)
      if (depth > surface + SEEN_BEHIND) {
        continue
      }
      v.project(camera)
      if (v.z < -1 || v.z > 1) {
        continue
      }
      screen[n * 2] = ((v.x + 1) / 2) * width
      screen[n * 2 + 1] = ((1 - v.y) / 2) * height
      shown[n] = 1
    }
    let node = -1
    let best = PICK_NODE
    for (let n = 0; n < model.nodes; n++) {
      if (!shown[n]) continue
      const d = Math.hypot(screen[n * 2] - px, screen[n * 2 + 1] - py)
      if (d < best) {
        best = d
        node = n
      }
    }
    if (node >= 0) {
      return { node, link: null }
    }
    let link: [number, number] | null = null
    best = PICK_LINK
    for (const l of model.links) {
      if (!shown[l.from] || !shown[l.to] || !this.shown(l)) continue
      const d = segmentDistance(
        px,
        py,
        screen[l.from * 2],
        screen[l.from * 2 + 1],
        screen[l.to * 2],
        screen[l.to * 2 + 1],
      )
      if (d < best) {
        best = d
        link = [l.from, l.to]
      }
    }
    return { node: null, link }
  }

  dispose() {
    for (const g of [this.graph, this.markup, this.marks]) {
      this.clear(g)
    }
    this.group.parent?.remove(this.group)
  }
}

export function forbiddenNodes(outcomes: Outcome[], file: OverlayFile | null): Set<number> {
  const out = new Set<number>()
  const patches = file?.nav?.patches ?? []
  outcomes.forEach((o, i) => {
    if (patches[i]?.op === 'forbid') {
      o.nodes.forEach((n) => out.add(n))
    }
  })
  return out
}
