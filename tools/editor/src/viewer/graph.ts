import type { NavInfo, Preview } from '../types'

/** `NodeFlags` of `lb-nav`. */
export const NODE = {
  CROUCH: 1,
  LADDER: 2,
  GOAL: 4,
  CAMP: 8,
  SNIPER: 16,
  MECHANISM: 32,
  AIRBORNE: 64,
  WATER: 128,
  ON_MOVER: 256,
} as const

/** `LinkFlags` of `lb-nav`. */
export const LINK = {
  VALID: 1,
  IMPORTED: 2,
  TRUSTED: 4,
  LIVE_CONFIRMED: 8,
  LIVE_MISMATCH: 16,
  DYNAMIC: 32,
  MECHANISM: 64,
} as const

/** Link colors by kind, in the order of `LinkKind::ALL`: tricks apart from red, which marks links that are off. */
export const KIND_COLORS: Record<string, number> = {
  walk: 0xb4b4b4,
  crouch: 0x9664ff,
  jump: 0x33e633,
  drop: 0xffe633,
  ladder: 0x33ffff,
  swim: 0x3380ff,
  door: 0xff9633,
  lift: 0xc833ff,
  teleport: 0xffffff,
  breakable: 0xa0642d,
  push: 0xff5cae,
  longjump: 0x9cff33,
  gauss_boost: 0x00c8a0,
}

/** Links that are off (the check failed, or the live server disagreed). */
export const OFF_COLOR = 0xff2020

export function nodeColor(flags: number): number {
  if (flags & NODE.GOAL) return 0xffdc00
  if (flags & NODE.LADDER) return 0x00ffff
  if (flags & NODE.WATER) return 0x0064ff
  if (flags & NODE.CROUCH) return 0x9664ff
  if (flags & NODE.MECHANISM) return 0xff9600
  return 0x78ff78
}

export function nodeFlagNames(flags: number): string[] {
  return Object.entries(NODE)
    .filter(([, bit]) => flags & bit)
    .map(([name]) => name.toLowerCase())
}

export interface Link {
  from: number
  to: number
  kind: string
  flags: number
  /** Seconds. */
  cost: number
  /** Put in by the overlays (or changed by them). */
  added: boolean
}

export function linkValid(l: Link): boolean {
  return (l.flags & LINK.VALID) !== 0 && (l.flags & LINK.LIVE_MISMATCH) === 0
}

/** The graph as the overlays leave it: the base graph, less the links they take out, plus what they put in. */
export class GraphModel {
  readonly kinds: string[]
  readonly baseNodes: number
  readonly nodes: number
  readonly pos: Float32Array
  readonly flags: Uint32Array
  readonly links: Link[]
  /** Links of the base graph the overlays take out (or change), `[from, to]`. */
  readonly removed: [number, number][]
  private out: number[][]
  private into: number[][]

  constructor(info: NavInfo, preview: Preview | null) {
    this.kinds = info.kinds
    const base = info.base
    const added = preview?.nodes ?? []
    this.baseNodes = base.nodes.length / 4
    this.nodes = this.baseNodes + added.length / 4
    this.pos = new Float32Array(this.nodes * 3)
    this.flags = new Uint32Array(this.nodes)
    const all = [base.nodes, added]
    let n = 0
    for (const list of all) {
      for (let i = 0; i < list.length; i += 4, n++) {
        this.pos.set([list[i], list[i + 1], list[i + 2]], n * 3)
        this.flags[n] = list[i + 3]
      }
    }
    const removed = new Set<string>()
    this.removed = []
    const gone = preview?.removed ?? []
    for (let i = 0; i < gone.length; i += 2) {
      removed.add(`${gone[i]},${gone[i + 1]}`)
      this.removed.push([gone[i], gone[i + 1]])
    }
    this.links = []
    const read = (list: number[], isAdded: boolean) => {
      for (let i = 0; i < list.length; i += 5) {
        const [from, to] = [list[i], list[i + 1]]
        if (!isAdded && removed.has(`${from},${to}`)) {
          continue
        }
        this.links.push({
          from,
          to,
          kind: this.kinds[list[i + 2]] ?? 'walk',
          flags: list[i + 3],
          cost: list[i + 4] / 100,
          added: isAdded,
        })
      }
    }
    read(base.links, false)
    read(preview?.added ?? [], true)
    this.out = Array.from({ length: this.nodes }, () => [])
    this.into = Array.from({ length: this.nodes }, () => [])
    this.links.forEach((l, i) => {
      this.out[l.from]?.push(i)
      this.into[l.to]?.push(i)
    })
  }

  origin(n: number): [number, number, number] {
    return [this.pos[n * 3], this.pos[n * 3 + 1], this.pos[n * 3 + 2]]
  }

  linksOut(n: number): Link[] {
    return (this.out[n] ?? []).map((i) => this.links[i])
  }

  linksIn(n: number): Link[] {
    return (this.into[n] ?? []).map((i) => this.links[i])
  }

  find(from: number, to: number): Link | undefined {
    return this.linksOut(from).find((l) => l.to === to)
  }
}
