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
  | { op: 'add_node'; at: Vec3; note?: string }

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
}

/** Flat graph: `[x, y, z, flags]` a node, `[from, to, kind, flags, centiseconds]` a link. */
export interface FlatGraph {
  nodes: number[]
  links: number[]
}

export interface NavInfo {
  origin: 'server' | 'made'
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
  added: number[]
  /** `[from, to]` pairs. */
  removed: number[]
  editor: Outcome[]
  overlay: Outcome[]
}

export interface Route {
  start: number
  goal: number
  nodes: number[]
  legs: { from: number; to: number; kind: string; cost: number }[]
  time: number
  plain: number | null
}

export type Tool = 'select' | 'link' | 'unlink' | 'node' | 'forbid' | 'place' | 'route'

/** What a click hit: an entity, a node or a link of the graph, and the point on the map under it. */
export interface Pick {
  entity: number | null
  node: number | null
  link: [number, number] | null
  point: Vec3 | null
}
