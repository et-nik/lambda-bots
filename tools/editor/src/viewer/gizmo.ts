import * as THREE from 'three'

import type { View } from '../types'

/** What a part of the gizmo moves along: an axis, a plane, or freely (the centre). */
export type Handle = 'x' | 'y' | 'z' | 'xy' | 'xz' | 'yz' | 'free'

export const AXIS: Record<'x' | 'y' | 'z', THREE.Vector3> = {
  x: new THREE.Vector3(1, 0, 0),
  y: new THREE.Vector3(0, 1, 0),
  z: new THREE.Vector3(0, 0, 1),
}

/** The axis a plane handle keeps: the plane's normal. */
export const NORMAL: Record<'xy' | 'xz' | 'yz', THREE.Vector3> = { xy: AXIS.z, xz: AXIS.y, yz: AXIS.x }

/** Length of an arrow on screen, pixels. */
const SIZE = 96
const COLOR: Record<Handle, number> = {
  x: 0xe8574f,
  y: 0x6cc44a,
  z: 0x4a8cf0,
  xy: 0x4a8cf0,
  xz: 0x6cc44a,
  yz: 0xe8574f,
  free: 0xf1f3f6,
}
const HOT = 0xffd24a
/** Handles each view shows: its own plane's axes and the centre. */
const SHOWN: Record<View, Handle[]> = {
  '3d': ['x', 'y', 'z', 'xy', 'xz', 'yz', 'free'],
  top: ['x', 'y', 'free'],
  front: ['y', 'z', 'free'],
  side: ['x', 'z', 'free'],
}

interface Part {
  /** What is drawn, and what the pointer picks it by (a little fatter than an arrow). */
  drawn: (THREE.Mesh | THREE.Sprite)[]
  picks: (THREE.Mesh | THREE.Sprite)[]
  opacity: number
}

function material(color: number, opacity = 1): THREE.MeshBasicMaterial {
  return new THREE.MeshBasicMaterial({
    color,
    opacity,
    transparent: true,
    depthTest: false,
    depthWrite: false,
    side: THREE.DoubleSide,
  })
}

function unseen(): THREE.MeshBasicMaterial {
  const m = new THREE.MeshBasicMaterial()
  m.visible = false
  return m
}

/** The axis' letter by its arrow's tip, facing the camera. */
function letter(text: string, color: number): THREE.Sprite {
  const canvas = document.createElement('canvas')
  canvas.width = canvas.height = 64
  const ctx = canvas.getContext('2d')!
  ctx.font = '700 46px system-ui, sans-serif'
  ctx.textAlign = 'center'
  ctx.textBaseline = 'middle'
  ctx.lineWidth = 8
  ctx.strokeStyle = 'rgba(15, 17, 21, 0.9)'
  ctx.strokeText(text, 32, 34)
  ctx.fillStyle = '#ffffff'
  ctx.fillText(text, 32, 34)
  const sprite = new THREE.Sprite(
    new THREE.SpriteMaterial({ map: new THREE.CanvasTexture(canvas), color, depthTest: false, transparent: true }),
  )
  sprite.scale.setScalar(0.24)
  sprite.renderOrder = 33
  return sprite
}

/** A mesh along +Y from `from` to `to` (a unit long thing made along Y), turned to point along `dir`. */
function along(geo: THREE.BufferGeometry, mat: THREE.Material, dir: THREE.Vector3, from: number, to: number): THREE.Mesh {
  geo.translate(0, (from + to) / 2, 0)
  const mesh = new THREE.Mesh(geo, mat)
  mesh.quaternion.setFromUnitVectors(new THREE.Vector3(0, 1, 0), dir)
  mesh.renderOrder = 30
  return mesh
}

/** Arrows for the axes, squares for the planes and a ball in the middle, over everything, the same size on screen. */
export class MoveGizmo {
  readonly group = new THREE.Group()
  private parts = new Map<Handle, Part>()
  private shown = new Set<Handle>(SHOWN['3d'])
  private hot: Handle | null = null

  constructor(scene: THREE.Scene) {
    for (const h of ['x', 'y', 'z'] as const) {
      const dir = AXIS[h]
      const color = material(COLOR[h])
      const shaft = along(new THREE.CylinderGeometry(0.026, 0.026, 0.68, 8), color, dir, 0.12, 0.8)
      const head = along(new THREE.ConeGeometry(0.085, 0.24, 16), color, dir, 0.76, 1)
      const pick = along(new THREE.CylinderGeometry(0.09, 0.09, 0.95, 8), unseen(), dir, 0.08, 1.03)
      const label = letter(h.toUpperCase(), COLOR[h])
      label.position.copy(dir).multiplyScalar(1.2)
      this.add(h, [shaft, head, label], [pick, label], 1)
    }
    for (const h of ['xy', 'xz', 'yz'] as const) {
      const square = new THREE.Mesh(new THREE.PlaneGeometry(0.22, 0.22), material(COLOR[h], 0.35))
      square.renderOrder = 31
      if (h === 'xy') square.position.set(0.36, 0.36, 0)
      if (h === 'xz') {
        square.rotation.x = Math.PI / 2
        square.position.set(0.36, 0, 0.36)
      }
      if (h === 'yz') {
        square.rotation.y = -Math.PI / 2
        square.position.set(0, 0.36, 0.36)
      }
      this.add(h, [square], [square], 0.35)
    }
    const ball = new THREE.Mesh(new THREE.SphereGeometry(0.075, 16, 12), material(COLOR.free, 0.9))
    ball.renderOrder = 32
    const grip = new THREE.Mesh(new THREE.SphereGeometry(0.13, 8, 6), unseen())
    this.add('free', [ball], [grip], 0.9)
    this.group.visible = false
    scene.add(this.group)
  }

  private add(h: Handle, drawn: Part['drawn'], picks: Part['picks'], opacity: number) {
    this.parts.set(h, { drawn, picks, opacity })
    for (const m of new Set([...drawn, ...picks])) this.group.add(m)
  }

  get visible(): boolean {
    return this.group.visible
  }

  /** Stands the gizmo at `at`, or takes it away. */
  show(at: THREE.Vector3 | null) {
    this.group.visible = at !== null
    if (at) this.group.position.copy(at)
  }

  setView(view: View) {
    this.shown = new Set(SHOWN[view])
    for (const [h, part] of this.parts) {
      for (const m of [...part.drawn, ...part.picks]) m.visible = this.shown.has(h)
    }
  }

  /** Keeps the arrows `SIZE` pixels long on a view `height` pixels high. */
  fit(camera: THREE.Camera, height: number) {
    let scale: number
    if (camera instanceof THREE.PerspectiveCamera) {
      const d = camera.position.distanceTo(this.group.position)
      scale = (2 * d * Math.tan(THREE.MathUtils.degToRad(camera.fov) / 2) * SIZE) / Math.max(height, 1)
    } else {
      scale = SIZE / (camera as THREE.OrthographicCamera).zoom
    }
    this.group.scale.setScalar(scale)
  }

  /** The handle the ray meets first, among those shown. */
  pick(raycaster: THREE.Raycaster): Handle | null {
    if (!this.group.visible) return null
    this.group.updateMatrixWorld(true)
    let best: { h: Handle; d: number } | null = null
    for (const [h, part] of this.parts) {
      if (!this.shown.has(h)) continue
      for (const hit of raycaster.intersectObjects(part.picks, false)) {
        if (!best || hit.distance < best.d) best = { h, d: hit.distance }
      }
    }
    // The ball in the middle sits in front of the arrows' roots: it wins where both are under the pointer.
    return best?.h ?? null
  }

  highlight(h: Handle | null) {
    if (h === this.hot) return
    this.hot = h
    for (const [k, part] of this.parts) {
      for (const m of part.drawn) {
        const mat = m.material as THREE.MeshBasicMaterial | THREE.SpriteMaterial
        mat.color.setHex(k === h ? HOT : COLOR[k])
        mat.opacity = k === h ? Math.max(part.opacity, 0.7) : part.opacity
      }
    }
  }

  dispose() {
    this.group.traverse((o) => {
      const m = o as THREE.Mesh
      m.geometry?.dispose()
      const mat = m.material as THREE.SpriteMaterial | undefined
      mat?.map?.dispose()
      mat?.dispose()
    })
    this.group.removeFromParent()
  }
}

/** Where along the line `p0 + a·t` the ray comes closest to it: `t`, or null when they run (almost) parallel. */
export function closestOnAxis(ray: THREE.Ray, p0: THREE.Vector3, a: THREE.Vector3): number | null {
  const w = p0.clone().sub(ray.origin)
  const b = a.dot(ray.direction)
  const denom = 1 - b * b
  if (denom < 1e-4) return null
  return (b * ray.direction.dot(w) - a.dot(w)) / denom
}

/** Where the ray meets the plane through `p0` across `n`, when it meets it in front. */
export function onPlane(ray: THREE.Ray, p0: THREE.Vector3, n: THREE.Vector3): THREE.Vector3 | null {
  const denom = n.dot(ray.direction)
  if (Math.abs(denom) < 1e-4) return null
  const s = n.dot(p0.clone().sub(ray.origin)) / denom
  return s < 0 ? null : ray.at(s, new THREE.Vector3())
}
