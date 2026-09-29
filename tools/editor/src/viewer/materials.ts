import * as THREE from 'three'

import { mapUrl } from '../api'
import type { Group, Layer, Manifest, ModelInfo } from '../types'

// GoldSrc keeps lightmaps dark and brightens them through its light gamma table; a power curve comes close.
const VERTEX = /* glsl */ `
#include <clipping_planes_pars_vertex>
attribute vec2 lightUv;
varying vec2 vUv;
varying vec2 vLightUv;
void main() {
  vUv = uv;
  vLightUv = lightUv;
  vec4 mvPosition = modelViewMatrix * vec4(position, 1.0);
  gl_Position = projectionMatrix * mvPosition;
  #include <clipping_planes_vertex>
}
`

const FRAGMENT = /* glsl */ `
#include <clipping_planes_pars_fragment>
uniform sampler2D map;
uniform sampler2D lightmap;
uniform float lit;
uniform float brightness;
uniform float opacity;
uniform float cutout;
uniform vec3 tint;
varying vec2 vUv;
varying vec2 vLightUv;
void main() {
  #include <clipping_planes_fragment>
  vec4 t = texture2D(map, vUv);
  if (t.a < cutout) discard;
  vec3 light = mix(vec3(1.0), pow(texture2D(lightmap, vLightUv).rgb, vec3(0.6)) * brightness, lit);
  gl_FragColor = vec4(t.rgb * light * tint, opacity);
}
`

const RENDER_TRANS_COLOR = 1
const RENDER_TRANS_TEXTURE = 2
const RENDER_TRANS_ALPHA = 4
const RENDER_TRANS_ADD = 5

function checker(): THREE.Texture {
  const size = 8
  const data = new Uint8Array(size * size * 4)
  for (let y = 0; y < size; y++) {
    for (let x = 0; x < size; x++) {
      const on = (x >> 2) ^ (y >> 2)
      data.set(on ? [255, 0, 255, 255] : [24, 24, 24, 255], (y * size + x) * 4)
    }
  }
  const t = new THREE.DataTexture(data, size, size)
  t.wrapS = t.wrapT = THREE.RepeatWrapping
  t.magFilter = THREE.NearestFilter
  t.needsUpdate = true
  return t
}

function white(): THREE.Texture {
  const t = new THREE.DataTexture(new Uint8Array([255, 255, 255, 255]), 1, 1)
  t.needsUpdate = true
  return t
}

/** Textures, lightmap pages and materials of one map, made as the groups ask for them. */
export class MapMaterials {
  private loader = new THREE.TextureLoader()
  private textures = new Map<number, THREE.Texture>()
  private pages = new Map<number, THREE.Texture>()
  private materials = new Map<string, THREE.ShaderMaterial>()
  private missing = checker()
  private blank = white()
  private brightness = 1

  constructor(private manifest: Manifest) {}

  private texture(i: number | null): THREE.Texture {
    const info = i === null ? undefined : this.manifest.textures[i]
    if (i === null || !info || info.source === 'missing') {
      return this.missing
    }
    let t = this.textures.get(i)
    if (!t) {
      t = this.loader.load(mapUrl(this.manifest, `texture/${i}.png`))
      t.flipY = false
      t.wrapS = t.wrapT = THREE.RepeatWrapping
      t.anisotropy = 4
      if (info.alpha) {
        t.magFilter = THREE.NearestFilter
        t.minFilter = THREE.NearestMipmapNearestFilter
      }
      this.textures.set(i, t)
    }
    return t
  }

  private page(i: number | null): THREE.Texture {
    if (i === null) {
      return this.blank
    }
    let t = this.pages.get(i)
    if (!t) {
      t = this.loader.load(mapUrl(this.manifest, `lightmap/${i}.png`))
      t.flipY = false
      t.generateMipmaps = false
      t.minFilter = THREE.LinearFilter
      this.pages.set(i, t)
    }
    return t
  }

  material(g: Group, m: ModelInfo): THREE.ShaderMaterial {
    const key = `${g.layer}/${g.texture}/${g.page}/${g.kind}/${m.rendermode}/${m.renderamt}`
    let mat = this.materials.get(key)
    if (mat) {
      return mat
    }
    const tex = this.texture(g.texture)
    const uniforms = {
      map: { value: tex },
      lightmap: { value: this.page(g.page) },
      lit: { value: g.page === null ? 0 : 1 },
      brightness: { value: this.brightness },
      opacity: { value: 1 },
      cutout: { value: 0 },
      tint: { value: new THREE.Color(1, 1, 1) },
    }
    mat = new THREE.ShaderMaterial({ uniforms, vertexShader: VERTEX, fragmentShader: FRAGMENT, clipping: true })
    mat.userData.layer = g.layer
    if (g.kind === 'alpha' || m.rendermode === RENDER_TRANS_ALPHA) {
      uniforms.cutout.value = 0.5
    }
    if (g.kind === 'water') {
      uniforms.opacity.value = 0.65
      mat.transparent = true
      mat.depthWrite = false
    } else if (g.kind === 'sky') {
      uniforms.map.value = this.blank
      uniforms.tint.value.setRGB(0.45, 0.62, 0.85)
    } else if (g.kind === 'tool') {
      uniforms.opacity.value = 0.35
      uniforms.lit.value = 0
      mat.transparent = true
      mat.depthWrite = false
    }
    if ([RENDER_TRANS_COLOR, RENDER_TRANS_TEXTURE, RENDER_TRANS_ADD].includes(m.rendermode)) {
      uniforms.opacity.value = Math.max(m.renderamt / 255, 0.25)
      mat.transparent = true
      mat.depthWrite = false
      if (m.rendermode === RENDER_TRANS_ADD) {
        mat.blending = THREE.AdditiveBlending
      }
    }
    this.materials.set(key, mat)
    return mat
  }

  setLayers(layers: Record<Layer, boolean>) {
    for (const m of this.materials.values()) {
      m.visible = layers[m.userData.layer as Layer] ?? true
    }
  }

  setBrightness(b: number) {
    this.brightness = b
    for (const m of this.materials.values()) {
      m.uniforms.brightness.value = b
    }
  }

  setWireframe(on: boolean) {
    for (const m of this.materials.values()) {
      m.wireframe = on
    }
  }

  dispose() {
    for (const t of [...this.textures.values(), ...this.pages.values(), this.missing, this.blank]) {
      t.dispose()
    }
    for (const m of this.materials.values()) {
      m.dispose()
    }
  }
}
