import type { EditedGraph, Manifest, MapFile, NavInfo, OnDisk, OverlayFile, Preview, Route, Vec3 } from './types'

export class ApiError extends Error {
  constructor(
    readonly status: number,
    message: string,
  ) {
    super(message)
  }
}

async function get(url: string): Promise<Response> {
  const r = await fetch(url, { credentials: 'same-origin' })
  if (!r.ok) {
    throw new ApiError(r.status, (await r.text()).trim() || r.statusText)
  }
  return r
}

export async function listMaps(): Promise<MapFile[]> {
  return (await get('/api/maps')).json()
}

export async function manifest(map: string): Promise<Manifest> {
  return (await get(`/api/maps/${encodeURIComponent(map)}`)).json()
}

/** A file built from the map, addressed by its fingerprint. */
export function mapUrl(m: Manifest, path: string): string {
  return `/api/maps/${encodeURIComponent(m.map)}/${m.fingerprint}/${path}`
}

export async function meshBuffer(m: Manifest): Promise<ArrayBuffer> {
  return (await get(mapUrl(m, 'mesh.bin'))).arrayBuffer()
}

async function send(method: string, url: string, body?: unknown): Promise<Response> {
  const r = await fetch(url, {
    method,
    credentials: 'same-origin',
    headers: { 'X-LB': '1', ...(body === undefined ? {} : { 'Content-Type': 'application/json' }) },
    body: body === undefined ? undefined : JSON.stringify(body),
  })
  if (!r.ok && r.status !== 409) {
    throw new ApiError(r.status, (await r.text()).trim() || r.statusText)
  }
  return r
}

const mapPath = (map: string) => `/api/maps/${encodeURIComponent(map)}`

export async function navInfo(map: string): Promise<NavInfo> {
  return (await get(`${mapPath(map)}/nav`)).json()
}

export async function navPreview(map: string, editor: OverlayFile): Promise<Preview> {
  return (await send('POST', `${mapPath(map)}/nav/preview`, { editor })).json()
}

export async function navRoute(
  map: string,
  editor: OverlayFile,
  from: Vec3,
  to: Vec3,
  longjump: boolean,
  gauss: boolean,
): Promise<Route> {
  return (await send('POST', `${mapPath(map)}/nav/route`, { editor, from, to, longjump, gauss })).json()
}

/** Saves the editor file over version `base`: the new version, or the file on disk when it changed meanwhile. */
export async function saveEditor(
  map: string,
  file: OverlayFile,
  base: string,
): Promise<{ saved: true; version: string; graph: EditedGraph } | { saved: false; now: OnDisk }> {
  const r = await send('PUT', `${mapPath(map)}/overlay`, { file, base })
  if (r.status === 409) {
    return { saved: false, now: await r.json() }
  }
  const body = await r.json()
  return { saved: true, version: body.version, graph: body.graph }
}

export async function applyOnServer(map: string): Promise<{ sent: string; to: string }> {
  return (await send('POST', `${mapPath(map)}/apply`)).json()
}
