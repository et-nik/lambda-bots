import type { Outcome, PatchOp, Vec3 } from './types'
import { KIND_COLORS, OFF_COLOR } from './viewer/graph'

export function hex(color: number): string {
  return `#${color.toString(16).padStart(6, '0')}`
}

export function kindColor(kind: string, valid = true): string {
  return hex(valid ? (KIND_COLORS[kind] ?? 0xffffff) : OFF_COLOR)
}

export function kindName(kind: string): string {
  return kind.replace('_', ' ')
}

export function spot(p: Vec3): string {
  return p.map((v) => Math.round(v)).join(' ')
}

export function seconds(s: number): string {
  return `${s.toFixed(2)} s`
}

/** A change in a few words, naming the nodes it came to (`outcome`) when it did. */
export function describe(p: PatchOp, outcome?: Outcome | null): string {
  const [a, b] = outcome?.nodes ?? []
  const pair = (both: boolean) => (a !== undefined && b !== undefined ? ` ${a} ${both ? '⇄' : '→'} ${b}` : '')
  switch (p.op) {
    case 'forbid':
      return `Forbid ${p.radius} u`
    case 'add_link':
      return `Link${pair(p.both)}${p.kind ? `, ${kindName(p.kind)}` : ''}${p.trust ? ', trusted' : ''}`
    case 'remove_link':
      return `Unlink${pair(p.both)}`
    case 'add_node':
      return a === undefined ? 'Node' : `Node ${a}`
    case 'move_node': {
      const d = Math.round(Math.hypot(p.to[0] - p.from[0], p.to[1] - p.from[1], p.to[2] - p.from[2]))
      return `Move${a === undefined ? '' : ` ${a}`}, ${d} u`
    }
  }
}
