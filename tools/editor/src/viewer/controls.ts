import * as THREE from 'three'

import type { View } from '../types'

const UP = new THREE.Vector3(0, 0, 1)

/** Keys that would scroll the page or press a focused button, taken for the camera instead. */
const CAMERA_KEYS = new Set(['Space', 'ArrowUp', 'ArrowDown', 'ArrowLeft', 'ArrowRight'])
/** Keys that raise and lower the camera. */
const RISE = ['KeyE', 'Space']
const SINK = ['KeyQ', 'ControlLeft', 'ControlRight']
/** How fast the arrow keys turn the view, radians a second (Shift doubles it). */
const TURN = 2.2
/** How fast the arrow keys move a 2D view, pixels a second (Shift doubles it). */
const PAN = 700

/** A field the keys type into or change: a text box, a list, a slider or a checkbox with the focus. */
function inField(t: EventTarget | null): boolean {
  return t instanceof HTMLInputElement || t instanceof HTMLSelectElement || t instanceof HTMLTextAreaElement
}

/** Keys held down, ignoring the ones typed into fields. */
export class Keys {
  private down = new Set<string>()
  private onDown = (e: KeyboardEvent) => {
    if (inField(e.target)) {
      return
    }
    if (CAMERA_KEYS.has(e.code)) {
      e.preventDefault()
    }
    this.down.add(e.code)
  }
  private onUp = (e: KeyboardEvent) => {
    this.down.delete(e.code)
    // A button with the focus is pressed by Space on its release.
    if (CAMERA_KEYS.has(e.code) && !inField(e.target)) {
      e.preventDefault()
    }
  }
  private onBlur = () => this.down.clear()

  constructor() {
    window.addEventListener('keydown', this.onDown)
    window.addEventListener('keyup', this.onUp)
    window.addEventListener('blur', this.onBlur)
  }

  has(...codes: string[]): boolean {
    return codes.some((c) => this.down.has(c))
  }

  axis(plus: string | string[], minus: string | string[]): number {
    const any = (c: string | string[]) => this.has(...(Array.isArray(c) ? c : [c]))
    return (any(plus) ? 1 : 0) - (any(minus) ? 1 : 0)
  }

  /** 1, or 2 with Shift held. */
  boost(): number {
    return this.has('ShiftLeft', 'ShiftRight') ? 2 : 1
  }

  dispose() {
    window.removeEventListener('keydown', this.onDown)
    window.removeEventListener('keyup', this.onUp)
    window.removeEventListener('blur', this.onBlur)
  }
}

/**
 * The 3D view, flown as in Hammer: the right mouse button or the arrow keys look around, WASD moves, Space or E goes
 * up and Ctrl or Q down, Shift speeds up, the wheel steps forward and back.
 */
export class FlyControls {
  yaw = 0
  pitch = -0.3
  speed = 600
  private looking = false

  constructor(
    readonly camera: THREE.PerspectiveCamera,
    private el: HTMLElement,
    private keys: Keys,
  ) {
    camera.up.copy(UP)
  }

  place(position: THREE.Vector3, yaw: number, pitch: number) {
    this.camera.position.copy(position)
    this.yaw = yaw
    this.pitch = pitch
    this.aim()
  }

  /** Turns to look at `p` from where the camera is. */
  lookAt(p: THREE.Vector3) {
    const d = p.clone().sub(this.camera.position)
    this.yaw = Math.atan2(d.y, d.x)
    this.pitch = THREE.MathUtils.clamp(Math.atan2(d.z, Math.hypot(d.x, d.y)), -1.55, 1.55)
    this.aim()
  }

  forward(): THREE.Vector3 {
    const c = Math.cos(this.pitch)
    return new THREE.Vector3(c * Math.cos(this.yaw), c * Math.sin(this.yaw), Math.sin(this.pitch))
  }

  private aim() {
    this.camera.lookAt(this.camera.position.clone().add(this.forward()))
  }

  pointerDown(e: PointerEvent) {
    if (e.button === 2) {
      this.looking = true
      this.el.setPointerCapture(e.pointerId)
    }
  }

  pointerMove(e: PointerEvent) {
    if (!this.looking) {
      return
    }
    this.yaw -= e.movementX * 0.004
    this.pitch = THREE.MathUtils.clamp(this.pitch - e.movementY * 0.004, -1.55, 1.55)
    this.aim()
  }

  pointerUp(e: PointerEvent) {
    if (e.button === 2) {
      this.looking = false
      this.el.releasePointerCapture(e.pointerId)
    }
  }

  wheel(e: WheelEvent) {
    const step = this.speed * 0.15 * (e.deltaY < 0 ? 1 : -1)
    this.camera.position.addScaledVector(this.forward(), step)
  }

  update(dt: number) {
    const k = this.keys
    const turn = k.axis('ArrowLeft', 'ArrowRight')
    const tilt = k.axis('ArrowUp', 'ArrowDown')
    if (turn !== 0 || tilt !== 0) {
      const step = TURN * k.boost() * dt
      this.yaw += turn * step
      this.pitch = THREE.MathUtils.clamp(this.pitch + tilt * step, -1.55, 1.55)
      this.aim()
    }
    const f = k.axis('KeyW', 'KeyS')
    const r = k.axis('KeyD', 'KeyA')
    const u = k.axis(RISE, SINK)
    if (f === 0 && r === 0 && u === 0) {
      return
    }
    const fast = k.has('ShiftLeft', 'ShiftRight') ? 3 : 1
    const forward = this.forward()
    const right = new THREE.Vector3().crossVectors(forward, UP).normalize()
    const move = forward.multiplyScalar(f).addScaledVector(right, r).addScaledVector(UP, u)
    this.camera.position.addScaledVector(move, this.speed * fast * dt)
  }
}

/** The axis a 2D view looks along, and the ones it shows across and up. */
export function viewAxes(view: Exclude<View, '3d'>): { depth: number; across: number; up: number } {
  switch (view) {
    case 'top':
      return { depth: 2, across: 0, up: 1 }
    case 'front':
      return { depth: 0, across: 1, up: 2 }
    case 'side':
      return { depth: 1, across: 0, up: 2 }
  }
}

/** A 2D view: the right or middle button or the arrow keys move the map, the wheel zooms at the pointer. */
export class OrthoControls {
  private dragging = false

  constructor(
    readonly camera: THREE.OrthographicCamera,
    private el: HTMLElement,
    private keys: Keys,
  ) {}

  update(dt: number) {
    const across = this.keys.axis('ArrowRight', 'ArrowLeft')
    const up = this.keys.axis('ArrowUp', 'ArrowDown')
    if (across === 0 && up === 0) {
      return
    }
    const right = new THREE.Vector3().setFromMatrixColumn(this.camera.matrixWorld, 0)
    const top = new THREE.Vector3().setFromMatrixColumn(this.camera.matrixWorld, 1)
    const step = (PAN * this.keys.boost() * dt) / this.camera.zoom
    this.camera.position.addScaledVector(right, across * step).addScaledVector(top, up * step)
  }

  /** Looks at the box from outside it along the view's axis. */
  frame(view: Exclude<View, '3d'>, mins: THREE.Vector3, maxs: THREE.Vector3) {
    const { depth, up } = viewAxes(view)
    const center = mins.clone().add(maxs).multiplyScalar(0.5)
    const pos = center.clone()
    pos.setComponent(depth, maxs.getComponent(depth) + 4096)
    const upVec = new THREE.Vector3()
    upVec.setComponent(up, 1)
    this.camera.up.copy(upVec)
    this.camera.position.copy(pos)
    this.camera.lookAt(center)
    const size = maxs.clone().sub(mins)
    const across = Math.max(size.getComponent(viewAxes(view).across), size.getComponent(up), 256)
    const w = this.el.clientWidth || 1
    const h = this.el.clientHeight || 1
    this.camera.zoom = Math.min(w, h) / (across * 1.1)
    this.camera.near = 1
    this.camera.far = size.getComponent(depth) + 8192
    this.resize(w, h)
  }

  resize(w: number, h: number) {
    this.camera.left = -w / 2
    this.camera.right = w / 2
    this.camera.top = h / 2
    this.camera.bottom = -h / 2
    this.camera.updateProjectionMatrix()
  }

  pointerDown(e: PointerEvent) {
    if (e.button === 1 || e.button === 2) {
      this.dragging = true
      this.el.setPointerCapture(e.pointerId)
    }
  }

  pointerMove(e: PointerEvent) {
    if (!this.dragging) {
      return
    }
    const right = new THREE.Vector3().setFromMatrixColumn(this.camera.matrixWorld, 0)
    const up = new THREE.Vector3().setFromMatrixColumn(this.camera.matrixWorld, 1)
    const k = 1 / this.camera.zoom
    this.camera.position.addScaledVector(right, -e.movementX * k).addScaledVector(up, e.movementY * k)
  }

  pointerUp(e: PointerEvent) {
    if (this.dragging && (e.button === 1 || e.button === 2)) {
      this.dragging = false
      this.el.releasePointerCapture(e.pointerId)
    }
  }

  wheel(e: WheelEvent) {
    const rect = this.el.getBoundingClientRect()
    const ndc = new THREE.Vector2(((e.clientX - rect.left) / rect.width) * 2 - 1, -((e.clientY - rect.top) / rect.height) * 2 + 1)
    const before = new THREE.Vector3(ndc.x, ndc.y, 0).unproject(this.camera)
    this.camera.zoom = THREE.MathUtils.clamp(this.camera.zoom * (e.deltaY < 0 ? 1.2 : 1 / 1.2), 0.005, 50)
    this.camera.updateProjectionMatrix()
    const after = new THREE.Vector3(ndc.x, ndc.y, 0).unproject(this.camera)
    this.camera.position.add(before.sub(after))
  }
}
