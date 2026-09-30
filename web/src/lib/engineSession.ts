// The transport-agnostic heart of the app: loads the wasm module, builds the
// Simulation and Renderer, runs the requestAnimationFrame loop
// (advance → render → snapshot), and routes every Control through one
// exhaustive applyControl switch. Runs identically inside the engine worker
// (OffscreenCanvas) or inline on the main thread — the caller only supplies
// callbacks.

import type { EngineModule, Renderer, Sim } from "./engineTypes.ts";
import { BRUSH_CODES, type Control, type InitConfig, type StatsSnapshot, type WalkSnapshot } from "./protocol.ts";

export type ReadyInfo = { backend: string; seed: number };

export type SessionCallbacks = {
  onReady: (r: ReadyInfo) => void;
  onFrame: (f: { snapshot: StatsSnapshot }) => void;
  onFatal: (message: string) => void;
  /** The tile inspector's report ("" when the click missed the map). */
  onInspect: (text: string) => void;
};

export type SessionHandle = { applyControl(c: Control): void; dispose(): void };

// Worker global scopes historically lack requestAnimationFrame, so drive the
// loop with it where present (vsync-paced) and a ~60 Hz timer otherwise.
const hasRaf = typeof requestAnimationFrame === "function";
const schedule = (cb: (t: number) => void): number =>
  hasRaf ? requestAnimationFrame(cb) : (setTimeout(() => cb(performance.now()), 16) as unknown as number);
const unschedule = (id: number): void => (hasRaf ? cancelAnimationFrame(id) : clearTimeout(id));

function assertNever(x: never): never {
  throw new Error(`unhandled control: ${JSON.stringify(x)}`);
}

// Fingerprint the build (`version.txt`, never cached) so a new build always
// beats the browser cache while an unchanged one keeps it.
async function fingerprint(basePath: string): Promise<string> {
  try {
    const r = await fetch(`${basePath}/wasm-pkg/version.txt`, { cache: "no-store" });
    if (r.ok) return `?v=${(await r.text()).trim()}`;
  } catch {}
  return "";
}

async function loadEngine(config: InitConfig): Promise<EngineModule> {
  const q = await fingerprint(config.basePath);
  const mod = (await import(
    /* webpackIgnore: true */ `${config.basePath}/wasm-pkg/engine.js${q}`
  )) as EngineModule;
  await mod.default({ module_or_path: `${config.basePath}/wasm-pkg/engine_bg.wasm${q}` });
  return mod;
}

function walkSnapshot(sim: Sim): WalkSnapshot | null {
  if (!sim.walking()) return null;
  const [years, dayFrac, daysPerSec, lapse, wood, wading, third, yearFrac] = sim.walk_status();
  return {
    years,
    dayFrac,
    daysPerSec,
    lapse,
    wood,
    wading: wading > 0,
    thirdPerson: third > 0,
    yearFrac,
    message: sim.walk_message(),
    target: sim.target_label(),
  };
}

function snapshot(sim: Sim, biomeView: boolean): StatsSnapshot {
  const [bare, grass, trees, burning, storms, infested, houses] = sim.counts();
  return {
    tick: sim.tick(),
    bare,
    grass,
    trees,
    burning: burning ?? 0,
    storms: storms ?? 0,
    infested: infested ?? 0,
    sun: sim.sun(),
    moisture: sim.moisture(),
    mast: sim.mast_year(),
    diversity: sim.diversity(),
    deathsRecent: Array.from(sim.deaths_recent()),
    clouds: Array.from(sim.cloud_counts()),
    biomeShares: Array.from(sim.biome_shares()),
    biomeSamples: biomeView ? Array.from(sim.biome_samples()) : [],
    cloudEvents: Array.from(sim.cloud_events()),
    heat: sim.heat(),
    localDiversity: sim.local_diversity(),
    flooding: sim.flooding(),
    houses: houses ?? 0,
    walk: walkSnapshot(sim),
    playing: sim.is_playing(),
    selectedSpeed: sim.selected_speed(),
    actualSpeed: sim.actual_speed(),
    gridWidth: sim.grid_width(),
    gridHeight: sim.grid_height(),
    seed: sim.seed(),
  };
}

export async function startEngineSession(
  canvas: HTMLCanvasElement | OffscreenCanvas,
  isOffscreen: boolean,
  config: InitConfig,
  cb: SessionCallbacks,
): Promise<SessionHandle> {
  const mod = await loadEngine(config);
  canvas.width = config.width;
  canvas.height = config.height;
  const sim: Sim = new mod.Simulation(config.seed);
  sim.set_viewport(config.width, config.height);
  const renderer: Renderer = isOffscreen
    ? await mod.Renderer.create_offscreen(canvas as OffscreenCanvas)
    : await mod.Renderer.create(canvas as HTMLCanvasElement);
  sim.play();
  cb.onReady({ backend: renderer.backend(), seed: sim.seed() });

  let disposed = false;
  let biomeView = false;
  let last = performance.now();
  // The renderer's terrain is re-uploaded whenever the engine rebuilds it
  // (reseed, landscape params, the ground materials drifting).
  let terrainVersion = -1;
  let rafId = 0;

  const frame = (t: number): void => {
    if (disposed) return;
    // Clamp a background-tab hiatus so the sim glides instead of lurching.
    const dt = Math.min(0.25, Math.max(0, (t - last) / 1000));
    last = t;
    try {
      sim.advance(dt);
      sim.prepare_frame();
      if (sim.terrain_version() !== terrainVersion) {
        terrainVersion = sim.terrain_version();
        renderer.set_terrain(
          sim.terrain_vertices(),
          sim.terrain_indices(),
          sim.terrain_chunks(),
          sim.skirt_vertices(),
          sim.skirt_indices(),
        );
      }
      renderer.render(sim.frame_uniforms(), sim.frame_bytes(), sim.frame_counts(), sim.render_flags());
      cb.onFrame({ snapshot: snapshot(sim, biomeView) });
    } catch (e) {
      disposed = true;
      cb.onFatal(String(e));
      return;
    }
    rafId = schedule(frame);
  };
  rafId = schedule(frame);

  const applyControl = (c: Control): void => {
    switch (c.type) {
      case "play":
        sim.play();
        break;
      case "pause":
        sim.pause();
        break;
      case "step":
        sim.single_step();
        break;
      case "speed":
        sim.set_speed(c.value);
        break;
      case "reseed":
        sim.reseed(c.seed);
        break;
      case "params": {
        const p = c.params;
        sim.set_params(
          p.grassSeedP,
          p.grassClonalP,
          p.shadeStrength,
          p.treeGrowthP,
          p.treeRange,
          p.treeMaturityAge,
          p.sodFactor,
          p.crowdingP,
          p.grassMeanLife,
          p.treeMeanLife,
          p.fireIgnitionP,
          p.fireSpreadP,
          p.nutrientBoost,
          p.stormRate,
          p.stormLightningP,
          p.climateSwing,
          p.waterTable,
          p.mutationRate,
          p.terrain,
          p.grassNiches,
          p.pestStrength,
          p.browse,
          p.competition,
          p.climateZones,
          p.rivers,
          p.grazing,
          p.physiology,
          p.seasons,
          p.cloudDynamics,
          p.biomes,
          p.seedTreeP,
          p.seedGrassP,
          p.width,
          p.height,
        );
        break;
      }
      case "paint":
        sim.paint_at(c.bx, c.by, BRUSH_CODES[c.brush], c.species ?? 0, c.grass ?? 0);
        break;
      case "orbit":
        sim.orbit(c.dyaw, c.dpitch);
        break;
      case "panBy":
        sim.pan_pixels(c.dx, c.dy);
        break;
      case "zoom":
        sim.zoom(c.factor);
        break;
      case "resetCamera":
        sim.reset_camera();
        break;
      case "inspect":
        cb.onInspect(sim.inspect_at(c.bx, c.by));
        break;
      case "rootsView":
        sim.set_roots_view(c.on);
        break;
      case "flashes":
        sim.set_flashes(c.on);
        break;
      case "walk":
        sim.enter_walk(c.bx, c.by);
        break;
      case "exitWalk":
        sim.exit_walk();
        break;
      case "keys":
        sim.set_keys(c.bits);
        break;
      case "look":
        sim.look(c.dyaw, c.dpitch);
        break;
      case "act":
        sim.act(c.action, c.species, c.grass);
        break;
      case "inspectTarget":
        cb.onInspect(sim.inspect_target());
        break;
      case "thirdPerson":
        sim.toggle_third_person();
        break;
      case "hexColumns":
        sim.set_hex_columns(c.on);
        break;
      case "landforms":
        sim.set_landforms(c.on);
        break;
      case "lightMode":
        sim.set_light_mode(c.mode);
        break;
      case "bloom":
        sim.set_bloom(c.on);
        break;
      case "detail":
        sim.set_detail(c.value);
        break;
      case "hexOverlay":
        sim.set_hex_overlay(c.on);
        break;
      case "biomeView":
        biomeView = c.on;
        sim.set_biome_view(c.on);
        break;
      case "resize":
        canvas.width = c.w;
        canvas.height = c.h;
        renderer.resize(c.w, c.h);
        sim.set_viewport(c.w, c.h);
        break;
      default:
        assertNever(c);
    }
  };

  return {
    applyControl,
    dispose: () => {
      disposed = true;
      unschedule(rafId);
      renderer.free();
      sim.free();
    },
  };
}
