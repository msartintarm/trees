// The transport-agnostic heart of the app: loads the wasm module, builds the
// Simulation and Renderer, runs the requestAnimationFrame loop
// (advance → render → snapshot), and routes every Control through one
// exhaustive applyControl switch. Runs identically inside the engine worker
// (OffscreenCanvas) or inline on the main thread — the caller only supplies
// callbacks.

import type { EngineModule, Renderer, Sim } from "./engineTypes.ts";
import { BRUSH_CODES, type Control, type InitConfig, type StatsSnapshot } from "./protocol.ts";

export type ReadyInfo = { backend: string; seed: number };

export type SessionCallbacks = {
  onReady: (r: ReadyInfo) => void;
  onFrame: (f: { snapshot: StatsSnapshot }) => void;
  onFatal: (message: string) => void;
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

function snapshot(sim: Sim): StatsSnapshot {
  const [bare, grass, trees, burning, storms] = sim.counts();
  return {
    tick: sim.tick(),
    bare,
    grass,
    trees,
    burning: burning ?? 0,
    storms: storms ?? 0,
    sun: sim.sun(),
    moisture: sim.moisture(),
    playing: sim.is_playing(),
    selectedSpeed: sim.selected_speed(),
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
  let last = performance.now();
  let rafId = 0;

  const frame = (t: number): void => {
    if (disposed) return;
    // Clamp a background-tab hiatus so the sim glides instead of lurching.
    const dt = Math.min(0.25, Math.max(0, (t - last) / 1000));
    last = t;
    try {
      sim.advance(dt);
      renderer.render(
        sim.view_proj(),
        sim.alpha(),
        sim.light_level(),
        sim.eye(),
        sim.ground_instances(),
        sim.ground_instance_count(),
        sim.tree_instances(0),
        sim.tree_instance_count(0),
        sim.tree_instances(1),
        sim.tree_instance_count(1),
        sim.tree_instances(2),
        sim.tree_instance_count(2),
        sim.tree_instances(3),
        sim.tree_instance_count(3),
        sim.grass_instances(),
        sim.grass_instance_count(),
        sim.mushroom_instances(),
        sim.mushroom_instance_count(),
        sim.cloud_instances(),
        sim.cloud_instance_count(),
        sim.bolt_instances(),
        sim.bolt_instance_count(),
      );
      cb.onFrame({ snapshot: snapshot(sim) });
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
          p.seedTreeP,
          p.seedGrassP,
        );
        break;
      }
      case "paint":
        sim.paint_at(c.bx, c.by, BRUSH_CODES[c.brush], c.species ?? 0);
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
