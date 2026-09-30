import * as THREE from 'three'

/**
 * A grid for a 2D view in the plane across `across` and `up`, at `depth` along the view's axis: a line every
 * `step` units over the box, every eighth one brighter.
 */
export function grid(
  axes: { depth: number; across: number; up: number },
  mins: THREE.Vector3,
  maxs: THREE.Vector3,
  depth: number,
  step = 64,
): THREE.LineSegments {
  const lo = [mins.getComponent(axes.across), mins.getComponent(axes.up)].map((v) => Math.floor(v / step - 1) * step)
  const hi = [maxs.getComponent(axes.across), maxs.getComponent(axes.up)].map((v) => Math.ceil(v / step + 1) * step)
  const positions: number[] = []
  const colors: number[] = []
  const minor = new THREE.Color(0x2a2e36)
  const major = new THREE.Color(0x3d4450)
  const axis = new THREE.Color(0x56607a)
  const point = (a: number, u: number) => {
    const p = new THREE.Vector3()
    p.setComponent(axes.across, a)
    p.setComponent(axes.up, u)
    p.setComponent(axes.depth, depth)
    positions.push(p.x, p.y, p.z)
  }
  const color = (v: number) => {
    const c = v === 0 ? axis : v % (step * 8) === 0 ? major : minor
    colors.push(c.r, c.g, c.b, c.r, c.g, c.b)
  }
  for (let a = lo[0]; a <= hi[0]; a += step) {
    point(a, lo[1])
    point(a, hi[1])
    color(a)
  }
  for (let u = lo[1]; u <= hi[1]; u += step) {
    point(lo[0], u)
    point(hi[0], u)
    color(u)
  }
  const geo = new THREE.BufferGeometry()
  geo.setAttribute('position', new THREE.Float32BufferAttribute(positions, 3))
  geo.setAttribute('color', new THREE.Float32BufferAttribute(colors, 3))
  return new THREE.LineSegments(geo, new THREE.LineBasicMaterial({ vertexColors: true }))
}
