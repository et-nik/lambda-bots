import * as THREE from 'three'

import type { Layer, Manifest, Pick, Vec3, View } from '../types'
import { FlyControls, Keys, OrthoControls, viewAxes } from './controls'
import { AXIS, closestOnAxis, type Handle, MoveGizmo, NORMAL, onPlane } from './gizmo'
import { STAND } from './graph'
import { grid } from './grid'
import { pointMarkers } from './markers'
import { MapMaterials } from './materials'
import { NavLayer } from './navlayer'

export interface ViewerEvents {
  /** A click (not a drag) with the left button. */
  click(pick: Pick): void
  /** What is under the pointer while it moves over the map without buttons; null when it leaves. */
  hover(pick: Pick | null): void
  camera(position: [number, number, number]): void
  /** Whether pressing the left button on a node there picks it up to drag. */
  grab(pick: Pick): boolean
  /** Whether the node the gizmo stands on may be moved (a press on one of its handles). */
  moveStart(node: number): boolean
  /** Where the node dragged would stand now. */
  moveLive(node: number, to: Vec3): void
  /** A node dragged is let go where it should stand, or back where it was (`null`: a click, or Esc). */
  drop(node: number, to: Vec3 | null): void
}

/** Milliseconds between hover picks, and between the spots a node dragged is shown at. */
const HOVER_EVERY = 50
const DRAG_EVERY = 16

/** How near the edge of the view (in normalized coordinates) a revealed thing may already be. */
const REVEAL_EDGE = 0.85
/** Nearer than this, the 3D camera turns to what it reveals instead of flying to it (units). */
const REVEAL_TURN = 1500

/** Grid lines of a 2D view are this far apart on screen at least, and this many across the map at most. */
const GRID_PX = 8
const GRID_LINES = 2000

/** Pixels the pointer may move between press and release for a click that selects. */
const CLICK_SLOP = 4
const EYE = 28
/** How far behind the first spawn the camera starts, out of the spawn's box. */
const BEHIND = 128

export class Viewer {
  private renderer: THREE.WebGLRenderer
  private scene = new THREE.Scene()
  private perspective = new THREE.PerspectiveCamera(75, 1, 2, 65536)
  private orthographic = new THREE.OrthographicCamera(-1, 1, 1, -1, 1, 65536)
  private keys = new Keys()
  private fly: FlyControls
  private flat: OrthoControls
  private view: View = '3d'
  private manifest: Manifest | null = null
  private map = new THREE.Group()
  private points: THREE.Group | null = null
  private gridLines: THREE.LineSegments | null = null
  private highlight: THREE.Box3Helper | null = null
  private materials: MapMaterials | null = null
  private geometries: THREE.BufferGeometry[] = []
  private layers: Record<Layer, boolean>
  private slice: number | null = null
  private selected: number | null = null
  private raycaster = new THREE.Raycaster()
  private pressed: { x: number; y: number } | null = null
  /**
   * A node being moved: by a handle of the gizmo, or by itself (`free`); where it stood, the point on the axis (`t0`)
   * or plane (`grab`) the press took, where the press was, and where the node would stand now.
   */
  private dragging: {
    node: number
    handle: Handle
    start: THREE.Vector3
    t0: number
    grab: THREE.Vector3 | null
    x: number
    y: number
    to: Vec3 | null
  } | null = null
  private dragged = 0
  private gizmo: MoveGizmo
  /** The node the gizmo stands on, and where. */
  private gizmoNode: number | null = null
  private gizmoAt: THREE.Vector3 | null = null
  /** Grid step moves snap to, and the step the 2D grid is drawn at. */
  private snap = 8
  private gridStep = 0
  private raf = 0
  private last = performance.now()
  private reported = 0
  private resizer: ResizeObserver
  private hovered = 0
  private focusTarget: THREE.Box3 | null = null
  readonly nav: NavLayer

  constructor(
    private el: HTMLElement,
    private events: ViewerEvents,
    layers: Record<Layer, boolean>,
  ) {
    this.layers = { ...layers }
    this.renderer = new THREE.WebGLRenderer({ antialias: true })
    this.renderer.setPixelRatio(window.devicePixelRatio)
    this.renderer.outputColorSpace = THREE.LinearSRGBColorSpace
    this.scene.background = new THREE.Color(0x15171b)
    this.scene.add(this.map)
    this.nav = new NavLayer(this.scene)
    this.gizmo = new MoveGizmo(this.scene)
    const canvas = this.renderer.domElement
    canvas.tabIndex = 0
    el.appendChild(canvas)
    this.fly = new FlyControls(this.perspective, canvas, this.keys)
    this.flat = new OrthoControls(this.orthographic, canvas, this.keys)
    canvas.addEventListener('pointerdown', this.onDown)
    canvas.addEventListener('pointermove', this.onMove)
    canvas.addEventListener('pointerup', this.onUp)
    canvas.addEventListener('wheel', this.onWheel, { passive: false })
    canvas.addEventListener('contextmenu', (e) => e.preventDefault())
    canvas.addEventListener('pointerleave', () => this.events.hover(null))
    window.addEventListener('keydown', this.onKey)
    this.resizer = new ResizeObserver(() => this.resize())
    this.resizer.observe(el)
    this.resize()
    this.tick()
  }

  private camera(): THREE.Camera {
    return this.view === '3d' ? this.perspective : this.orthographic
  }

  private resize() {
    const w = this.el.clientWidth || 1
    const h = this.el.clientHeight || 1
    this.renderer.setSize(w, h)
    this.perspective.aspect = w / h
    this.perspective.updateProjectionMatrix()
    this.flat.resize(w, h)
  }

  private tick = () => {
    this.raf = requestAnimationFrame(this.tick)
    const now = performance.now()
    const dt = Math.min((now - this.last) / 1000, 0.1)
    this.last = now
    if (this.view === '3d') {
      this.fly.update(dt)
    } else {
      this.flat.update(dt)
    }
    if (this.gizmo.visible) {
      this.gizmo.fit(this.camera(), this.el.clientHeight)
    }
    this.nav.fit(this.camera(), this.el.clientWidth, this.el.clientHeight)
    this.renderer.render(this.scene, this.camera())
    if (now - this.reported > 100) {
      this.reported = now
      const p = (this.view === '3d' ? this.perspective : this.orthographic).position
      this.events.camera([Math.round(p.x), Math.round(p.y), Math.round(p.z)])
    }
  }

  load(manifest: Manifest, buffer: ArrayBuffer) {
    this.unload()
    this.manifest = manifest
    const b = manifest.buffers
    const position = new THREE.BufferAttribute(new Float32Array(buffer, b.positions, b.vertices * 3), 3)
    const uv = new THREE.BufferAttribute(new Float32Array(buffer, b.uvs, b.vertices * 2), 2)
    const lightUv = new THREE.BufferAttribute(new Float32Array(buffer, b.light_uvs, b.vertices * 2), 2)
    const index = new THREE.BufferAttribute(new Uint32Array(buffer, b.triangles, b.indices), 1)
    this.materials = new MapMaterials(manifest)
    for (const m of manifest.models) {
      if (m.groups.length === 0) {
        continue
      }
      const g = new THREE.BufferGeometry()
      g.setAttribute('position', position)
      g.setAttribute('uv', uv)
      g.setAttribute('lightUv', lightUv)
      g.setIndex(index)
      m.groups.forEach((group, k) => g.addGroup(group.first, group.count, k))
      g.boundingBox = new THREE.Box3(new THREE.Vector3(...m.mins), new THREE.Vector3(...m.maxs)).expandByScalar(1)
      g.boundingSphere = g.boundingBox.getBoundingSphere(new THREE.Sphere())
      const mesh = new THREE.Mesh(
        g,
        m.groups.map((group) => this.materials!.material(group, m)),
      )
      mesh.position.set(...m.origin)
      mesh.userData = { entity: m.entity }
      this.map.add(mesh)
      this.geometries.push(g)
    }
    this.points = pointMarkers(manifest.entities)
    this.scene.add(this.points)
    this.applyLayers()
    this.startCamera()
    this.setView(this.view)
  }

  private unload() {
    this.map.clear()
    for (const g of this.geometries) {
      g.dispose()
    }
    this.geometries = []
    this.materials?.dispose()
    this.materials = null
    if (this.points) {
      this.scene.remove(this.points)
      this.points.userData.dispose?.()
      this.points = null
    }
    this.setHighlight(null)
    this.selected = null
  }

  private bounds(): [THREE.Vector3, THREE.Vector3] {
    const m = this.manifest
    return m ? [new THREE.Vector3(...m.mins), new THREE.Vector3(...m.maxs)] : [new THREE.Vector3(), new THREE.Vector3()]
  }

  /** Behind the first player spawn, looking the way it faces; else over the middle of the map. */
  private startCamera() {
    const spawn = this.manifest?.entities.find(
      (e) => e.classname === 'info_player_deathmatch' || e.classname === 'info_player_start',
    )
    if (spawn) {
      const yaw = THREE.MathUtils.degToRad(Number(spawn.kv.find(([k]) => k === 'angle')?.[1] ?? 0))
      const back = new THREE.Vector3(Math.cos(yaw), Math.sin(yaw), 0).multiplyScalar(-BEHIND)
      const at = new THREE.Vector3(...spawn.origin).add(back).add(new THREE.Vector3(0, 0, EYE + 32))
      this.fly.place(at, yaw, -0.2)
    } else {
      const [mins, maxs] = this.bounds()
      this.fly.place(mins.clone().add(maxs).multiplyScalar(0.5), 0, -0.3)
    }
  }

  setView(view: View) {
    this.view = view
    this.dropGrid()
    if (view !== '3d') {
      const [mins, maxs] = this.bounds()
      this.flat.frame(view, mins, maxs)
      this.refreshGrid()
    }
    this.gizmo.setView(view)
    this.applySlice()
  }

  /** The 2D view's grid at the snap step, or coarser where its lines would crowd together. */
  private refreshGrid() {
    if (this.view === '3d') return
    const [mins, maxs] = this.bounds()
    const axes = viewAxes(this.view)
    const extent = Math.max(
      maxs.getComponent(axes.across) - mins.getComponent(axes.across),
      maxs.getComponent(axes.up) - mins.getComponent(axes.up),
      1,
    )
    let step = Math.max(this.snap, 1)
    while (step * this.orthographic.zoom < GRID_PX || extent / step > GRID_LINES) step *= 2
    if (this.gridLines && step === this.gridStep) return
    this.dropGrid()
    this.gridStep = step
    this.gridLines = grid(axes, mins, maxs, mins.getComponent(axes.depth) - 64, step)
    this.gridLines.renderOrder = -1
    this.scene.add(this.gridLines)
  }

  private dropGrid() {
    if (this.gridLines) {
      this.scene.remove(this.gridLines)
      this.gridLines.geometry.dispose()
      ;(this.gridLines.material as THREE.Material).dispose()
      this.gridLines = null
    }
  }

  /** The grid step moves snap to. */
  setSnap(step: number) {
    this.snap = step
    this.refreshGrid()
  }

  /** The node the move gizmo stands on (the Move tool's selection), and where; or none. */
  setGizmo(g: { node: number; at: Vec3 } | null) {
    this.gizmoNode = g?.node ?? null
    this.gizmoAt = g ? new THREE.Vector3(...g.at) : null
    if (!this.dragging) this.gizmo.show(this.gizmoAt)
    if (!g) this.gizmo.highlight(null)
  }

  setSlice(at: number | null) {
    this.slice = at
    this.applySlice()
  }

  private applySlice() {
    if (this.view === '3d' || this.slice === null) {
      this.renderer.clippingPlanes = []
      return
    }
    const normal = new THREE.Vector3()
    normal.setComponent(viewAxes(this.view).depth, -1)
    this.renderer.clippingPlanes = [new THREE.Plane(normal, this.slice)]
  }

  setLayers(layers: Record<Layer, boolean>) {
    this.layers = { ...layers }
    this.applyLayers()
  }

  private applyLayers() {
    this.materials?.setLayers(this.layers)
    for (const p of this.points?.children ?? []) {
      p.visible = this.layers[p.userData.layer as Layer] ?? true
    }
  }

  setBrightness(b: number) {
    this.materials?.setBrightness(b)
  }

  setWireframe(on: boolean) {
    this.materials?.setWireframe(on)
  }

  select(entity: number | null) {
    this.selected = entity
    this.setHighlight(entity === null ? null : this.entityBox(entity))
  }

  private entityBox(entity: number): THREE.Box3 | null {
    const e = this.manifest?.entities[entity]
    if (!e) {
      return null
    }
    const model = e.model === null ? undefined : this.manifest!.models.find((m) => m.index === e.model)
    if (model) {
      const o = new THREE.Vector3(...model.origin)
      return new THREE.Box3(new THREE.Vector3(...model.mins).add(o), new THREE.Vector3(...model.maxs).add(o))
    }
    const marker = this.points?.children.find((c) => c.userData.entity === entity)
    return marker ? new THREE.Box3().setFromObject(marker) : null
  }

  private setHighlight(box: THREE.Box3 | null) {
    if (this.highlight) {
      this.scene.remove(this.highlight)
      this.highlight.dispose()
      this.highlight = null
    }
    if (box) {
      this.highlight = new THREE.Box3Helper(box.clone().expandByScalar(2), 0xffd24a)
      // Seen through walls, as Hammer shows a selection.
      const material = this.highlight.material as THREE.LineBasicMaterial
      material.depthTest = false
      material.transparent = true
      this.highlight.renderOrder = 10
      this.scene.add(this.highlight)
    }
  }

  /** What F brings into view instead of the selected entity: a node, a link or a change of the graph. */
  setFocusTarget(box: THREE.Box3 | null) {
    this.focusTarget = box
  }

  /** Brings the focus target, else the selected entity, into view. */
  focus() {
    const box = this.focusTarget ?? (this.selected === null ? null : this.entityBox(this.selected))
    if (box) {
      this.show(box)
    }
  }

  /**
   * Brings the box into view when its middle is out of sight (behind or off the edges): in 3D the camera turns to it
   * from where it is, and flies over only when it is far.
   */
  reveal(box: THREE.Box3) {
    const camera = this.camera()
    camera.updateMatrixWorld()
    const center = box.getCenter(new THREE.Vector3())
    const p = center.clone().project(camera)
    if (p.z > -1 && p.z < 1 && Math.abs(p.x) < REVEAL_EDGE && Math.abs(p.y) < REVEAL_EDGE) {
      return
    }
    if (this.view === '3d' && center.distanceTo(this.perspective.position) < REVEAL_TURN) {
      this.fly.lookAt(center)
    } else {
      this.show(box)
    }
  }

  show(box: THREE.Box3) {
    const center = box.getCenter(new THREE.Vector3())
    if (this.view === '3d') {
      const size = box.getSize(new THREE.Vector3()).length()
      const back = this.fly.forward().multiplyScalar(-Math.max(size * 1.2, 160))
      this.perspective.position.copy(center).add(back)
    } else {
      const depth = viewAxes(this.view).depth
      const keep = this.orthographic.position.getComponent(depth)
      this.orthographic.position.copy(center).setComponent(depth, keep)
    }
  }

  /** The pointer in normalized device coordinates. */
  private ndc(clientX: number, clientY: number): THREE.Vector2 {
    const rect = this.renderer.domElement.getBoundingClientRect()
    return new THREE.Vector2(((clientX - rect.left) / rect.width) * 2 - 1, -((clientY - rect.top) / rect.height) * 2 + 1)
  }

  /** The map surface under the pointer: its point and entity, and its depth along the view (Infinity for none). */
  private surface(clientX: number, clientY: number): { entity: number | null; point: Vec3 | null; depth: number } {
    const camera = this.camera()
    // No frame may have been drawn since the camera moved (a hidden tab draws none).
    camera.updateMatrixWorld()
    this.raycaster.setFromCamera(this.ndc(clientX, clientY), camera)
    const planes = this.renderer.clippingPlanes
    const targets = [...this.map.children, ...(this.points?.children.filter((p) => p.visible) ?? [])]
    const forward = camera.getWorldDirection(new THREE.Vector3())
    for (const hit of this.raycaster.intersectObjects(targets, false)) {
      if (planes.some((p) => p.distanceToPoint(hit.point) < 0)) {
        continue
      }
      const obj = hit.object as THREE.Mesh
      const mat = Array.isArray(obj.material) ? obj.material[hit.face?.materialIndex ?? 0] : obj.material
      if (!mat?.visible) {
        continue
      }
      const e = (obj.userData.entity as number | null) ?? null
      return {
        entity: e === 0 ? null : e,
        point: [hit.point.x, hit.point.y, hit.point.z],
        depth: hit.point.clone().sub(camera.position).dot(forward),
      }
    }
    return { entity: null, point: null, depth: Infinity }
  }

  /** What is under the pointer: the map surface and its entity, and the graph's node or link drawn nearest. */
  private pick(clientX: number, clientY: number): Pick {
    const rect = this.renderer.domElement.getBoundingClientRect()
    const s = this.surface(clientX, clientY)
    const planes = this.renderer.clippingPlanes
    const nav = this.nav.pick(clientX - rect.left, clientY - rect.top, this.camera(), rect.width, rect.height, s.depth, planes)
    return { entity: s.entity, point: s.point, node: nav.node, link: nav.link }
  }

  /** The pointer's ray into the view (the raycaster is left aimed along it). */
  private rayAt(clientX: number, clientY: number): THREE.Ray {
    const camera = this.camera()
    camera.updateMatrixWorld()
    this.raycaster.setFromCamera(this.ndc(clientX, clientY), camera)
    return this.raycaster.ray.clone()
  }

  private pickHandle(clientX: number, clientY: number): Handle | null {
    if (this.gizmoNode === null) return null
    this.rayAt(clientX, clientY)
    return this.gizmo.pick(this.raycaster)
  }

  /** The plane a plane handle moves in, or the centre in a 2D view: its normal. */
  private planeNormal(handle: Handle): THREE.Vector3 {
    if (handle === 'xy' || handle === 'xz' || handle === 'yz') return NORMAL[handle]
    const n = new THREE.Vector3()
    if (this.view !== '3d') n.setComponent(viewAxes(this.view).depth, 1)
    return n
  }

  private beginDrag(node: number, handle: Handle, e: PointerEvent) {
    const model = this.nav.model
    if (!model || node >= model.nodes) return
    const start = new THREE.Vector3(...model.origin(node))
    const ray = this.rayAt(e.clientX, e.clientY)
    let t0 = 0
    let grab: THREE.Vector3 | null = null
    if (handle === 'x' || handle === 'y' || handle === 'z') {
      t0 = closestOnAxis(ray, start, AXIS[handle]) ?? 0
    } else if (handle !== 'free' || this.view !== '3d') {
      grab = onPlane(ray, start, this.planeNormal(handle)) ?? start.clone()
    }
    this.dragging = { node, handle, start, t0, grab, x: e.clientX, y: e.clientY, to: null }
    this.renderer.domElement.setPointerCapture(e.pointerId)
  }

  /**
   * Where the node dragged would stand with the pointer there: moved along the axis or in the plane taken, or (the
   * node itself in 3D) a player's height over the map surface under the pointer; on the grid unless `free`. The
   * server sets it down on the floor under the spot.
   */
  private dragTarget(clientX: number, clientY: number, free: boolean): Vec3 | null {
    const d = this.dragging
    if (!d) return null
    const snap = (v: number) => (free || this.snap <= 1 ? Math.round(v) : Math.round(v / this.snap) * this.snap)
    const ray = this.rayAt(clientX, clientY)
    const p = d.start.clone()
    const h = d.handle
    if (h === 'x' || h === 'y' || h === 'z') {
      const t = closestOnAxis(ray, d.start, AXIS[h])
      if (t === null) return d.to
      p.addScaledVector(AXIS[h], t - d.t0)
      const i = 'xyz'.indexOf(h)
      p.setComponent(i, snap(p.getComponent(i)))
    } else if (d.grab) {
      const n = this.planeNormal(h)
      const hit = onPlane(ray, d.start, n)
      if (!hit) return d.to
      p.add(hit.sub(d.grab))
      for (let i = 0; i < 3; i++) {
        if (n.getComponent(i) === 0) p.setComponent(i, snap(p.getComponent(i)))
      }
    } else {
      const hit = this.surface(clientX, clientY).point
      if (!hit) return d.to
      p.set(snap(hit[0]), snap(hit[1]), hit[2] + STAND)
    }
    return [Math.round(p.x), Math.round(p.y), Math.round(p.z)]
  }

  private endDrag(e?: PointerEvent) {
    this.dragging = null
    this.nav.setDrag(null)
    this.gizmo.show(this.gizmoAt)
    const canvas = this.renderer.domElement
    canvas.style.cursor = ''
    if (e && canvas.hasPointerCapture(e.pointerId)) {
      canvas.releasePointerCapture(e.pointerId)
    }
  }

  private onDown = (e: PointerEvent) => {
    const canvas = this.renderer.domElement
    canvas.focus()
    if (e.button === 0) {
      this.pressed = { x: e.clientX, y: e.clientY }
      const handle = this.pickHandle(e.clientX, e.clientY)
      if (handle && this.gizmoNode !== null) {
        // A press on the gizmo moves its node or does nothing: never a click on the map under it.
        this.pressed = null
        if (this.events.moveStart(this.gizmoNode)) this.beginDrag(this.gizmoNode, handle, e)
      } else {
        const pick = this.pick(e.clientX, e.clientY)
        if (pick.node !== null && this.events.grab(pick)) this.beginDrag(pick.node, 'free', e)
      }
    }
    if (this.view === '3d') {
      this.fly.pointerDown(e)
    } else {
      this.flat.pointerDown(e)
    }
  }

  private onMove = (e: PointerEvent) => {
    const d = this.dragging
    if (d && (d.to || Math.hypot(e.clientX - d.x, e.clientY - d.y) > CLICK_SLOP)) {
      const now = performance.now()
      if (now - this.dragged >= DRAG_EVERY) {
        this.dragged = now
        this.renderer.domElement.style.cursor = 'grabbing'
        const to = this.dragTarget(e.clientX, e.clientY, e.altKey)
        if (to && String(to) !== String(d.to)) {
          d.to = to
          this.nav.setDrag({ from: [d.start.x, d.start.y, d.start.z], to, marker: false })
          this.gizmo.show(new THREE.Vector3(...to))
          this.events.moveLive(d.node, to)
        }
      }
    }
    if (e.buttons === 0) {
      const now = performance.now()
      if (now - this.hovered > HOVER_EVERY) {
        this.hovered = now
        this.events.hover(this.pick(e.clientX, e.clientY))
      }
      if (this.gizmoNode !== null) {
        const h = this.pickHandle(e.clientX, e.clientY)
        this.gizmo.highlight(h)
        this.renderer.domElement.style.cursor = h ? 'grab' : ''
      }
    }
    if (this.view === '3d') {
      this.fly.pointerMove(e)
    } else {
      this.flat.pointerMove(e)
    }
  }

  private onUp = (e: PointerEvent) => {
    const d = this.dragging
    if (e.button === 0 && d) {
      const moved = Math.hypot(e.clientX - d.x, e.clientY - d.y) > CLICK_SLOP
      const to = moved ? (this.dragTarget(e.clientX, e.clientY, e.altKey) ?? d.to) : null
      this.endDrag(e)
      if (moved) this.pressed = null
      this.events.drop(d.node, to)
    }
    if (e.button === 0 && this.pressed) {
      const moved = Math.hypot(e.clientX - this.pressed.x, e.clientY - this.pressed.y)
      this.pressed = null
      if (moved <= CLICK_SLOP) {
        this.events.click(this.pick(e.clientX, e.clientY))
      }
    }
    if (this.view === '3d') {
      this.fly.pointerUp(e)
    } else {
      this.flat.pointerUp(e)
    }
  }

  private onWheel = (e: WheelEvent) => {
    e.preventDefault()
    if (this.view === '3d') {
      this.fly.wheel(e)
    } else {
      this.flat.wheel(e)
      this.refreshGrid()
    }
  }

  private onKey = (e: KeyboardEvent) => {
    if (e.code === 'KeyF' && !(e.target instanceof HTMLInputElement)) {
      this.focus()
    }
    if (e.code === 'Escape' && this.dragging) {
      // Back where it was, and no click on release; the selection stays (the editor's Esc would drop it).
      e.stopImmediatePropagation()
      const node = this.dragging.node
      this.endDrag()
      this.pressed = null
      this.events.drop(node, null)
    }
  }

  dispose() {
    cancelAnimationFrame(this.raf)
    this.unload()
    this.resizer.disconnect()
    this.nav.dispose()
    this.gizmo.dispose()
    this.keys.dispose()
    window.removeEventListener('keydown', this.onKey)
    this.renderer.dispose()
    this.renderer.domElement.remove()
  }
}
