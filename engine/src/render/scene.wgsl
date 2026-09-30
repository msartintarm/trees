// The scene: a sky dome, the continuous terrain with per-biome procedural
// ground materials, instanced vegetation and props, translucent clouds,
// precipitation and ambient particles — all rendered into an HDR target
// that post.wgsl blooms and tone-maps.
//
// Growth animation happens in the vertex stage: it lerps prev_scale → scale
// by the clock alpha, scaling z fully and xy partially so plants read as
// shooting up before filling out.
//
// Lighting: hemisphere ambient that dims to moonlight at night, a key light
// (sun or moon) gated by two shadow-map cascades (sharp near the viewer,
// broad far away), backlit translucency through leaves, and procedural bump
// — value/cellular-noise normal perturbation in world space, per biome on
// the ground, per bark kind on trunks — with no textures at all.

struct Globals {
    view_proj: mat4x4<f32>,
    // The cascade the shadow pass is rendering.
    shadow_vp: mat4x4<f32>,
    // Near (sharp, around the viewer) and far (broad) shadow cascades.
    light_vp0: mat4x4<f32>,
    light_vp1: mat4x4<f32>,
    // Key-light direction (xyz, toward the light); w = lightning flash.
    sun_dir: vec4<f32>,
    alpha: f32,
    // Global illumination level from the climate's sun signal (1 = neutral).
    light: f32,
    // Heat, 0..1: warmer, brighter, more glaring sunlight when hot.
    heat: f32,
    // Cloud cover, 0..1: storm light (dimmer, cooler, softer sun).
    overcast: f32,
    // Camera position (xyz) and wall-clock seconds (w, for animation).
    eye: vec4<f32>,
    // Surface wind (xy direction, z strength, w gust).
    wind: vec4<f32>,
    // Distance haze color (rgb) and density (a).
    haze: vec4<f32>,
    // Valley fog: top height (x) and strength (y); zw = where the latest
    // lightning strike hit (its flash is a local light).
    fog: vec4<f32>,
    // Key-light color × intensity (rgb); w = daylight 0 (night) .. 1.
    sun_color: vec4<f32>,
    // The sun's true direction (xyz, for the sky disc); w = sun trail
    // (0 crisp disc .. 1 smeared along its daily arc as time flies).
    sun_sky: vec4<f32>,
    // x = solar declination, y = latitude (radians), z = sun disc size
    // (grows with heat), w = hex grid overlay (0/1).
    celestial: vec4<f32>,
    // Sky-ray basis: forward, right × tan·aspect, up × tan. w slots:
    // detail quality (0 low .. 1 high), walking (0/1), season phase.
    cam_fwd: vec4<f32>,
    cam_right: vec4<f32>,
    cam_up: vec4<f32>,
    // Regional color grade (rgb multiplier) and exposure (w).
    grade: vec4<f32>,
};

@group(0) @binding(0) var<uniform> globals: Globals;
@group(1) @binding(0) var shadow_near: texture_depth_2d;
@group(1) @binding(1) var shadow_far: texture_depth_2d;
@group(1) @binding(2) var shadow_smp: sampler_comparison;

struct VsIn {
    @location(0) pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) vcolor: vec3<f32>,
    // Baked-material weight: 0 = instance color, 1 = baked color; values
    // above 1 also name a bark kind (see geometry::BARK_*).
    @location(3) vweight: f32,
    @location(4) ipos: vec3<f32>,
    @location(5) scale: f32,
    @location(6) prev_scale: f32,
    @location(7) icolor: vec3<f32>,
    // Canopy-race form: 0 broad … 1 tall and thin (trees only).
    @location(8) slim: f32,
    // Trees: lean toward open light (a shear, world units per unit of
    // height). Clouds: the wind direction (unit vector) to orient along.
    @location(9) lean: vec2<f32>,
    // Surface kind for props: 0 foliage, 2 wood, 3 other.
    @location(10) gloss: f32,
};

fn world_with_form(in: VsIn, slim: f32) -> vec3<f32> {
    let s = mix(in.prev_scale, in.scale, globals.alpha);
    let sxy = 0.3 + 0.7 * s;
    // Shade-avoidance form: crowded youth = taller, narrower.
    let h = 1.0 + 0.45 * slim;
    let w = 1.0 - 0.35 * slim;
    let z = in.pos.z * s * h;
    // Wind sway: stems bend more the higher up, each plant on its own
    // phase, gusting with the storms. Wood (houses, stumps) doesn't sway.
    let pliant = select(1.0, 0.0, in.gloss > 1.5);
    let lz = max(in.pos.z, 0.0);
    let phase = in.ipos.x * 0.37 + in.ipos.y * 0.61;
    let osc = 0.45 + 0.55 * sin(globals.eye.w * 1.9 + phase) + globals.wind.w;
    let sway = globals.wind.xy * (globals.wind.z * lz * lz * 0.05 * s * osc) * pliant;
    // Heat shimmer: a faint wobble on the hottest days.
    let shimmer = max(globals.heat - 0.7, 0.0) / 0.3 * 0.025 * sin(globals.eye.w * 9.0 + in.ipos.y * 2.3 + in.pos.z * 5.0) * pliant;
    // Phototropic lean: the crown leans out over open ground, more the
    // higher up the stem.
    return vec3<f32>(
        in.pos.x * sxy * w + in.lean.x * z + sway.x + shimmer,
        in.pos.y * sxy * w + in.lean.y * z + sway.y,
        z,
    ) + in.ipos;
}

// Clouds: no growth form; the mesh turns to face along the wind (anvils
// stream downwind, cirrus streaks lie along the jet).
fn cloud_world(in: VsIn) -> vec3<f32> {
    let s = mix(in.prev_scale, in.scale, globals.alpha);
    var d = in.lean;
    if (length(d) < 1e-3) {
        d = vec2<f32>(1.0, 0.0);
    } else {
        d = normalize(d);
    }
    let x = in.pos.x * d.x - in.pos.y * d.y;
    let y = in.pos.x * d.y + in.pos.y * d.x;
    return vec3<f32>(x * s, y * s, in.pos.z * s) + in.ipos;
}

fn world_of(in: VsIn) -> vec3<f32> {
    return world_with_form(in, in.slim);
}

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) world: vec3<f32>,
    // Vertex material weight: 1 = baked material (trunks, stems), 0 = crown.
    @location(3) mat_w: f32,
    // Clouds and precipitation: mesh-local position and optical depth τ.
    @location(4) local: vec3<f32>,
    @location(5) tau: f32,
    // x = bark kind (−1 none), y = foliage (backlit translucency), z = wood
    // grain (houses, stumps), w = mesh height (for ambient occlusion).
    @location(6) info: vec4<f32>,
};

@vertex
fn vs_main(in: VsIn) -> VsOut {
    let world = world_of(in);
    var out: VsOut;
    out.clip = globals.view_proj * vec4<f32>(world, 1.0);
    let w = min(in.vweight, 1.0);
    out.color = mix(in.icolor, in.vcolor, w);
    out.normal = in.normal;
    out.world = world;
    out.mat_w = w;
    out.local = vec3<f32>(0.0);
    out.tau = 0.0;
    let bark = select(-1.0, round((in.vweight - 1.0) * 8.0), in.vweight >= 0.99);
    let foliage = select(0.0, 1.0, in.gloss < 0.5 && in.vweight < 0.5);
    let wood = select(0.0, 1.0, in.gloss > 1.5 && in.gloss < 2.5 && in.vweight < 0.5);
    out.info = vec4<f32>(bark, foliage, wood, in.pos.z);
    return out;
}

// Clouds: the instance's `slim` slot carries optical depth τ instead of a
// growth form, so geometry is built with no form distortion.
@vertex
fn vs_cloud(in: VsIn) -> VsOut {
    let world = cloud_world(in);
    var out: VsOut;
    out.clip = globals.view_proj * vec4<f32>(world, 1.0);
    out.color = mix(in.icolor, in.vcolor, min(in.vweight, 1.0));
    var d = in.lean;
    if (length(d) < 1e-3) {
        d = vec2<f32>(1.0, 0.0);
    } else {
        d = normalize(d);
    }
    out.normal = vec3<f32>(in.normal.x * d.x - in.normal.y * d.y, in.normal.x * d.y + in.normal.y * d.x, in.normal.z);
    out.world = world;
    out.mat_w = min(in.vweight, 1.0);
    out.local = in.pos;
    out.tau = in.slim;
    out.info = vec4<f32>(-1.0, 0.0, 0.0, 0.0);
    return out;
}

// Precipitation shafts: the mesh spans z 0 (cloud base) .. −1 (ground);
// scale is the radius, prev_scale the length, lean the wind (the shaft
// slants downwind as it falls).
@vertex
fn vs_rain(in: VsIn) -> VsOut {
    let down = -in.pos.z;
    let len = in.prev_scale;
    let wl = length(in.lean);
    var slant = vec2<f32>(0.0);
    if (wl > 1e-4) {
        slant = in.lean / wl * (0.3 * down * len * min(wl * 4.0, 1.0));
    }
    let world = vec3<f32>(in.pos.x * in.scale + slant.x, in.pos.y * in.scale + slant.y, in.pos.z * len) + in.ipos;
    var out: VsOut;
    out.clip = globals.view_proj * vec4<f32>(world, 1.0);
    out.color = in.icolor;
    out.normal = in.normal;
    out.world = world;
    out.mat_w = 0.0;
    out.local = in.pos;
    out.tau = in.slim;
    out.info = vec4<f32>(-1.0, 0.0, 0.0, 0.0);
    return out;
}

// Ambient motes, animated entirely here from a per-mote phase (gloss):
// kind (slim) 0 pollen/dust drifting downwind, 1 blowing snow, 2 fireflies
// wandering and blinking, 3 leaves spiralling down.
@vertex
fn vs_particle(in: VsIn) -> VsOut {
    let t = globals.eye.w;
    let ph = in.gloss * 97.0;
    let kind = in.slim;
    let wind = vec3<f32>(in.lean, 0.0);
    var p = in.ipos;
    var fade = 1.0;
    if (kind < 0.5) {
        let f = fract(t * 0.09 + in.gloss);
        p = p + normalize(wind + vec3<f32>(1e-4, 0.0, 0.0)) * (f - 0.5) * 7.0 + vec3<f32>(0.0, 0.0, 0.25 * sin(t * 1.3 + ph));
        fade = sin(3.14159 * f);
    } else if (kind < 1.5) {
        let f = fract(t * 0.16 + in.gloss);
        p = p + vec3<f32>(wind.xy * f * 9.0 + 0.2 * vec2<f32>(sin(t * 2.0 + ph), cos(t * 1.7 + ph)), 1.2 - 2.4 * f);
        fade = sin(3.14159 * f);
    } else if (kind < 2.5) {
        p = p + vec3<f32>(sin(t * 0.43 + ph), cos(t * 0.37 + ph * 1.3), 0.4 * sin(t * 0.61 + ph * 0.7)) * 0.7;
        fade = pow(max(sin(t * 2.3 + ph * 3.1), 0.0), 3.0);
    } else {
        let f = fract(t * 0.11 + in.gloss);
        let spin = t * 2.5 + ph;
        p = p + vec3<f32>(0.35 * sin(spin) + wind.x * f * 3.0, 0.35 * cos(spin) + wind.y * f * 3.0, 0.8 - 2.0 * f);
        fade = sin(3.14159 * f);
    }
    let world = p + in.pos * in.scale;
    var out: VsOut;
    out.clip = globals.view_proj * vec4<f32>(world, 1.0);
    out.color = in.icolor;
    out.normal = in.normal;
    out.world = world;
    out.mat_w = 0.0;
    out.local = in.pos;
    out.tau = fade;
    out.info = vec4<f32>(kind, 0.0, 0.0, 0.0);
    return out;
}

// Depth-only pass rendered from the key light for a shadow cascade.
@vertex
fn vs_shadow(in: VsIn) -> @builtin(position) vec4<f32> {
    return globals.shadow_vp * vec4<f32>(world_of(in), 1.0);
}

// ---------------------------------------------------------------- noise

fn hash2(p: vec2<f32>) -> f32 {
    // Fold large world coordinates first so sin() keeps its precision.
    let q = p - 289.0 * floor(p / 289.0);
    return fract(sin(dot(q, vec2<f32>(127.1, 311.7))) * 43758.5453);
}

fn hash22(p: vec2<f32>) -> vec2<f32> {
    let q = p - 289.0 * floor(p / 289.0);
    return fract(sin(vec2<f32>(dot(q, vec2<f32>(127.1, 311.7)), dot(q, vec2<f32>(269.5, 183.3)))) * 43758.5453);
}

// Single-octave bilinear value noise.
fn vnoise(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    let a = hash2(i);
    let b = hash2(i + vec2<f32>(1.0, 0.0));
    let c = hash2(i + vec2<f32>(0.0, 1.0));
    let d = hash2(i + vec2<f32>(1.0, 1.0));
    return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
}

fn fbm2(p: vec2<f32>) -> f32 {
    return 0.62 * vnoise(p) + 0.38 * vnoise(p * 2.3 + vec2<f32>(5.2, 1.3));
}

// Cellular (Worley) noise: x = distance to the nearest feature, y = the
// gap to the second nearest (0 on cell edges), z = the nearest cell's id.
fn cells(p: vec2<f32>) -> vec3<f32> {
    let i = floor(p);
    let f = fract(p);
    var d1 = 8.0;
    var d2 = 8.0;
    var id = 0.0;
    for (var y = -1; y <= 1; y++) {
        for (var x = -1; x <= 1; x++) {
            let g = vec2<f32>(f32(x), f32(y));
            let o = hash22(i + g);
            let r = g + o - f;
            let d = dot(r, r);
            if (d < d1) {
                d2 = d1;
                d1 = d;
                id = hash2(i + g + vec2<f32>(7.0, 3.0));
            } else if (d < d2) {
                d2 = d;
            }
        }
    }
    return vec3<f32>(sqrt(d1), sqrt(d2) - sqrt(d1), id);
}

// --------------------------------------------------------------- shadows

fn cascade_uv(m: mat4x4<f32>, world: vec3<f32>) -> vec3<f32> {
    let sp = m * vec4<f32>(world, 1.0);
    return vec3<f32>(sp.xy * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5, 0.5), sp.z);
}

// Key-light visibility, 0..1. Both cascades are sampled UNCONDITIONALLY
// (textureSampleCompare demands uniform control flow) and picked by select:
// the near cascade wherever it covers, the far one elsewhere, fully lit
// outside both.
fn shadow_at(world: vec3<f32>) -> f32 {
    let a = cascade_uv(globals.light_vp0, world);
    let b = cascade_uv(globals.light_vp1, world);
    let sa = textureSampleCompare(shadow_near, shadow_smp, a.xy, a.z - 0.0015);
    let sb = textureSampleCompare(shadow_far, shadow_smp, b.xy, b.z - 0.0025);
    let in_a = all(a.xy >= vec2<f32>(0.02)) && all(a.xy <= vec2<f32>(0.98));
    let in_b = all(b.xy >= vec2<f32>(0.0)) && all(b.xy <= vec2<f32>(1.0));
    return select(select(1.0, sb, in_b), sa, in_a);
}

// ------------------------------------------------------------------ sky

// Sky radiance along a direction (also what water reflects).
fn sky_color(dir: vec3<f32>) -> vec3<f32> {
    let day = globals.sun_color.w;
    let heat = clamp(globals.heat, 0.0, 1.0);
    let oc = clamp(globals.overcast, 0.0, 1.0);
    let up = max(dir.z, 0.0);
    // Daytime gradient: deep zenith blue to a pale horizon matching the
    // distance haze (so terrain fades seamlessly into the sky).
    var zenith = mix(vec3<f32>(0.20, 0.40, 0.72), vec3<f32>(0.42, 0.52, 0.66), heat * 0.6);
    let horizon = globals.haze.rgb;
    zenith = mix(zenith, vec3<f32>(0.46, 0.48, 0.52), oc * 0.7);
    var col = mix(horizon, zenith, pow(up, 0.55));
    // Below the horizon: the hazy far ground.
    col = mix(col, horizon * 0.8, smoothstep(0.0, -0.08, dir.z));
    // Sunrise/sunset glow around the sun's azimuth when it is low.
    let sun = normalize(globals.sun_sky.xyz);
    let low = 1.0 - smoothstep(0.0, 0.35, sun.z);
    let toward = max(dot(normalize(vec3<f32>(dir.xy, 0.0) + vec3<f32>(1e-4, 0.0, 0.0)), normalize(vec3<f32>(sun.xy, 0.0) + vec3<f32>(1e-4, 0.0, 0.0))), 0.0);
    let glow = low * smoothstep(-0.25, 0.1, sun.z) * pow(toward, 3.0) * exp(-up * 5.0) * (1.0 - globals.sun_sky.w);
    col = col + vec3<f32>(1.0, 0.45, 0.2) * glow * 0.9;
    // Night: deep blue with stars.
    let night_col = mix(vec3<f32>(0.035, 0.05, 0.10), vec3<f32>(0.01, 0.015, 0.04), pow(up, 0.5));
    col = mix(night_col, col, day);
    return col * mix(0.55, 1.0, globals.light * 0.5 + 0.5);
}

@vertex
fn vs_sky(@builtin(vertex_index) i: u32) -> VsOut {
    // One oversized triangle covering the screen.
    let uv = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    let ndc = uv * 2.0 - 1.0;
    var out: VsOut;
    out.clip = vec4<f32>(ndc, 0.0, 1.0);
    out.color = vec3<f32>(0.0);
    out.normal = vec3<f32>(0.0, 0.0, 1.0);
    out.world = vec3<f32>(0.0);
    out.mat_w = 0.0;
    out.local = vec3<f32>(ndc, 0.0);
    out.tau = 0.0;
    out.info = vec4<f32>(0.0);
    return out;
}

@fragment
fn fs_sky(in: VsOut) -> @location(0) vec4<f32> {
    let ndc = in.local.xy;
    let dir = normalize(globals.cam_fwd.xyz + ndc.x * globals.cam_right.xyz + ndc.y * globals.cam_up.xyz);
    var col = sky_color(dir);
    let day = globals.sun_color.w;
    let sun = normalize(globals.sun_sky.xyz);
    let trail = clamp(globals.sun_sky.w, 0.0, 1.0);
    let above = smoothstep(-0.02, 0.02, dir.z);
    // Stars (a hashed sparse field on the celestial sphere), twinkling,
    // only at night.
    let cell = floor(dir * 180.0);
    let star = step(0.9965, hash2(cell.xy + cell.z * 17.0)) * (0.6 + 0.4 * sin(globals.eye.w * 3.0 + cell.x));
    col = col + vec3<f32>(0.9, 0.92, 1.0) * star * (1.0 - day) * above * 1.2;
    // The moon, opposite the sun, at night.
    let moon = normalize(vec3<f32>(-sun.x, -sun.y, abs(sun.z) + 0.35));
    let md = acos(clamp(dot(dir, moon), -1.0, 1.0));
    col = col + vec3<f32>(0.85, 0.88, 1.0) * (1.0 - smoothstep(0.018, 0.022, md)) * (1.0 - day) * 1.4;
    // The sun: a crisp over-bright disc with a halo — or, as time flies,
    // smeared into the luminous band of its daily arc (a long exposure):
    // the band follows the sun's declination around the celestial pole,
    // so it rides high in summer and low in winter.
    let r = globals.celestial.z;
    let sd = acos(clamp(dot(dir, sun), -1.0, 1.0));
    let disc = (1.0 - smoothstep(r * 0.85, r, sd)) * (1.0 - trail) * above;
    let halo = exp(-sd * 9.0) * 0.5 * (1.0 - trail) * above;
    let lat = globals.celestial.y;
    let pole = vec3<f32>(0.0, cos(lat), sin(lat));
    let dec = asin(clamp(dot(dir, pole), -1.0, 1.0));
    let band = exp(-pow((dec - globals.celestial.x) / (r * 0.9), 2.0)) * trail * above;
    let oc = clamp(globals.overcast, 0.0, 1.0);
    let sun_rgb = mix(vec3<f32>(1.0, 0.95, 0.85), vec3<f32>(1.0, 0.6, 0.3), 1.0 - smoothstep(0.0, 0.3, sun.z));
    col = col + sun_rgb * (disc * 7.0 + halo + band * 1.6) * (1.0 - 0.8 * oc);
    return vec4<f32>(col, 1.0);
}

// ------------------------------------------------------------- lighting

// Everything lit by the sun/moon: base albedo, shading normal, position,
// shadow visibility, ambient occlusion, foliage translucency, and the
// specular sharpness/strength.
fn shade(base: vec3<f32>, n: vec3<f32>, world: vec3<f32>, lit_in: f32, ao: f32, foliage: f32, spec_pow: f32, spec_k: f32) -> vec3<f32> {
    let day = globals.sun_color.w;
    let lit = mix(0.42, 1.0, lit_in);
    let heat = clamp(globals.heat, 0.0, 1.0);
    let oc = clamp(globals.overcast, 0.0, 1.0);
    let l = normalize(globals.sun_dir.xyz);
    // Hemisphere ambient: cool sky from above, warm soil bounce from below
    // — fading to a dim blue at night.
    let hemi_day = mix(vec3<f32>(0.30, 0.26, 0.22), vec3<f32>(0.42, 0.44, 0.48), n.z * 0.5 + 0.5);
    let hemi_night = mix(vec3<f32>(0.05, 0.05, 0.07), vec3<f32>(0.11, 0.14, 0.23), n.z * 0.5 + 0.5);
    let hemi = mix(hemi_night, hemi_day, day) * (1.0 - 0.3 * oc) * ao;
    // Heat: a warmer, more glaring sun; overcast: dimmer, cooler, flatter.
    let warm = mix(vec3<f32>(0.92, 0.96, 1.0), vec3<f32>(1.0, 0.84, 0.58), heat);
    let key_col = globals.sun_color.rgb * mix(warm, vec3<f32>(0.78, 0.84, 0.95), oc);
    let key = key_col * max(dot(n, l), 0.0) * mix(0.58, 0.98, heat) * lit * (1.0 - 0.75 * oc);
    // Leaves glow when the sun shines through them from behind.
    let v = normalize(globals.eye.xyz - world);
    let through = pow(max(dot(-v, l), 0.0), 4.0) * (0.3 + 0.7 * lit_in) * foliage;
    let trans = key_col * base * through * 0.9 * (1.0 - 0.7 * oc);
    // A lightning strike lights its surroundings: a blue-white point
    // light at the strike, fading within ~14 world units — never the
    // whole scene.
    let fd = length(world.xy - globals.fog.zw);
    let flash = vec3<f32>(0.85, 0.9, 1.0) * clamp(globals.sun_dir.w, 0.0, 1.0) * 0.9 * exp(-(fd * fd) / 196.0);
    let lightv = (hemi + key) * globals.light + flash;
    // View-dependent terms: a Blinn-Phong sheen and a fresnel rim (the
    // rim sells curvature on cones and trunks).
    let h = normalize(v + l);
    let spec = pow(max(dot(n, h), 0.0), spec_pow) * spec_k * lit * globals.light * dot(key_col, vec3<f32>(0.33));
    let fres = pow(1.0 - clamp(dot(n, v), 0.0, 1.0), 3.0);
    let rim = fres * 0.2 * globals.light * mix(0.25, 1.0, day);
    return base * lightv + trans * globals.light + vec3<f32>(1.0, 0.97, 0.90) * spec + vec3<f32>(0.55, 0.62, 0.70) * rim;
}

// Bark grain by kind: furrowed ridges, birch's white with dark lenticel
// dashes, pine/spruce plates, and smooth fissured grey. Returns the normal
// perturbation (xyz) and an albedo multiplier (w).
fn bark(kind: f32, world: vec3<f32>, n: vec3<f32>) -> vec4<f32> {
    let around = (world.x + world.y) * 7.0;
    let p = vec2<f32>(around, world.z * 2.2);
    if (kind < 0.5) {
        let b0 = vnoise(p);
        let bx = vnoise(p + vec2<f32>(0.9, 0.0));
        let bz = vnoise(p + vec2<f32>(0.0, 0.9));
        let g = vec3<f32>((b0 - bx) * n.y, (b0 - bx) * -n.x, (b0 - bz) * 0.6) * 0.75;
        return vec4<f32>(g, 0.85 + 0.3 * b0);
    } else if (kind < 1.5) {
        // Birch: horizontal lenticels — short dark dashes around the trunk.
        let q = vec2<f32>(around * 0.6, world.z * 14.0);
        let dash = step(0.78, vnoise(q)) * step(0.35, vnoise(q * vec2<f32>(0.3, 1.0) + vec2<f32>(3.0, 0.0)));
        return vec4<f32>(0.0, 0.0, (vnoise(q) - 0.5) * 0.2, 1.0 - 0.75 * dash);
    } else if (kind < 2.5) {
        // Plates: cellular scales with dark cracks between them.
        let c = cells(vec2<f32>(around * 0.45, world.z * 3.5));
        let crack = 1.0 - smoothstep(0.02, 0.12, c.y);
        let g = vec3<f32>(n.y, -n.x, 0.3) * (c.z - 0.5) * 0.4;
        return vec4<f32>(g, (0.85 + 0.3 * c.z) * (1.0 - 0.55 * crack));
    }
    // Smooth, finely fissured.
    let f = vnoise(vec2<f32>(around * 1.5, world.z * 0.8));
    return vec4<f32>(vec3<f32>(n.y, -n.x, 0.0) * (f - 0.5) * 0.25, 0.9 + 0.2 * f);
}

// Distance haze (aerial perspective: clear after rain, milky in heat and
// dust, dim under overcast) and valley fog pooling below the fog line.
fn atmosphere(c: vec3<f32>, world: vec3<f32>) -> vec3<f32> {
    let dist = length(globals.eye.xyz - world);
    let hz = 1.0 - exp(-dist * globals.haze.a);
    let day = globals.sun_color.w;
    var col = mix(c, globals.haze.rgb * mix(0.12, 1.0, day), hz);
    let fog = globals.fog.y * (1.0 - smoothstep(globals.fog.x - 3.0, globals.fog.x, world.z));
    col = mix(col, vec3<f32>(0.86, 0.88, 0.90) * globals.light * mix(0.15, 1.0, day), clamp(fog, 0.0, 1.0) * 0.85);
    return col;
}

fn lit_object(in: VsOut) -> vec3<f32> {
    var n = normalize(in.normal);
    let lit_in = shadow_at(in.world);
    var base = in.color;
    let up = clamp(n.z, 0.0, 1.0);
    // Canopy grain on up-facing foliage; bark on trunks by kind; log grain
    // on house walls and stumps.
    if (in.info.x >= 0.0) {
        let b = bark(in.info.x, in.world, n);
        n = normalize(n + b.xyz);
        base = base * b.w;
    } else if (in.info.z > 0.5) {
        let g = vnoise(vec2<f32>(in.world.x * 1.5 + in.world.y * 1.5, in.world.z * 22.0));
        let seam = 1.0 - smoothstep(0.0, 0.2, abs(fract(in.world.z * 9.0) - 0.5) * 2.0 - 0.75);
        n = normalize(n + vec3<f32>(0.0, 0.0, (g - 0.5) * 0.6));
        base = base * (0.85 + 0.25 * g) * (1.0 - 0.3 * seam);
    } else {
        let fg = 3.1;
        let e = 0.22;
        let g0 = vnoise(in.world.xy * fg);
        let gx = vnoise((in.world.xy + vec2<f32>(e, 0.0)) * fg);
        let gy = vnoise((in.world.xy + vec2<f32>(0.0, e)) * fg);
        n = normalize(n + vec3<f32>((g0 - gx) / e, (g0 - gy) / e, 0.0) * (0.13 * up));
    }
    // Self-occlusion: the lower parts of plants sit in their own shade.
    let ao = mix(0.7, 1.0, smoothstep(0.0, 1.2, in.info.w));
    return shade(base, n, in.world, lit_in, ao, in.info.y, 26.0, 0.12 + 0.22 * clamp(globals.heat, 0.0, 1.0));
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return vec4<f32>(atmosphere(lit_object(in), in.world), 1.0);
}

// ---------------------------------------------------------------- terrain

// The terrain: one vertex per tile from a static buffer (position, normal,
// material weights, occlusion/flow/relief), plus that tile's per-frame
// ground data from the ground instance stream stepped per VERTEX.
struct TerrainIn {
    @location(0) pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) mat_a: vec4<f32>,
    @location(3) mat_b: vec4<f32>,
    @location(4) extra: vec4<f32>,
    // Per-frame ground data (scene::Instance, stepped per vertex).
    @location(5) gpos: vec3<f32>,
    @location(6) snow: f32,
    @location(7) canopy_ao: f32,
    @location(8) color: vec3<f32>,
    @location(9) water: f32,
    @location(10) cover: vec2<f32>,
    @location(11) gloss: f32,
};

struct TerrainOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec3<f32>,
    @location(3) mat_a: vec4<f32>,
    @location(4) mat_b: vec4<f32>,
    // Static occlusion, downslope xy, column drop.
    @location(5) extra: vec4<f32>,
    // Snow, canopy occlusion, water, gloss.
    @location(6) ground: vec4<f32>,
    // Grass cover, burning.
    @location(7) cover: vec2<f32>,
};

@vertex
fn vs_terrain(in: TerrainIn) -> TerrainOut {
    var out: TerrainOut;
    out.clip = globals.view_proj * vec4<f32>(in.pos, 1.0);
    out.world = in.pos;
    out.normal = in.normal;
    out.color = in.color;
    out.mat_a = in.mat_a;
    out.mat_b = in.mat_b;
    out.extra = in.extra;
    out.ground = vec4<f32>(in.snow, in.canopy_ao, in.water, in.gloss);
    out.cover = in.cover;
    return out;
}

@vertex
fn vs_terrain_shadow(in: TerrainIn) -> @builtin(position) vec4<f32> {
    return globals.shadow_vp * vec4<f32>(in.pos, 1.0);
}

// Hex columns (the stepped, platformer surface): the same per-tile
// terrain and ground data, read per INSTANCE, on a hex prism mesh.
struct ColumnIn {
    @location(0) pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) mat_a: vec4<f32>,
    @location(3) mat_b: vec4<f32>,
    @location(4) extra: vec4<f32>,
    @location(5) gpos: vec3<f32>,
    @location(6) snow: f32,
    @location(7) canopy_ao: f32,
    @location(8) color: vec3<f32>,
    @location(9) water: f32,
    @location(10) cover: vec2<f32>,
    @location(11) gloss: f32,
    @location(12) mpos: vec3<f32>,
    @location(13) mnormal: vec3<f32>,
};

@vertex
fn vs_column(in: ColumnIn) -> TerrainOut {
    // Walls reach down only as far as the lowest neighbor (extra.w).
    let world = in.pos + vec3<f32>(in.mpos.xy, in.mpos.z * in.extra.w);
    var out: TerrainOut;
    out.clip = globals.view_proj * vec4<f32>(world, 1.0);
    out.world = world;
    out.normal = in.mnormal;
    out.color = in.color;
    out.mat_a = in.mat_a;
    out.mat_b = in.mat_b;
    // Column walls read as their soil profile: no water or snow on them.
    let top = step(0.5, in.mnormal.z);
    out.extra = vec4<f32>(in.extra.x, in.extra.yz, in.extra.w);
    out.ground = vec4<f32>(in.snow * top, in.canopy_ao, in.water * top, in.gloss * top);
    out.cover = in.cover * top;
    return out;
}

@vertex
fn vs_column_shadow(in: ColumnIn) -> @builtin(position) vec4<f32> {
    return globals.shadow_vp * vec4<f32>(in.pos + vec3<f32>(in.mpos.xy, in.mpos.z * in.extra.w), 1.0);
}

// A material's surface height at p (0..1-ish, world units scaled by the
// caller) — the bump field whose gradient perturbs the normal.
fn mat_height(m: i32, p: vec2<f32>) -> f32 {
    switch m {
        case 0: {
            // Wetland: soft mud; puddles are flat.
            let puddle = smoothstep(0.52, 0.6, vnoise(p * 0.5));
            return mix(fbm2(p * 1.6) * 0.6, 0.25, puddle);
        }
        case 1: {
            // Tundra: frost-heave polygons with raised rims.
            let c = cells(p * 0.55);
            return (1.0 - smoothstep(0.0, 0.16, c.y)) * 0.8 + vnoise(p * 3.0) * 0.2;
        }
        case 2: {
            // Boreal floor: lumpy moss cushions.
            return fbm2(p * 1.4) * 0.9;
        }
        case 3: {
            // Temperate floor: overlapping leaf litter.
            let c = cells(p * 3.2);
            return c.z * 0.5 + (1.0 - smoothstep(0.0, 0.1, c.y)) * -0.3;
        }
        case 4: {
            // Grassland: fine grain and tussock bases.
            return vnoise(p * 5.0) * 0.4 + vnoise(p * 1.5) * 0.3;
        }
        case 5: {
            // Savanna: dry laterite grain and termite mounds.
            let c = cells(p * 0.3);
            let mound = 1.0 - smoothstep(0.0, 0.12, c.x);
            return vnoise(p * 4.0) * 0.35 + mound * 1.4;
        }
        case 6: {
            // Desert: wind ripples, crests across the wind, meandering.
            let w = normalize(vec2<f32>(0.93, 0.37));
            let s = dot(p, w) * 7.0 + vnoise(p * 0.7) * 3.0;
            let f = fract(s / 6.2831);
            let r = select((1.0 - f) / 0.3, f / 0.7, f < 0.7);
            return r * 0.6 + vnoise(p * 0.4) * 0.2;
        }
        default: {
            // Rock: faceted blocks with deep joints.
            let c = cells(p * 0.9);
            return c.z * 0.6 + smoothstep(0.0, 0.12, c.y) * 0.6;
        }
    }
}

// Bump strength per material (world-unit relief of the height field).
fn mat_relief(m: i32) -> f32 {
    var r = array<f32, 8>(0.18, 0.35, 0.3, 0.22, 0.18, 0.3, 0.28, 0.6);
    return r[m];
}

// Albedo multiplier per material at p (the ground's own coloring).
fn mat_tint(m: i32, p: vec2<f32>, h: f32) -> vec3<f32> {
    switch m {
        case 0: {
            let puddle = smoothstep(0.52, 0.6, vnoise(p * 0.5));
            return mix(vec3<f32>(0.85, 0.95, 0.9), vec3<f32>(0.45, 0.55, 0.6), puddle);
        }
        case 1: {
            // Lichen: orange and pale green patches on grey-brown.
            let l = vnoise(p * 2.5);
            let orange = step(0.8, l);
            let pale = step(l, 0.2);
            return mix(mix(vec3<f32>(0.95, 0.93, 0.9), vec3<f32>(1.4, 0.9, 0.5), orange), vec3<f32>(0.95, 1.15, 0.95), pale) * (0.85 + 0.3 * h);
        }
        case 2: {
            let moss = smoothstep(0.35, 0.65, h);
            return mix(vec3<f32>(1.1, 0.8, 0.6), vec3<f32>(0.55, 0.9, 0.5), moss);
        }
        case 3: {
            let c = cells(p * 3.2);
            let leaf = mix(vec3<f32>(1.2, 0.8, 0.45), vec3<f32>(1.15, 0.55, 0.3), c.z);
            return mix(leaf, vec3<f32>(0.9, 0.85, 0.5), step(0.8, c.z));
        }
        case 4: {
            return mix(vec3<f32>(1.05, 1.0, 0.8), vec3<f32>(0.9, 1.05, 0.8), vnoise(p * 0.8));
        }
        case 5: {
            return vec3<f32>(1.35, 0.85, 0.62) * (0.9 + 0.2 * h);
        }
        case 6: {
            return vec3<f32>(1.45, 1.25, 0.95) * (0.92 + 0.16 * h);
        }
        default: {
            // Rock strata: bands by height, lighter on block faces.
            return vec3<f32>(1.0, 0.97, 0.94) * (0.8 + 0.35 * h);
        }
    }
}

// Specular strength per material (wet mud and ice shine; sand glints).
fn mat_spec(m: i32) -> f32 {
    var r = array<f32, 8>(0.5, 0.15, 0.05, 0.05, 0.05, 0.08, 0.2, 0.18);
    return r[m];
}

struct Ground {
    grad: vec2<f32>,
    tint: vec3<f32>,
    spec: f32,
};

// One material at a point: bump gradient (from three height samples),
// albedo tint, and specular. Rock and ripples use a parallax offset near
// the viewer so their relief holds up up close.
fn material(m: i32, p_in: vec2<f32>, v: vec3<f32>, detail: f32) -> Ground {
    var p = p_in;
    let relief = mat_relief(m);
    let par = select(0.0, 1.0, m == 7 || m == 6 || m == 1) * detail * globals.cam_fwd.w;
    if (par > 0.0) {
        let h0 = mat_height(m, p);
        p = p - v.xy / max(v.z, 0.25) * (h0 - 0.5) * relief * 0.35 * par;
    }
    let e = 0.08;
    let h = mat_height(m, p);
    let hx = mat_height(m, p + vec2<f32>(e, 0.0));
    let hy = mat_height(m, p + vec2<f32>(0.0, e));
    var g: Ground;
    // Bump fades with distance so far ground doesn't shimmer.
    g.grad = vec2<f32>(h - hx, h - hy) / e * relief * mix(0.25, 1.0, detail);
    g.tint = mat_tint(m, p, h);
    g.spec = mat_spec(m);
    return g;
}

fn terrain_color(in: TerrainOut) -> vec3<f32> {
    let lit_in = shadow_at(in.world);
    let v = normalize(globals.eye.xyz - in.world);
    let dist = length(globals.eye.xyz - in.world);
    let detail = 1.0 - smoothstep(20.0, 140.0, dist);
    var n = normalize(in.normal);
    let p = in.world.xy;
    // Material weights, jittered by noise so ecotones are ragged, not
    // straight hex-scale gradients; pick the two strongest.
    var w = array<f32, 8>(in.mat_a.x, in.mat_a.y, in.mat_a.z, in.mat_a.w, in.mat_b.x, in.mat_b.y, in.mat_b.z, in.mat_b.w);
    let jit = (fbm2(p * 0.45) - 0.5) * 0.5;
    var m1 = 0;
    var m2 = 1;
    for (var k = 0; k < 8; k++) {
        w[k] = w[k] + jit * (f32(k & 1) * 2.0 - 1.0) * w[k];
    }
    if (w[1] > w[0]) {
        m1 = 1;
        m2 = 0;
    }
    for (var k = 2; k < 8; k++) {
        if (w[k] > w[m1]) {
            m2 = m1;
            m1 = k;
        } else if (w[k] > w[m2]) {
            m2 = k;
        }
    }
    let a = material(m1, p, v, detail);
    let b = material(m2, p, v, detail);
    let t = smoothstep(-0.12, 0.12, w[m2] - w[m1]) ;
    var grad = mix(a.grad, b.grad, t);
    var tint = mix(a.tint, b.tint, t);
    var spec = mix(a.spec, b.spec, t);
    // Grass cover softens the bump into a sward.
    grad = grad * (1.0 - 0.5 * in.cover.x);
    var base = in.color * mix(vec3<f32>(1.0), tint, mix(0.55, 0.85, detail) * (1.0 - 0.6 * in.cover.x));

    // Drought cracks on parched open ground (gloss < 0).
    let parched = max(-in.ground.w, 0.0) * clamp(n.z, 0.0, 1.0);
    let cr = cells(p * 1.3);
    base = base * (1.0 - parched * (1.0 - smoothstep(0.02, 0.07, cr.y)) * 0.5);

    // Snow: smooth drifts over the ground's relief, with sparkle.
    let snow = clamp(in.ground.x, 0.0, 1.0);
    let drift = vnoise(p * 0.6);
    let snow_cover = smoothstep(0.15, 0.6, snow + (drift - 0.5) * 0.3);
    grad = mix(grad, vec2<f32>(vnoise(p * 0.6) - vnoise(p * 0.6 + vec2<f32>(0.3, 0.0)), 0.0) * 0.4, snow_cover);
    base = mix(base, vec3<f32>(0.93, 0.95, 0.99), snow_cover);
    spec = mix(spec, 0.6, snow_cover);
    n = normalize(n + vec3<f32>(grad, 0.0));

    // Occlusion: valleys (static) and under canopies (per frame).
    let ao = clamp(in.extra.x, 0.5, 1.1) * in.ground.y;
    let wet = max(in.ground.w, 0.0);
    var col = shade(base, n, in.world, lit_in, ao, 0.0, mix(24.0, 90.0, max(wet, snow_cover)), spec + 1.2 * wet);
    // Snow glints: sparse sun-facing crystals near the viewer.
    let glint = step(0.985, hash2(floor(p * 40.0))) * snow_cover * detail * lit_in * globals.sun_color.w;
    col = col + vec3<f32>(3.0, 3.0, 2.8) * glint * max(dot(reflect(-v, n), normalize(globals.sun_dir.xyz)), 0.0);

    // Open water: rippling along the flow, reflecting the sky with a
    // fresnel falloff, a sun glint, and foam at the banks.
    let water = clamp(in.ground.z, 0.0, 1.0);
    if (water > 0.05) {
        let flow = in.extra.yz;
        let t = globals.eye.w;
        let q = p * 2.2 - flow * t * 0.8;
        let e = 0.1;
        let r0 = vnoise(q) + 0.5 * vnoise(q * 2.7 + vec2<f32>(t * 0.3, 0.0));
        let rx = vnoise(q + vec2<f32>(e, 0.0)) + 0.5 * vnoise((q + vec2<f32>(e, 0.0)) * 2.7 + vec2<f32>(t * 0.3, 0.0));
        let ry = vnoise(q + vec2<f32>(0.0, e)) + 0.5 * vnoise((q + vec2<f32>(0.0, e)) * 2.7 + vec2<f32>(t * 0.3, 0.0));
        let wn = normalize(vec3<f32>((r0 - rx) / e * 0.08, (r0 - ry) / e * 0.08, 1.0));
        let fres = 0.04 + 0.96 * pow(1.0 - clamp(dot(wn, v), 0.0, 1.0), 5.0);
        let refl = sky_color(reflect(-v, wn));
        let body = in.color * shade(vec3<f32>(1.0), wn, in.world, lit_in, 1.0, 0.0, 200.0, 0.0);
        let hs = normalize(v + normalize(globals.sun_dir.xyz));
        let glint_w = pow(max(dot(wn, hs), 0.0), 300.0) * 4.0 * lit_in * globals.sun_color.w * (1.0 - globals.overcast);
        var wcol = mix(body, refl, fres) + globals.sun_color.rgb * glint_w;
        // Foam where the water thins toward the bank: soft flecks, faded
        // with distance so they don't alias into streaks.
        let bank = 1.0 - smoothstep(0.35, 0.8, water);
        let foam = bank * smoothstep(0.55, 0.75, vnoise(q * 1.4 + vec2<f32>(t * 0.5, 0.0))) * detail;
        wcol = mix(wcol, vec3<f32>(0.9, 0.93, 0.95) * globals.sun_color.w + 0.05, foam * 0.7);
        col = mix(col, wcol, smoothstep(0.05, 0.45, water));
    }

    // Embers glow on their own.
    col = col + in.color * in.cover.y * 0.8;

    // The hex grid overlay (a toggle): thin lines on tile edges.
    if (globals.celestial.w > 0.5) {
        let qf = (0.57735 * p.x - p.y / 3.0);
        let rf = 2.0 / 3.0 * p.y;
        let sf = -qf - rf;
        var rq = round(qf);
        var rr = round(rf);
        let rs = round(sf);
        let dq = abs(rq - qf);
        let dr = abs(rr - rf);
        let ds = abs(rs - sf);
        if (dq > dr && dq > ds) {
            rq = -rr - rs;
        } else if (dr > ds) {
            rr = -rq - rs;
        }
        let c = vec2<f32>(1.7320508 * (rq + rr / 2.0), 1.5 * rr);
        let d = p - c;
        // Distance to the hex edge along the three edge normals.
        let e1 = abs(d.x) / 0.8660254;
        let e2 = abs(0.5 * d.x + 0.8660254 * d.y) / 0.8660254;
        let e3 = abs(-0.5 * d.x + 0.8660254 * d.y) / 0.8660254;
        let edge = max(e1, max(e2, e3));
        let line = smoothstep(0.93, 0.985, edge) * (1.0 - smoothstep(60.0, 200.0, dist));
        col = mix(col, vec3<f32>(0.05, 0.05, 0.04), line * 0.45);
    }
    return col;
}

@fragment
fn fs_terrain(in: TerrainOut) -> @location(0) vec4<f32> {
    return vec4<f32>(atmosphere(terrain_color(in), in.world), 1.0);
}

// The roots view: the ground as tinted glass over the root systems.
@fragment
fn fs_terrain_xray(in: TerrainOut) -> @location(0) vec4<f32> {
    return vec4<f32>(atmosphere(terrain_color(in), in.world), 0.32);
}

// --------------------------------------------------------- translucents

// Rain and snow shafts: falling streaks (fast dashes for rain, slow
// drifting flakes for snow) over a faint veil; virga thins out and
// vanishes before reaching the ground.
@fragment
fn fs_rain(in: VsOut) -> @location(0) vec4<f32> {
    let reach = in.color.x;
    let dark = in.color.y;
    let snow = in.color.z;
    let down = -in.local.z;
    let end = 1.0 - smoothstep(reach * 0.65, reach, down);
    let top = smoothstep(0.0, 0.04, down);
    let r = length(in.local.xy);
    let edge = 1.0 - smoothstep(0.65, 1.0, r);
    let t = globals.eye.w;
    let phase = vnoise(in.world.xy * 3.1) * 7.0;
    let mask = step(0.42, vnoise(in.world.xy * 5.3 + vec2<f32>(3.7, 1.9)));
    let v = fract(in.world.z * mix(0.35, 1.3, snow) + t * mix(3.2, 0.45, snow) + phase);
    let dash = smoothstep(0.0, 0.06, v) * (1.0 - smoothstep(0.06, 0.32, v));
    let flake = 1.0 - smoothstep(0.0, 0.08, abs(v - 0.5));
    let streak = mix(dash, flake, snow) * mask;
    let alpha = in.tau * end * top * edge * (0.85 * streak + 0.14);
    if (alpha < 0.01) {
        discard;
    }
    let day = globals.sun_color.w;
    let col = mix(vec3<f32>(0.62, 0.66, 0.72) * (1.0 - 0.45 * dark), vec3<f32>(0.97, 0.98, 1.0), snow) * globals.light * mix(0.15, 1.0, day);
    return vec4<f32>(atmosphere(col, in.world), alpha);
}

// Motes: fireflies glow (emissive, bloom), everything else is lit simply.
@fragment
fn fs_particle(in: VsOut) -> @location(0) vec4<f32> {
    let kind = in.info.x;
    let day = globals.sun_color.w;
    var col = in.color * (mix(0.12, 1.0, day) * globals.light);
    var a = in.tau * 0.9;
    if (kind > 1.5 && kind < 2.5) {
        col = in.color * 2.2 * in.tau;
        a = in.tau;
    }
    if (a < 0.02) {
        discard;
    }
    return vec4<f32>(col, a);
}

// Cloud mesh radius at scale 1 (both the puffy and the sheet mesh).
const CLOUD_R: f32 = 8.0;

// Beer–Lambert: a cloud passes e^(−τ) of the light behind it. Optical
// depth is greatest at the core and thins toward a ragged, noise-broken
// edge; thin genera (cirrus, τ < 1) also break into fibrous streaks.
@fragment
fn fs_cloud(in: VsOut) -> @location(0) vec4<f32> {
    let lit = shade(in.color, normalize(in.normal), in.world, shadow_at(in.world), 1.0, 0.0, 26.0, 0.1);
    let r = length(in.local.xy) / CLOUD_R;
    let ragged = vnoise(in.local.xy * 0.9 + in.world.xy * 0.05) - 0.5;
    let core = 1.0 - smoothstep(0.30, 1.0, r + 0.35 * ragged);
    let thin = clamp(1.0 - in.tau / 3.0, 0.0, 1.0);
    let fibers = vnoise(in.local.xy * vec2<f32>(0.35, 2.6));
    let streak = mix(1.0, 0.25 + 1.5 * fibers, thin);
    let tau = in.tau * core * streak;
    let alpha = 1.0 - exp(-tau);
    // Silver lining: thin, backlit edges glow as sunlight scatters forward
    // through them.
    let v = normalize(globals.eye.xyz - in.world);
    let back = pow(max(dot(-v, normalize(globals.sun_sky.xyz)), 0.0), 3.0);
    let lining = vec3<f32>(1.0, 0.96, 0.88) * back * (1.0 - core) * 1.4 * globals.light * globals.sun_color.w;
    // Drop near-invisible fringes so they don't write depth and cut holes
    // in the clouds behind them.
    if (alpha < 0.04) {
        discard;
    }
    return vec4<f32>(atmosphere(lit + lining, in.world), alpha);
}
