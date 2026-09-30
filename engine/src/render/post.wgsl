// Post-processing: the scene renders into an HDR target; bright light
// (the sun disc, snow and water glints, fireflies, embers, lightning) is
// extracted and blurred at half resolution into bloom, then everything is
// tone-mapped to the screen with a soft highlight knee — colors authored in
// 0..1 pass through nearly unchanged — graded by the region's light and
// lightly vignetted.

struct Post {
    // xy = source texel size, zw = blur direction (in texels).
    step: vec4<f32>,
    // x = bloom strength (0 = off), y = exposure, z = vignette, w = bright
    // threshold.
    params: vec4<f32>,
    // Regional grade: an rgb multiplier (warm desert light, cool boreal
    // air) — w unused.
    grade: vec4<f32>,
};

@group(0) @binding(0) var<uniform> post: Post;
@group(0) @binding(1) var src: texture_2d<f32>;
@group(0) @binding(2) var smp: sampler;
@group(0) @binding(3) var bloom_tex: texture_2d<f32>;

struct FullOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_full(@builtin(vertex_index) i: u32) -> FullOut {
    let uv = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    var out: FullOut;
    out.clip = vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
    out.uv = vec2<f32>(uv.x, 1.0 - uv.y);
    return out;
}

fn luma(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
}

// Downsample with a 4-tap box and keep only what exceeds the threshold
// (soft knee, so bloom fades in rather than switching on).
@fragment
fn fs_bright(in: FullOut) -> @location(0) vec4<f32> {
    let t = post.step.xy;
    var c = textureSample(src, smp, in.uv + t * vec2<f32>(-0.5, -0.5)).rgb;
    c = c + textureSample(src, smp, in.uv + t * vec2<f32>(0.5, -0.5)).rgb;
    c = c + textureSample(src, smp, in.uv + t * vec2<f32>(-0.5, 0.5)).rgb;
    c = c + textureSample(src, smp, in.uv + t * vec2<f32>(0.5, 0.5)).rgb;
    c = c * 0.25;
    let l = luma(c);
    let th = post.params.w;
    let knee = th * 0.5;
    let soft = clamp(l - th + knee, 0.0, 2.0 * knee);
    let w = max(soft * soft / (4.0 * knee + 1e-4), l - th) / max(l, 1e-4);
    return vec4<f32>(c * w, 1.0);
}

// Separable 9-tap Gaussian along post.step.zw.
@fragment
fn fs_blur(in: FullOut) -> @location(0) vec4<f32> {
    let d = post.step.xy * post.step.zw;
    var c = textureSample(src, smp, in.uv).rgb * 0.227;
    c = c + textureSample(src, smp, in.uv + d * 1.385).rgb * 0.316;
    c = c + textureSample(src, smp, in.uv - d * 1.385).rgb * 0.316;
    c = c + textureSample(src, smp, in.uv + d * 3.231).rgb * 0.070;
    c = c + textureSample(src, smp, in.uv - d * 3.231).rgb * 0.070;
    return vec4<f32>(c, 1.0);
}

// Identity up to the knee, then an exponential shoulder toward 1.
fn shoulder(x: f32) -> f32 {
    let k = 0.78;
    if (x <= k) {
        return x;
    }
    return k + (1.0 - k) * (1.0 - exp(-(x - k) / (1.0 - k)));
}

@fragment
fn fs_composite(in: FullOut) -> @location(0) vec4<f32> {
    let scene = textureSample(src, smp, in.uv).rgb;
    let glow = textureSample(bloom_tex, smp, in.uv).rgb;
    var c = (scene + glow * post.params.x) * post.params.y * post.grade.rgb;
    c = vec3<f32>(shoulder(c.r), shoulder(c.g), shoulder(c.b));
    let d = in.uv - vec2<f32>(0.5);
    let vig = 1.0 - post.params.z * smoothstep(0.35, 0.85, length(d) * 1.3);
    return vec4<f32>(c * vig, 1.0);
}
