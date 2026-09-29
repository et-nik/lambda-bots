import * as THREE from 'three'

import type { EntityInfo, Layer } from '../types'

interface Look {
  mins: [number, number, number]
  maxs: [number, number, number]
  color: number
  layer: Layer
}

const PLAYER: Look = { mins: [-16, -16, -36], maxs: [16, 16, 36], color: 0x3ddc84, layer: 'points' }
const PICKUP: Omit<Look, 'color'> = { mins: [-12, -12, 0], maxs: [12, 12, 16], layer: 'points' }
const SMALL: Omit<Look, 'color' | 'layer'> = { mins: [-6, -6, -6], maxs: [6, 6, 6] }

/** How a point entity shows: players and monsters by their hull, pickups on the floor, the rest as a small cube. */
export function lookOf(classname: string): Look {
  if (classname === 'info_player_deathmatch' || classname === 'info_player_start') {
    return PLAYER
  }
  if (classname.startsWith('weapon_') || classname === 'weaponbox') {
    return { ...PICKUP, color: 0xff8c1a }
  }
  if (classname.startsWith('ammo_')) {
    return { ...PICKUP, color: 0xd9a441 }
  }
  if (classname.startsWith('item_')) {
    return { ...PICKUP, color: 0x33c3f0 }
  }
  if (classname.startsWith('monster_')) {
    return { mins: [-16, -16, 0], maxs: [16, 16, 72], color: 0xe5484d, layer: 'points' }
  }
  if (classname.startsWith('light')) {
    return { ...SMALL, color: 0xf5e663, layer: 'lights' }
  }
  return { ...SMALL, color: 0x9aa4b2, layer: 'points' }
}

/** Boxes for the map's point entities; each keeps its entity index and layer in `userData`. */
export function pointMarkers(entities: EntityInfo[]): THREE.Group {
  const group = new THREE.Group()
  const geometries = new Map<string, { box: THREE.BoxGeometry; edges: THREE.EdgesGeometry }>()
  const materials = new Map<number, { fill: THREE.MeshBasicMaterial; line: THREE.LineBasicMaterial }>()
  for (const e of entities) {
    if (e.model !== null || e.classname === 'worldspawn' || e.index === 0) {
      continue
    }
    const look = lookOf(e.classname)
    const size = look.maxs.map((v, i) => v - look.mins[i])
    const key = size.join('x')
    let geo = geometries.get(key)
    if (!geo) {
      const box = new THREE.BoxGeometry(size[0], size[1], size[2])
      geo = { box, edges: new THREE.EdgesGeometry(box) }
      geometries.set(key, geo)
    }
    let mat = materials.get(look.color)
    if (!mat) {
      mat = {
        fill: new THREE.MeshBasicMaterial({ color: look.color, transparent: true, opacity: 0.35, depthWrite: false }),
        line: new THREE.LineBasicMaterial({ color: look.color }),
      }
      materials.set(look.color, mat)
    }
    const mesh = new THREE.Mesh(geo.box, mat.fill)
    mesh.add(new THREE.LineSegments(geo.edges, mat.line))
    mesh.position.set(
      e.origin[0] + (look.mins[0] + look.maxs[0]) / 2,
      e.origin[1] + (look.mins[1] + look.maxs[1]) / 2,
      e.origin[2] + (look.mins[2] + look.maxs[2]) / 2,
    )
    mesh.userData = { entity: e.index, layer: look.layer }
    group.add(mesh)
  }
  group.userData.dispose = () => {
    for (const g of geometries.values()) {
      g.box.dispose()
      g.edges.dispose()
    }
    for (const m of materials.values()) {
      m.fill.dispose()
      m.line.dispose()
    }
  }
  return group
}
