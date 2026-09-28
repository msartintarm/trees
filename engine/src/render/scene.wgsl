// One pipeline for everything: instanced meshes with per-vertex normals and a
// per-instance position/scale/color. Growth animation happens here — the
// vertex stage lerps prev_scale → scale by the clock alpha, scaling z fully
// and xy partially so plants read as shooting up before filling out.
//
// Lighting: hemisphere ambient (sky above, warm ground bounce below) + a
// directional sun gated by a shadow map rendered from the sun's view, plus a
// procedural bump term — value-noise normal perturbation in world space, so
// soil and canopies get surface grain without any UVs or textures.

struct Globals {
    view_proj: mat4x4<f32>,
    // Orthographic sun-view matrix for the shadow map.
    light_vp: mat4x4<f32>,
    sun_dir: vec4<f32>,
    alpha: f32,
    // Global illumination level from the climate's sun signal (1 = neutral).
    light: f32,
    // Heat, 0..1: warmer, brighter, more glaring sunlight when hot.
    heat: f32,
    _pad2: f32,
    // Camera position, for specular and fresnel rim.
    eye: vec4<f32>,
};

@group(0) @binding(0) var<uniform> globals: Globals;
@group(1) @binding(0) var shadow_tex: texture_depth_2d;
@group(1) @binding(1) var shadow_smp: sampler_comparison;

struct VsIn {
    @location(0) pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) vcolor: vec3<f32>,
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
};

fn world_with_form(in: VsIn, slim: f32) -> vec3<f32> {
    let s = mix(in.prev_scale, in.scale, globals.alpha);
    let sxy = 0.3 + 0.7 * s;
    // Shade-avoidance form: crowded youth = taller, narrower.
    let h = 1.0 + 0.45 * slim;
    let w = 1.0 - 0.35 * slim;
    let z = in.pos.z * s * h;
    // Phototropic lean: the crown leans out over open ground, more the
    // higher up the stem.
    return vec3<f32>(in.pos.x * sxy * w + in.lean.x * z, in.pos.y * sxy * w + in.lean.y * z, z) + in.ipos;
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
    @location(3) shadow_pos: vec3<f32>,
    // Vertex material weight: 1 = baked material (trunks, stems), 0 = crown.
    @location(4) mat_w: f32,
    // Clouds only: mesh-local position (for the edge falloff) and the
    // genus' central optical depth τ. Zero elsewhere.
    @location(5) local: vec3<f32>,
    @location(6) tau: f32,
};

@vertex
fn vs_main(in: VsIn) -> VsOut {
    let world = world_of(in);
    var out: VsOut;
    out.clip = globals.view_proj * vec4<f32>(world, 1.0);
    out.color = mix(in.icolor, in.vcolor, in.vweight);
    out.normal = in.normal;
    out.world = world;
    out.mat_w = in.vweight;
    let sp = globals.light_vp * vec4<f32>(world, 1.0);
    out.shadow_pos = vec3<f32>(sp.xy * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5, 0.5), sp.z);
    out.local = vec3<f32>(0.0);
    out.tau = 0.0;
    return out;
}

// Clouds: the instance's `slim` slot carries optical depth τ instead of a
// growth form, so geometry is built with no form distortion.
@vertex
fn vs_cloud(in: VsIn) -> VsOut {
    let world = cloud_world(in);
    var out: VsOut;
    out.clip = globals.view_proj * vec4<f32>(world, 1.0);
    out.color = mix(in.icolor, in.vcolor, in.vweight);
    var d = in.lean;
    if (length(d) < 1e-3) {
        d = vec2<f32>(1.0, 0.0);
    } else {
        d = normalize(d);
    }
    out.normal = vec3<f32>(in.normal.x * d.x - in.normal.y * d.y, in.normal.x * d.y + in.normal.y * d.x, in.normal.z);
    out.world = world;
    out.mat_w = in.vweight;
    let sp = globals.light_vp * vec4<f32>(world, 1.0);
    out.shadow_pos = vec3<f32>(sp.xy * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5, 0.5), sp.z);
    out.local = in.pos;
    out.tau = in.slim;
    return out;
}

// Depth-only pass rendered from the sun for the shadow map.
@vertex
fn vs_shadow(in: VsIn) -> @builtin(position) vec4<f32> {
    return globals.light_vp * vec4<f32>(world_of(in), 1.0);
}

fn hash2(p: vec2<f32>) -> f32 {
    return fract(sin(dot(p, vec2<f32>(127.1, 311.7))) * 43758.5453);
}

// Single-octave bilinear value noise for the bump term.
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

fn lit_color(in: VsOut) -> vec3<f32> {
    var n = normalize(in.normal);

    // Procedural bump, two domains blended by surface orientation:
    // up-facing surfaces sample soil/canopy clod noise in the ground plane;
    // steep surfaces sample bark grain — noise stretched along z so trunks
    // read as ridged bark rather than random speckle. Trunks and stems
    // (mat_w = 1) get the strongest grain.
    let up = clamp(n.z, 0.0, 1.0);
    let e = 0.22;
    // Ground-plane clods.
    let fg = 3.1;
    let g0 = vnoise(in.world.xy * fg);
    let gx = vnoise((in.world.xy + vec2<f32>(e, 0.0)) * fg);
    let gy = vnoise((in.world.xy + vec2<f32>(0.0, e)) * fg);
    let g_grad = vec3<f32>((g0 - gx) / e, (g0 - gy) / e, 0.0);
    // Bark: high frequency around the axis, low along it.
    let bark_p = vec2<f32>((in.world.x + in.world.y) * 7.0, in.world.z * 2.2);
    let b0 = vnoise(bark_p);
    let bx = vnoise(bark_p + vec2<f32>(0.9, 0.0));
    let bz = vnoise(bark_p + vec2<f32>(0.0, 0.9));
    let b_grad = vec3<f32>((b0 - bx) * n.y, (b0 - bx) * -n.x, (b0 - bz) * 0.6);
    let bark_strength = 0.30 + 0.45 * in.mat_w;
    let grad = g_grad * (0.13 * up) + b_grad * (bark_strength * (1.0 - up));
    n = normalize(n + grad);

    // Shadow map lookup (hardware PCF via the comparison sampler). Sampled
    // UNCONDITIONALLY: textureSampleCompare demands uniform control flow, so
    // no branch may guard it. That's safe here — the sun frustum covers the
    // whole world (clamp-to-edge for strays), and geometry above it (clouds)
    // produces a negative reference depth, which always compares lit.
    let bias = 0.0025;
    let cmp =
        textureSampleCompare(shadow_tex, shadow_smp, in.shadow_pos.xy, in.shadow_pos.z - bias);
    // The shadow frustum follows the camera: anything outside it reads lit
    // (select, not a branch — the sample above stays in uniform flow).
    let sp = in.shadow_pos.xy;
    let inside = all(sp >= vec2<f32>(0.0)) && all(sp <= vec2<f32>(1.0));
    // Shadows soften rather than blacken.
    let lit = mix(0.45, 1.0, select(1.0, cmp, inside));

    // Hemisphere ambient: cool sky from above, warm soil bounce from below.
    let hemi = mix(vec3<f32>(0.30, 0.26, 0.22), vec3<f32>(0.42, 0.44, 0.48), n.z * 0.5 + 0.5);
    // Heat: a stronger, warmer, more glaring sun (hot years read bright
    // and golden, cool ones pale and soft).
    let heat = clamp(globals.heat, 0.0, 1.0);
    let sun_color = mix(vec3<f32>(0.92, 0.96, 1.0), vec3<f32>(1.0, 0.84, 0.58), heat);
    let sun = max(dot(n, globals.sun_dir.xyz), 0.0) * mix(0.58, 0.98, heat) * lit;
    let shade = (hemi + sun_color * sun) * globals.light;

    // View-dependent terms: a broad Blinn-Phong sheen and a fresnel rim.
    // The rim is what sells curvature — cone and trunk silhouettes catch a
    // sliver of sky light exactly where a smooth surface would.
    let v = normalize(globals.eye.xyz - in.world);
    let h = normalize(v + globals.sun_dir.xyz);
    let spec = pow(max(dot(n, h), 0.0), 26.0) * (0.12 + 0.22 * heat) * lit * globals.light;
    let fres = pow(1.0 - clamp(dot(n, v), 0.0, 1.0), 3.0);
    let rim = fres * 0.20 * globals.light;
    return in.color * shade
        + vec3<f32>(1.0, 0.97, 0.90) * spec
        + vec3<f32>(0.55, 0.62, 0.70) * rim;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return vec4<f32>(lit_color(in), 1.0);
}

// The roots view: the ground as tinted glass over the root systems.
@fragment
fn fs_xray(in: VsOut) -> @location(0) vec4<f32> {
    return vec4<f32>(lit_color(in), 0.32);
}

// Cloud mesh radius at scale 1 (both the puffy and the sheet mesh).
const CLOUD_R: f32 = 8.0;

// Beer–Lambert: a cloud passes e^(−τ) of the light behind it. Optical
// depth is greatest at the core and thins toward a ragged, noise-broken
// edge; thin genera (cirrus, τ < 1) also break into fibrous streaks.
@fragment
fn fs_cloud(in: VsOut) -> @location(0) vec4<f32> {
    let r = length(in.local.xy) / CLOUD_R;
    let ragged = vnoise(in.local.xy * 0.9 + in.world.xy * 0.05) - 0.5;
    let core = 1.0 - smoothstep(0.30, 1.0, r + 0.35 * ragged);
    let thin = clamp(1.0 - in.tau / 3.0, 0.0, 1.0);
    let fibers = vnoise(in.local.xy * vec2<f32>(0.35, 2.6));
    let streak = mix(1.0, 0.25 + 1.5 * fibers, thin);
    let tau = in.tau * core * streak;
    let alpha = 1.0 - exp(-tau);
    // Drop near-invisible fringes so they don't write depth and cut holes
    // in the clouds behind them.
    if (alpha < 0.04) {
        discard;
    }
    return vec4<f32>(lit_color(in), alpha);
}
