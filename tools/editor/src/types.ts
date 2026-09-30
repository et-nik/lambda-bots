// The server's answers (`lb-mapmesh` manifest, `lb-editor` routes).

export interface MapFile {
  name: string
  bytes: number
}

export type Kind = 'solid' | 'alpha' | 'water' | 'sky' | 'tool'

export type Layer =
  | 'world'
  | 'movers'
  | 'breakables'
  | 'walls'
  | 'water'
  | 'ladders'
  | 'triggers'
  | 'sky'
  | 'points'
  | 'lights'

export interface Group {
  texture: number | null
  page: number | null
  kind: Kind
  layer: Layer
  first: number
  count: number
}

export interface ModelInfo {
  index: number
  entity: number | null
  classname: string
  layer: Layer
  origin: [number, number, number]
  mins: [number, number, number]
  maxs: [number, number, number]
  rendermode: number
  renderamt: number
  groups: Group[]
}

export interface EntityInfo {
  index: number
  classname: string
  targetname: string | null
  origin: [number, number, number]
  model: number | null
  kv: [string, string][]
}

export interface TextureInfo {
  name: string
  width: number
  height: number
  source: string
  alpha: boolean
}

export interface Manifest {
  format: number
  map: string
  fingerprint: string
  mins: [number, number, number]
  maxs: [number, number, number]
  wads: { name: string; found: boolean }[]
  textures: TextureInfo[]
  lightmaps: { size: number; pages: number }
  models: ModelInfo[]
  entities: EntityInfo[]
  buffers: {
    vertices: number
    indices: number
    positions: number
    uvs: number
    light_uvs: number
    triangles: number
    bytes: number
  }
  stats: {
    faces: number
    skipped: number
    triangles: number
    missing_textures: number
    lightmaps: { faces: number; float: number; mismatched: number }
  }
}

export type View = '3d' | 'top' | 'front' | 'side'

export type Vec3 = [number, number, number]

export type PatchOp =
  | { op: 'forbid'; at: Vec3; radius: number; note?: string }
  | { op: 'add_link'; from: Vec3; to: Vec3; kind?: string; both: boolean; trust: boolean; note?: string }
  | { op: 'remove_link'; from: Vec3; to: Vec3; both: boolean; note?: string }
  | { op: 'add_node'; at: Vec3; link?: boolean; note?: string }
  | { op: 'move_node'; from: Vec3; to: Vec3; note?: string }

export interface Place {
  name: string
  at: Vec3
  radius: number
  tags?: string[]
}

/** `maps/<map>/editor.yaml` and `overlay.yaml` (schema `lambdabots/overlay@1`). */
export interface OverlayFile {
  schema: string
  map: string
  bsp_size?: number
  places?: Place[]
  nav?: { patches?: PatchOp[] }
}

export interface OnDisk {
  file: OverlayFile | null
  /** `none` for no file. */
  version: string
  error: string | null
}

export interface Outcome {
  ok: boolean
  message: string
  nodes: number[]
  links: [number, number][]
  /** Links it asked for that do not check out, and why: a change that put a link in one way only has some. */
  refused: { from: number; to: number; why: string }[]
}

/**
 * What the page flags: an error (a link a change asked for that does not check out, a link bots failed in test runs)
 * or one to look at (a change that does nothing, a trick link that lands only in some tries, one put in without the
 * check, a walk that falls off a ledge on the way).
 */
export interface Problem {
  level: 'error' | 'attention'
  kind: 'refused' | 'failed' | 'missed' | 'idle' | 'weak' | 'trusted' | 'fall'
  /** The link's ends in the graph shown; null where no node stands. */
  from: number | null
  to: number | null
  /** Where the ends stand (x, y, z each): a link not in the graph is drawn from them. */
  at: [number, number, number, number, number, number]
  /** The link's kind, or the kind a change asked for. */
  link: string
  why: string
  /** The change it comes of: `editor` or `overlay`, and its index there. */
  patch: ['editor' | 'overlay', number] | null
}

/** Flat graph: `[x, y, z, flags]` a node, `[from, to, kind, flags, centiseconds]` a link. */
export interface FlatGraph {
  nodes: number[]
  links: number[]
}

export interface NavInfo {
  origin: 'server' | 'made'
  /** The load of the map's graph the page works on: its previews and routes name it. */
  revision: number
  kinds: string[]
  base: FlatGraph
  editor: OnDisk
  overlay: OnDisk
  apply: { available: boolean; detail: string }
  bsp_size: number
}

export interface Preview {
  /** Nodes put in, numbered on from the base graph's. */
  nodes: number[]
  /** Nodes of the base graph moved: `[node, x, y, z, flags]` each. */
  moved: number[]
  added: number[]
  /** `[from, to]` pairs. */
  removed: number[]
  editor: Outcome[]
  overlay: Outcome[]
  /** Errors first. */
  problems: Problem[]
}

export interface Route {
  start: number
  goal: number
  nodes: number[]
  legs: { from: number; to: number; kind: string; cost: number }[]
  time: number
  plain: number | null
}

export type Tool = 'select' | 'link' | 'unlink' | 'node' | 'move' | 'forbid' | 'place' | 'route'

/** What became of `editor.lbnav`, the server's graph with the changes applied, on a save. */
export interface EditedGraph {
  written: boolean
  /** Where it is, or why it is not. */
  detail: string
}

/** What a click hit: an entity, a node or a link of the graph, and the point on the map under it. */
export interface Pick {
  entity: number | null
  node: number | null
  link: [number, number] | null
  point: Vec3 | null
}
