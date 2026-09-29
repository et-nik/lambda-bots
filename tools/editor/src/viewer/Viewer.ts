import * as THREE from 'three'

import type { Layer, Manifest, Pick, View } from '../types'
import { FlyControls, Keys, OrthoControls, viewAxes } from './controls'
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
}

/** Milliseconds between hover picks. */
const HOVER_EVERY = 50

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
    if (this.gridLines) {
      this.scene.remove(this.gridLines)
      this.gridLines.geometry.dispose()
      ;(this.gridLines.material as THREE.Material).dispose()
      this.gridLines = null
    }
    if (view !== '3d') {
      const [mins, maxs] = this.bounds()
      const axes = viewAxes(view)
      this.flat.frame(view, mins, maxs)
      this.gridLines = grid(axes, mins, maxs, mins.getComponent(axes.depth) - 64)
      this.gridLines.renderOrder = -1
      this.scene.add(this.gridLines)
    }
    this.applySlice()
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

  /** What is under the pointer: the map surface and its entity, and the graph's node or link drawn nearest. */
  private pick(clientX: number, clientY: number): Pick {
    const rect = this.renderer.domElement.getBoundingClientRect()
    const ndc = new THREE.Vector2(((clientX - rect.left) / rect.width) * 2 - 1, -((clientY - rect.top) / rect.height) * 2 + 1)
    const camera = this.camera()
    // No frame may have been drawn since the camera moved (a hidden tab draws none).
    camera.updateMatrixWorld()
    this.raycaster.setFromCamera(ndc, camera)
    const planes = this.renderer.clippingPlanes
    const targets = [...this.map.children, ...(this.points?.children.filter((p) => p.visible) ?? [])]
    const forward = camera.getWorldDirection(new THREE.Vector3())
    let entity: number | null = null
    let point: [number, number, number] | null = null
    let surface = Infinity
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
      entity = e === 0 ? null : e
      point = [hit.point.x, hit.point.y, hit.point.z]
      surface = hit.point.clone().sub(camera.position).dot(forward)
      break
    }
    const nav = this.nav.pick(clientX - rect.left, clientY - rect.top, camera, rect.width, rect.height, surface, planes)
    return { entity, point, node: nav.node, link: nav.link }
  }

  private onDown = (e: PointerEvent) => {
    this.renderer.domElement.focus()
    if (e.button === 0) {
      this.pressed = { x: e.clientX, y: e.clientY }
    }
    if (this.view === '3d') {
      this.fly.pointerDown(e)
    } else {
      this.flat.pointerDown(e)
    }
  }

  private onMove = (e: PointerEvent) => {
    if (e.buttons === 0) {
      const now = performance.now()
      if (now - this.hovered > HOVER_EVERY) {
        this.hovered = now
        this.events.hover(this.pick(e.clientX, e.clientY))
      }
    }
    if (this.view === '3d') {
      this.fly.pointerMove(e)
    } else {
      this.flat.pointerMove(e)
    }
  }

  private onUp = (e: PointerEvent) => {
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
    }
  }

  private onKey = (e: KeyboardEvent) => {
    if (e.code === 'KeyF' && !(e.target instanceof HTMLInputElement)) {
      this.focus()
    }
  }

  dispose() {
    cancelAnimationFrame(this.raf)
    this.unload()
    this.resizer.disconnect()
    this.nav.dispose()
    this.keys.dispose()
    window.removeEventListener('keydown', this.onKey)
    this.renderer.dispose()
    this.renderer.domElement.remove()
  }
}
