# Tree Simulator

A hex-grid forest ecology simulation in the browser: a pure Rust sim core
compiled to WebAssembly, rendered in 3D with wgpu (WebGPU, WebGL2 fallback),
driven by a Next.js static-export shell. The architecture mirrors the sibling
`../traffic` project: everything numeric is a pure, natively-tested module;
everything browser-shaped is a thin gated layer around it.

## The rules (design in one screen)

The ecology models real grass–tree spatial dynamics; every mechanism below
is a `Params` field, live-editable in the panel:

| Rule | Mechanism |
| --- | --- |
| Grid | 64×64 pointy-top hexes (odd-r offset), ground at z = 0 |
| Tree seed rain | each **mature** tree deposits a distance kernel over its range disk — Janzen-Connell dip at d1, peak at d2, exponential tail; sources compound as `p = 1 − (1−p₁)^W`, capped at `SEED_RAIN_CAP`. Seed rain is per species: offspring inherit their parent's kind |
| Species | four varieties on distinct strategy corners (`world.rs::SPECIES_TABLE`, curated multipliers — not panel knobs). **Acacia**: baseline pioneer, drought-hardy. **Oak**: climax — slow, late-maturing, long-lived, shade-tolerant seedlings, early fireproof bark, heavy acorns (dispersal 2), space-hungry (4× crowding), slow-rotting nutrient-rich wood. **Pine**: fast, flammable past maturity, but 3× recruitment on ash (serotiny). **Willow**: booms on wet ground (water exponent 2), dies hard in drought, fast and short-lived |
| Maturity | trees younger than 40 ticks don't seed, shade, crowd, or resist fire |
| Light gate | canopy proximity scales **both** grass growth and tree recruitment (gap-phase regeneration); full block at d1 of a mature tree by default |
| Grass | creep-only by default: no spontaneous seeding, 8 %/tick per adjacent grass tile — swards advance as fronts from what you plant, and a locally extinct sward stays gone |
| Sod competition | tree establishment on grass ×0.4 (>1 = nurse-plant mode) |
| Self-thinning | >2 mature neighbors → extra 0.2 %/tick death per excess neighbor |
| Fire | grass is fuel: ignitions spread tile-to-tile through contiguous grass and saplings; mature trees survive and act as firebreaks. Background lightning off by default; the 🔥 brush, storm bolts, and fire presets supply ignition |
| Climate | two deterministic signals — ☀ sun and 💧 moisture — drift through wet years and droughts (shown in the HUD; `climate_swing` scales the amplitude). They multiply growth, mortality, flammability, and storm frequency, all calibrated to exactly 1 at the neutral climate. Droughts brown and wilt the whole map, dim the light, quadruple ignition; wet years flush green and brew storms |
| Mortality | a per-tick hazard (`weather stress / mean_life`), not a lifespan: lifetimes are unbounded geometrics, so a tree CAN live forever — but survival decays exponentially and in equilibrium very few grow old. Droughts are when most of the dying happens |
| Weather | thunderclouds spawn off-map and sweep across (on by default, more in wet seasons): the inner rain core soaks tiles — wet fuel can't catch, burning tiles are doused, wet soil grows faster — while lightning strikes anywhere under the cloud, so dry edge strikes can start fires the storm's own rain never reaches. Clouds drift smoothly overhead with real shadows; ⛈ in the HUD |
| Genesis | the world starts **empty** — you paint the founding trees and grass with the brushes (runs replay exactly from seed + params + click history) |
| Nutrient cycle | decomposition returns biomass to a per-tile fertility store (rotted tree ≫ thatch; charcoal keeps most carbon locked) and fire mineralizes an immediate ash flush; fertility multiplies establishment by up to `1 + nutrient_boost`, living plants draw the store down, idle soil leaches back. Rendered as the soil darkening toward loam |
| Death & decay | old age is telegraphed (plants wilt and shrink past 80 % of their lifespan); every death leaves a standing husk — grey snag, straw thatch, or charcoal — that **blocks regrowth** until saprotrophic mycelium finishes it. Colonization builds faster beside other colonized wood (inoculum proximity), mushrooms fruit on established wood, and charcoal resists rot ~4×, so burn snags linger and old burn scars break up fuel. Ash stains the ground for 60 ticks (visual only) |
| Clock | 10 ticks/s at 1×; speeds 0.5–32×, pause, single-step |

Runs stay reproducible from seed + params + click history (stateless
counter-based RNG). Note the hysteresis: fire presets need a Reseed to show
their regime — an established closed forest has too little contiguous fuel
to burn down, which is faithful to the real thing.

All randomness is a stateless SplitMix64 hash of `(seed, cell, tick, stream)`
(`sim/rng.rs`), so the same seed replays the same forest, and there is no RNG
state to save.

## Layout

```
engine/                 Rust crate → wasm (cdylib + rlib)
  src/sim/              pure core, cargo-testable natively
    rng.rs              counter-based stateless randomness
    clock.rs            fixed-dt play/pause/speed clock
    hex.rs              axial hex math, disks, picking, world bounds
    world.rs            cell states + the per-tick ecology rules
  src/render/           pure render math + WGSL + wasm-only GPU
    camera.rs           orbit perspective camera, ground-plane ray picking
    geometry.rs         baked meshes: tile prism, tree, grass tuft, base slab
    scene.rs            world → per-frame instance streams (prev/cur scale)
    scene.wgsl          the one pipeline (naga-validated under cargo test)
    gpu.rs              wgpu Renderer (wasm32 only; WebGPU or WebGL2)
  src/bridge.rs         wasm-bindgen Simulation — marshalling only
web/                    Next.js 15 static export
  scripts/build-wasm.mjs  wasm-pack driver with content-hash skip cache
  src/lib/              pure tested modules (node --test, no DOM/wasm)
    protocol.ts         typed main↔worker Control/FromWorker contract
    simParams.ts        ecology tunables: defaults, clamping, panel field specs
    camera.ts           backing-store sizing + gesture→control mappings
    hud.ts              snapshot → HUD strings
    engineSession.ts    wasm load + rAF loop + applyControl switch
    session.ts          worker(OffscreenCanvas)-or-inline façade
  src/worker/engineWorker.ts  thin relay around engineSession
  src/components/EngineCanvas.tsx  canvas, input, panel, HUD
```

Rendering/lighting: hemisphere ambient (cool sky above, warm bounce below) +
a directional sun gated by a 2048² shadow map rendered from the sun's view
(vegetation casts real shadows; clouds keep their soft analytic ones);
Blinn-Phong specular sheen and a fresnel rim light (the cue that makes the
16–18-segment cones read as smooth solids); and dual-domain procedural bump —
world-space value-noise normal perturbation, soil clods on up-facing
surfaces and anisotropic bark grain (stretched along the trunk axis) on
steep ones — i.e. bump/normal mapping without UVs or textures. The surface
prefers a non-sRGB format so authored colors aren't re-encoded brighter on
WebGPU than on WebGL. True ray tracing isn't available in browsers; the
shadow map covers its main visible benefit here.

Per-frame data crosses the wasm boundary as copied-out byte buffers
(`bytemuck`-cast `Instance` arrays); smooth growth comes from shipping the
previous and current scale and lerping in the shader by the clock's alpha —
the same model as traffic, no shared memory views.

## Run the tests (the fast inner loop)

```
cd engine && cargo test        # unit + whole-world integration tests, native
cd web && npm test             # protocol/camera/hud/simParams via node --test
```

`engine/tests/ecology.rs` asserts long-run *regimes* against measured bands:
the default savanna coexists on every seed, faster trees close into moist
forest, strong lightning is bistable by seed, the fire trap selects against
late maturity, and a clonal-free grassland matches the analytic birth–death
balance (~485 grass tiles once husk-blocked turnover is accounted for). The
bands come from the probe:

```
cargo run --release --example equilibrium    # sweep configs, print stats
```

Measured regimes (12k-tick runs, multiple seeds) — selectable in the panel's
Preset dropdown (`simParams.ts::PRESETS`; reads Custom once you hand-edit):

| Preset | Config | Long-run |
| --- | --- | --- |
| Savanna parkland (default) | tree growth 0.2 %, clonal 8 %, no base lightning | grass ≈ 630 under a mixed pioneer woodland (pine-led, acacia, oak groves, willow fringe) |
| Moist forest | tree growth 1 %, no base lightning | **succession**: pioneers dominate the young stand, then shade-tolerant oaks close a dense canopy (~1,900) and relegate them to relics |
| Fire-swept grassland | tree growth 0.1 %, lightning 0.05 % | the sward surges in wet years and burns in droughts; trees extinct |
| Fire lottery | defaults + lightning 0.05 % | fire keeps the oaks out: pure grassland or a pioneer woodland, by seed |

Regimes are measured from a "planted" scatter (2 % trees, 10 % grass — the
probe's stand-in for your brushwork); presets themselves start empty.

An emergent worth knowing: charcoal's rot resistance makes old burn scars
into firebreaks, so a fast-growing (1 %) forest is effectively fireproof —
fires fragment their own future fuel. Converting forest to grassland takes
*slow* trees plus frequent lightning, which is why the fire presets pair
them.

## Run it in the browser

```
cd web
npm install
npm run dev                    # builds the wasm first (cached by content hash)
```

Open http://localhost:3000 — click to plant, drag to orbit, shift/right-drag
to pan, wheel to zoom. `npm run build` produces the static export in `web/out`.

## Deliberate differences from traffic

No threads build or COOP/COEP shim, no serde/`import` feature, no GPU compute,
no Canvas-2D fallback renderer (wgpu's WebGL2 backend covers non-WebGPU
browsers, chosen via `new_instance_with_webgpu_detection` because a present
`navigator.gpu` does not guarantee a usable adapter), no ASCII view, no
map/tools pipeline.
