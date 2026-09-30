"use client";

// The entire UI: canvas + input handling + controls panel + HUD. Gestures are
// converted to plain-data Controls (deltas, or backing-store pixels for
// paints) and sent to the session — worker or inline, it doesn't care. HUD
// text is written imperatively into refs each frame rather than re-rendering
// React at 60 Hz.

import { useEffect, useRef, useState } from "react";

import { basePath } from "../lib/basePath.ts";
import {
  backingSize,
  clientToBacking,
  dragToOrbit,
  isClick,
  wheelZoomFactor,
  MAX_DPR,
} from "../lib/camera.ts";
import { cloudsText, deathsText, newestCloudEvent, statsText, walkText } from "../lib/hud.ts";
import {
  BIOMES,
  GRASS_NAMES,
  KEY_BITS,
  LIGHT_MODES,
  SPECIES_NAMES,
  WALK_ACTIONS,
  type Brush,
  type GrassKind,
  type StatsSnapshot,
  type TreeSpecies,
} from "../lib/protocol.ts";
import { createSession, type Session } from "../lib/session.ts";
import {
  displayValue,
  applyPreset,
  matchingPreset,
  withFieldValue,
  DEFAULT_PARAMS,
  PARAM_FIELDS,
  PRESETS,
  type ParamField,
  type SimParams,
} from "../lib/simParams.ts";
import styles from "./EngineCanvas.module.css";

// 0.05× shows the seasons (a year every two seconds); faster speeds show
// the annual picture.
const SPEEDS = [0.05, 0.5, 1, 2, 8, 32];

/** A click either paints with a brush or inspects the tile. */
type Tool = Brush | "inspect";
const DEFAULT_SEED = 7;
/** Mouse-look sensitivity, radians per pixel. */
const LOOK_SPEED = 0.0024;

/** Movement key bits held for a keyboard code. */
function keyBit(code: string): number {
  switch (code) {
    case "KeyW":
    case "ArrowUp":
      return KEY_BITS.forward;
    case "KeyS":
    case "ArrowDown":
      return KEY_BITS.back;
    case "KeyA":
    case "ArrowLeft":
      return KEY_BITS.left;
    case "KeyD":
    case "ArrowRight":
      return KEY_BITS.right;
    case "ShiftLeft":
    case "ShiftRight":
      return KEY_BITS.sprint;
    case "Space":
      return KEY_BITS.jump;
    case "KeyR":
      return KEY_BITS.rest;
    default:
      return 0;
  }
}

/** The Whittaker chart: sampled tiles plotted by site temperature (x) and
 * water (y), colored by biome. */
function drawWhittaker(canvas: HTMLCanvasElement | null, samples: number[]): void {
  const ctx = canvas?.getContext("2d");
  if (!canvas || !ctx) return;
  const { width: w, height: h } = canvas;
  ctx.fillStyle = "rgba(10, 14, 18, 0.9)";
  ctx.fillRect(0, 0, w, h);
  ctx.strokeStyle = "rgba(255, 255, 255, 0.25)";
  ctx.strokeRect(0.5, 0.5, w - 1, h - 1);
  for (let k = 0; k + 2 < samples.length; k += 3) {
    const [t, water, b] = [samples[k], samples[k + 1], samples[k + 2]];
    ctx.fillStyle = BIOMES[b]?.color ?? "#fff";
    ctx.fillRect(4 + t * (w - 8), h - 4 - Math.min(water, 1) * (h - 8), 2, 2);
  }
}

export default function EngineCanvas() {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const sessionRef = useRef<Session | null>(null);
  const bootedRef = useRef(false);
  const hudRef = useRef<HTMLDivElement | null>(null);
  const deathsRef = useRef<HTMLDivElement | null>(null);
  const weatherRef = useRef<HTMLDivElement | null>(null);
  const tickerRef = useRef<HTMLDivElement | null>(null);
  const [inspectText, setInspectText] = useState<string | null>(null);
  const [rootsView, setRootsView] = useState(false);
  const [biomeView, setBiomeView] = useState(false);
  const [flashes, setFlashes] = useState(true);
  const biomeViewRef = useRef(false);
  biomeViewRef.current = biomeView;
  const chartRef = useRef<HTMLCanvasElement | null>(null);
  const [biomeShares, setBiomeShares] = useState<number[]>([]);
  const snapshotRef = useRef<StatsSnapshot | null>(null);
  const walkHudRef = useRef<HTMLDivElement | null>(null);
  const targetRef = useRef<HTMLDivElement | null>(null);
  const messageRef = useRef<HTMLDivElement | null>(null);
  const [walking, setWalking] = useState(false);
  const walkingRef = useRef(false);
  walkingRef.current = walking;
  const [action, setAction] = useState(0);
  const actionRef = useRef(0);
  actionRef.current = action;
  const [hexColumns, setHexColumns] = useState(false);
  const [landforms, setLandforms] = useState(true);
  const [lightMode, setLightMode] = useState(0);
  const [bloom, setBloom] = useState(true);
  const [detail, setDetail] = useState(true);
  const [hexOverlay, setHexOverlay] = useState(false);

  const [error, setError] = useState<string | null>(null);
  const [backend, setBackend] = useState<string>("");
  const [playing, setPlaying] = useState(true);
  const [speed, setSpeed] = useState(1);
  const [brush, setBrush] = useState<Tool>("tree");
  const [treeSpecies, setTreeSpecies] = useState<TreeSpecies>(0);
  const [seed, setSeed] = useState(DEFAULT_SEED);
  const [params, setParams] = useState<SimParams>(DEFAULT_PARAMS);
  // Raw input text per field, so partially-typed values ("0.", "") don't
  // snap back mid-keystroke; params only update on parseable input.
  const [paramText, setParamText] = useState<Record<string, string>>(() =>
    Object.fromEntries(PARAM_FIELDS.map((f) => [f.key, String(displayValue(f, DEFAULT_PARAMS))])),
  );
  const brushRef = useRef<Tool>(brush);
  brushRef.current = brush;
  const speciesRef = useRef<TreeSpecies>(treeSpecies);
  speciesRef.current = treeSpecies;
  const [grassKind, setGrassKind] = useState<GrassKind>(0);
  const grassRef = useRef<GrassKind>(grassKind);
  grassRef.current = grassKind;

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas || bootedRef.current) return;
    // The OffscreenCanvas transfer is one-shot; the guard keeps React
    // StrictMode's dev double-mount from booting twice.
    bootedRef.current = true;

    const initial = backingSize(canvas.clientWidth, canvas.clientHeight, devicePixelRatio);
    let lastEvents: number[] | null = null;
    let sharesAt = 0;
    let tickerShownAt = 0;
    const session = createSession(
      canvas,
      { basePath: basePath(), width: initial.w, height: initial.h, seed: DEFAULT_SEED },
      {
        onReady: (r) => setBackend(r.backend),
        onFrame: (f) => {
          snapshotRef.current = f.snapshot;
          if (hudRef.current) hudRef.current.textContent = statsText(f.snapshot);
          if (deathsRef.current) deathsRef.current.textContent = deathsText(f.snapshot);
          if (weatherRef.current) weatherRef.current.textContent = cloudsText(f.snapshot);
          if (biomeViewRef.current && f.snapshot.biomeSamples.length) {
            drawWhittaker(chartRef.current, f.snapshot.biomeSamples);
            if (performance.now() - sharesAt > 1000) {
              sharesAt = performance.now();
              setBiomeShares(f.snapshot.biomeShares);
            }
          }
          // Weather ticker: announce each cloud transition for a few seconds.
          const event = newestCloudEvent(lastEvents, f.snapshot.cloudEvents);
          lastEvents = f.snapshot.cloudEvents;
          if (event && tickerRef.current) {
            tickerRef.current.textContent = event;
            tickerShownAt = performance.now();
          } else if (tickerRef.current && performance.now() - tickerShownAt > 4000) {
            tickerRef.current.textContent = "";
          }
          setPlaying((p) => (p === f.snapshot.playing ? p : f.snapshot.playing));
          const w = f.snapshot.walk;
          setWalking((was) => (was === (w !== null) ? was : w !== null));
          if (w) {
            if (walkHudRef.current) walkHudRef.current.textContent = walkText(w);
            if (targetRef.current) targetRef.current.textContent = w.target ? `▸ ${w.target}` : "";
            if (messageRef.current) messageRef.current.textContent = w.message;
          }
        },
        onFatal: (message) => setError(message),
        onInspect: (text) => setInspectText(text || null),
      },
    );
    sessionRef.current = session;

    // ---- resize ----
    let lastSize = initial;
    const ro = new ResizeObserver(() => {
      const s = backingSize(canvas.clientWidth, canvas.clientHeight, devicePixelRatio);
      if (s.w === lastSize.w && s.h === lastSize.h) return;
      lastSize = s;
      session.applyControl({ type: "resize", w: s.w, h: s.h });
    });
    ro.observe(canvas);

    // ---- pointer gestures ----
    // Left-drag orbits, shift/middle/right-drag pans, wheel zooms, and a
    // left press that never leaves the click slop paints on release.
    let active: {
      id: number;
      button: number;
      pan: boolean;
      startX: number;
      startY: number;
      lastX: number;
      lastY: number;
      dragged: boolean;
    } | null = null;

    const dprScale = () => {
      const rect = canvas.getBoundingClientRect();
      return rect.width > 0 ? lastSize.w / rect.width : Math.min(devicePixelRatio, MAX_DPR);
    };

    const onPointerDown = (e: PointerEvent) => {
      if (walkingRef.current) {
        // Walking: the first click captures the mouse for looking; then
        // left acts and right inspects.
        if (document.pointerLockElement !== canvas) {
          canvas.requestPointerLock?.();
          return;
        }
        if (e.button === 0) {
          const a = WALK_ACTIONS[actionRef.current];
          session.applyControl({ type: "act", action: a.code, species: speciesRef.current, grass: grassRef.current });
        } else if (e.button === 2) {
          session.applyControl({ type: "inspectTarget" });
        }
        return;
      }
      if (active) return;
      canvas.setPointerCapture(e.pointerId);
      active = {
        id: e.pointerId,
        button: e.button,
        pan: e.button === 1 || e.button === 2 || e.shiftKey,
        startX: e.clientX,
        startY: e.clientY,
        lastX: e.clientX,
        lastY: e.clientY,
        dragged: false,
      };
    };

    const onPointerMove = (e: PointerEvent) => {
      if (walkingRef.current) {
        if (document.pointerLockElement === canvas && (e.movementX || e.movementY)) {
          session.applyControl({ type: "look", dyaw: -e.movementX * LOOK_SPEED, dpitch: -e.movementY * LOOK_SPEED });
        }
        return;
      }
      if (!active || e.pointerId !== active.id) return;
      const dx = e.clientX - active.lastX;
      const dy = e.clientY - active.lastY;
      active.lastX = e.clientX;
      active.lastY = e.clientY;
      if (!active.dragged && isClick(e.clientX - active.startX, e.clientY - active.startY)) return;
      active.dragged = true;
      if (active.pan) {
        const k = dprScale();
        session.applyControl({ type: "panBy", dx: dx * k, dy: dy * k });
      } else {
        session.applyControl({ type: "orbit", ...dragToOrbit(dx, dy) });
      }
    };

    const onPointerUp = (e: PointerEvent) => {
      if (!active || e.pointerId !== active.id) return;
      const wasClick = !active.dragged && active.button === 0 && !active.pan;
      active = null;
      if (!wasClick) return;
      const rect = canvas.getBoundingClientRect();
      const { bx, by } = clientToBacking(e.clientX, e.clientY, rect, lastSize);
      const tool = brushRef.current;
      if (tool === "inspect") {
        session.applyControl({ type: "inspect", bx, by });
        return;
      }
      session.applyControl({
        type: "paint",
        bx,
        by,
        brush: tool,
        species: speciesRef.current,
        grass: grassRef.current,
      });
    };

    const onWheel = (e: WheelEvent) => {
      e.preventDefault();
      if (walkingRef.current) return;
      session.applyControl({ type: "zoom", factor: wheelZoomFactor(e.deltaY) });
    };

    const onContextMenu = (e: Event) => e.preventDefault();

    canvas.addEventListener("pointerdown", onPointerDown);
    canvas.addEventListener("pointermove", onPointerMove);
    canvas.addEventListener("pointerup", onPointerUp);
    canvas.addEventListener("pointercancel", onPointerUp);
    canvas.addEventListener("wheel", onWheel, { passive: false });
    canvas.addEventListener("contextmenu", onContextMenu);

    // ---- walking keys ----
    let held = 0;
    const onKey = (down: boolean) => (e: KeyboardEvent) => {
      if (!walkingRef.current) return;
      const target = e.target as HTMLElement | null;
      if (target && (target.tagName === "INPUT" || target.tagName === "SELECT")) return;
      if (down && !e.repeat) {
        const n = Number(e.key);
        if (n >= 1 && n <= WALK_ACTIONS.length) {
          setAction(n - 1);
          return;
        }
        if (e.code === "KeyE") {
          const a = WALK_ACTIONS[actionRef.current];
          session.applyControl({ type: "act", action: a.code, species: speciesRef.current, grass: grassRef.current });
          return;
        }
        if (e.code === "KeyF") {
          session.applyControl({ type: "inspectTarget" });
          return;
        }
        if (e.code === "KeyV") {
          session.applyControl({ type: "thirdPerson" });
          return;
        }
        if (e.code === "KeyQ") {
          document.exitPointerLock?.();
          session.applyControl({ type: "exitWalk" });
          return;
        }
      }
      const bit = keyBit(e.code);
      if (!bit) return;
      e.preventDefault();
      const next = down ? held | bit : held & ~bit;
      if (next !== held) {
        held = next;
        session.applyControl({ type: "keys", bits: held });
      }
    };
    const onKeyDown = onKey(true);
    const onKeyUp = onKey(false);
    // Losing focus drops every held key (no runaway walking).
    const onBlur = () => {
      if (held !== 0) {
        held = 0;
        session.applyControl({ type: "keys", bits: 0 });
      }
    };
    window.addEventListener("keydown", onKeyDown);
    window.addEventListener("keyup", onKeyUp);
    window.addEventListener("blur", onBlur);

    return () => {
      window.removeEventListener("keydown", onKeyDown);
      window.removeEventListener("keyup", onKeyUp);
      window.removeEventListener("blur", onBlur);
      ro.disconnect();
      canvas.removeEventListener("pointerdown", onPointerDown);
      canvas.removeEventListener("pointermove", onPointerMove);
      canvas.removeEventListener("pointerup", onPointerUp);
      canvas.removeEventListener("pointercancel", onPointerUp);
      canvas.removeEventListener("wheel", onWheel);
      canvas.removeEventListener("contextmenu", onContextMenu);
      // The session (and its one-shot transferred canvas) outlives StrictMode
      // remounts on purpose; the page owns exactly one.
    };
  }, []);

  const send = (c: Parameters<Session["applyControl"]>[0]) => sessionRef.current?.applyControl(c);

  const togglePlay = () => {
    send({ type: playing ? "pause" : "play" });
    setPlaying(!playing);
  };

  const pickSpeed = (v: number) => {
    setSpeed(v);
    send({ type: "speed", value: v });
  };

  const editParam = (field: ParamField, text: string) => {
    setParamText((t) => ({ ...t, [field.key]: text }));
    const v = Number(text);
    if (text.trim() === "" || !Number.isFinite(v)) return;
    const next = withFieldValue(field, params, v);
    setParams(next);
    send({ type: "params", params: next });
  };

  // Snap the input text back to the clamped stored value when editing ends.
  const normalizeParam = (field: ParamField) => {
    setParamText((t) => ({ ...t, [field.key]: String(displayValue(field, params)) }));
  };

  const pickPreset = (key: string) => {
    const preset = PRESETS.find((p) => p.key === key);
    if (!preset) return;
    const next = applyPreset(preset, params);
    setParams(next);
    setParamText(Object.fromEntries(PARAM_FIELDS.map((f) => [f.key, String(displayValue(f, next))])));
    send({ type: "params", params: next });
  };

  const activePreset = matchingPreset(params);

  const enterWalk = () => {
    const c = canvasRef.current;
    const w = c?.width ?? 0;
    const h = c?.height ?? 0;
    send({ type: "walk", bx: w / 2, by: h / 2 });
    setWalking(true);
    c?.requestPointerLock?.();
  };

  const exitWalk = () => {
    document.exitPointerLock?.();
    send({ type: "exitWalk" });
    setWalking(false);
  };

  const toggle = (on: boolean, set: (v: boolean) => void, control: (on: boolean) => Parameters<Session["applyControl"]>[0]) => {
    set(!on);
    send(control(!on));
  };

  return (
    <div className={styles.wrap}>
      <canvas ref={canvasRef} className={styles.canvas} />
      <div className={styles.panel}>
        <p className={styles.title}>Tree Simulator {backend && <span className={styles.hint}>({backend})</span>}</p>
        <div className={styles.row}>
          <button onClick={togglePlay}>{playing ? "⏸ Pause" : "▶ Play"}</button>
          <button onClick={() => send({ type: "step" })}>Step</button>
          <button onClick={() => send({ type: "resetCamera" })}>Reset view</button>
        </div>
        <div className={styles.row}>
          {walking ? (
            <button className={styles.active} onClick={exitWalk} title="back to the overview (Q)">
              🦅 Overview
            </button>
          ) : (
            <button onClick={enterWalk} title="walk the world as a long-lived wanderer: time flows as you move">
              🚶 Walk
            </button>
          )}
        </div>
        <div className={styles.row}>
          <span className={styles.label}>Ground</span>
          <button
            className={hexColumns ? styles.active : ""}
            title="hex columns (stepped, platformer terraces) or a smooth surface"
            onClick={() => toggle(hexColumns, setHexColumns, (on) => ({ type: "hexColumns", on }))}
          >
            ⬢ Columns
          </button>
          <button
            className={landforms ? styles.active : ""}
            title="exaggerated biome landforms: dunes, mesas, jagged peaks, tundra hummocks"
            onClick={() => toggle(landforms, setLandforms, (on) => ({ type: "landforms", on }))}
          >
            ⛰ Landforms
          </button>
          <button
            className={hexOverlay ? styles.active : ""}
            title="draw the hex grid on the ground"
            onClick={() => toggle(hexOverlay, setHexOverlay, (on) => ({ type: "hexOverlay", on }))}
          >
            ⬡ Grid
          </button>
        </div>
        <div className={styles.row}>
          <span className={styles.label}>Light</span>
          {LIGHT_MODES.map((name, k) => (
            <button
              key={name}
              className={k === lightMode ? styles.active : ""}
              title={k === 0 ? "follow the time of day (walking); the sun smears into its daily arc as time flies" : undefined}
              onClick={() => {
                setLightMode(k);
                send({ type: "lightMode", mode: k });
              }}
            >
              {name}
            </button>
          ))}
        </div>
        <div className={styles.row}>
          <span className={styles.label}>Quality</span>
          <button
            className={bloom ? styles.active : ""}
            title="bloom: bright light glows (sun, glints, fireflies)"
            onClick={() => toggle(bloom, setBloom, (on) => ({ type: "bloom", on }))}
          >
            ✨ Bloom
          </button>
          <button
            className={detail ? styles.active : ""}
            title="close-up parallax relief on rock, sand and tundra"
            onClick={() => {
              setDetail(!detail);
              send({ type: "detail", value: detail ? 0 : 1 });
            }}
          >
            🔎 Detail
          </button>
        </div>
        <div className={styles.row}>
          <span className={styles.label}>Speed</span>
          {SPEEDS.map((v) => (
            <button
              key={v}
              className={v === speed ? styles.active : ""}
              title={v < 0.1 ? "slow enough to see the seasons" : undefined}
              onClick={() => pickSpeed(v)}
            >
              {v < 0.1 ? "🍂" : `${v}×`}
            </button>
          ))}
        </div>
        <div className={styles.row}>
          <span className={styles.label}>Brush</span>
          {(["fire", "clear", "inspect"] as Tool[]).map((b) => (
            <button key={b} className={b === brush ? styles.active : ""} onClick={() => setBrush(b)}>
              {b === "fire" ? "🔥 Fire" : b === "clear" ? "✕ Clear" : "🔍 Inspect"}
            </button>
          ))}
        </div>
        <div className={styles.row}>
          <span className={styles.label}>View</span>
          <button
            className={rootsView ? styles.active : ""}
            title="show root systems under glass ground; blue = reaching groundwater"
            onClick={() => {
              send({ type: "rootsView", on: !rootsView });
              setRootsView(!rootsView);
            }}
          >
            🌱 Roots
          </button>
          <button
            className={biomeView ? styles.active : ""}
            title="tint the ground by climate biome, with a legend and a Whittaker chart"
            onClick={() => {
              send({ type: "biomeView", on: !biomeView });
              setBiomeView(!biomeView);
            }}
          >
            🗺 Biomes
          </button>
          <button
            className={flashes ? styles.active : ""}
            title="lightning flashes (a local glow around each strike); turn off to avoid any flashing"
            onClick={() => {
              send({ type: "flashes", on: !flashes });
              setFlashes(!flashes);
            }}
          >
            ⚡ Flashes
          </button>
        </div>
        <div className={styles.row}>
          <span className={styles.label}>Grass</span>
          {GRASS_NAMES.map((name, k) => (
            <button
              key={name}
              className={brush === "grass" && grassKind === k ? styles.active : ""}
              onClick={() => {
                setBrush("grass");
                setGrassKind(k as GrassKind);
              }}
            >
              {name}
            </button>
          ))}
        </div>
        <div className={styles.row}>
          <span className={styles.label}>Tree</span>
          {SPECIES_NAMES.map((name, k) => (
            <button
              key={name}
              className={brush === "tree" && treeSpecies === k ? styles.active : ""}
              onClick={() => {
                setBrush("tree");
                setTreeSpecies(k as TreeSpecies);
              }}
            >
              {name}
            </button>
          ))}
        </div>
        <div className={styles.row}>
          <span className={styles.label}>Seed</span>
          <input
            type="number"
            value={seed}
            onChange={(e) => setSeed(Number(e.target.value) >>> 0)}
          />
          <button onClick={() => send({ type: "reseed", seed })}>Reseed</button>
        </div>
        <details className={styles.params}>
          <summary>Parameters</summary>
          <label className={styles.paramRow}>
            <span>Preset</span>
            <select
              className={styles.preset}
              value={activePreset ?? "custom"}
              onChange={(e) => pickPreset(e.target.value)}
            >
              {PRESETS.map((p) => (
                <option key={p.key} value={p.key}>
                  {p.label}
                </option>
              ))}
              {activePreset === null && (
                <option value="custom" disabled>
                  Custom
                </option>
              )}
            </select>
          </label>
          <div className={styles.hint}>
            {activePreset === null
              ? "custom mix"
              : `long-run: ${PRESETS.find((p) => p.key === activePreset)?.hint}`}
          </div>
          {PARAM_FIELDS.map((f) => (
            <label key={f.key} className={styles.paramRow}>
              <span>
                {f.label}
                {f.appliesOnReseed && <em title="takes effect on the next Reseed">*</em>}
              </span>
              <input
                type="number"
                min={f.min}
                max={f.max}
                step={f.step}
                value={paramText[f.key] ?? ""}
                onChange={(e) => editParam(f, e.target.value)}
                onBlur={() => normalizeParam(f)}
              />
            </label>
          ))}
          <div className={styles.hint}>* applies on Reseed</div>
        </details>
        <div className={styles.hint}>
          the world starts empty — paint life with the brushes · click: plant ·
          drag: orbit · shift/right-drag: pan · wheel: zoom
        </div>
      </div>
      {walking && (
        <>
          <div className={styles.crosshair} />
          <div className={styles.walkHud}>
            <div ref={walkHudRef} />
            <div ref={targetRef} className={styles.weather} />
            <div ref={messageRef} className={styles.ticker} />
            <div className={styles.actions}>
              {WALK_ACTIONS.map((a, k) => (
                <button
                  key={a.code}
                  className={k === action ? styles.active : ""}
                  title={a.hint}
                  onClick={() => setAction(k)}
                >
                  <kbd>{a.key}</kbd> {a.label}
                </button>
              ))}
            </div>
            <div className={styles.hint}>
              click to look · WASD move · shift sprint · space jump · hold R rest (years pass) · click/E act ·
              right-click/F inspect · V camera · Q overview — standing still, time crawls; moving, it flies
            </div>
          </div>
        </>
      )}
      <div className={styles.hud}>
        <div ref={hudRef}>loading…</div>
        <div ref={weatherRef} className={styles.weather} />
        <div ref={tickerRef} className={styles.ticker} />
        <div ref={deathsRef} className={styles.deaths} />
      </div>
      {biomeView && (
        <div className={styles.biomes}>
          <div className={styles.inspectTitle}>Biomes</div>
          {BIOMES.map((b, k) => (
            <div key={b.name} className={styles.legendRow}>
              <span className={styles.swatch} style={{ background: b.color }} />
              {b.name}
              {biomeShares[k] !== undefined && <span className={styles.share}>{Math.round(biomeShares[k] * 100)}%</span>}
            </div>
          ))}
          <canvas ref={chartRef} width={200} height={140} className={styles.chart} />
          <div className={styles.hint}>Whittaker chart: temperature → · water ↑</div>
        </div>
      )}
      {inspectText && (
        <div className={styles.inspect}>
          <button className={styles.close} onClick={() => setInspectText(null)} aria-label="close">
            ✕
          </button>
          {inspectText.split("\n").map((line, k) => (
            <div key={k} className={k === 0 ? styles.inspectTitle : undefined}>
              {line}
            </div>
          ))}
        </div>
      )}
      {error && <div className={styles.error}>engine failed to start:{"\n"}{error}</div>}
    </div>
  );
}
