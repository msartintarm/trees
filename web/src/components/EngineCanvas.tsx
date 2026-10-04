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
import { cloudEventIcons, cloudsText, deathsText, newestBiomeEvent, statsLines, walkText } from "../lib/hud.ts";
import {
  BIOMES,
  GRASS_NAMES,
  KEY_BITS,
  LIGHT_MODES,
  SPECIES_NAMES,
  WALK_ACTIONS,
  type Brush,
  type GrassKind,
  type MapLabel,
  type StatsSnapshot,
  type TreeSpecies,
} from "../lib/protocol.ts";
import { createSession, type Session } from "../lib/session.ts";
import {
  displayValue,
  applyPreset,
  matchingPreset,
  AUTO_PLANT_DEFAULT,
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

/** A collapsible panel section. Whether it's open is remembered per
 * viewer (localStorage, read after mount so the static export hydrates
 * cleanly; absent storage just means the defaults). */
function Section({ id, title, open = false, children }: { id: string; title: string; open?: boolean; children: React.ReactNode }) {
  const [isOpen, setOpen] = useState(open);
  useEffect(() => {
    try {
      const v = localStorage.getItem(`tree-sim:section:${id}`);
      if (v !== null) setOpen(v === "1");
    } catch {}
  }, [id]);
  return (
    <details
      className={styles.section}
      open={isOpen}
      onToggle={(e) => {
        const o = (e.currentTarget as HTMLDetailsElement).open;
        setOpen(o);
        try {
          localStorage.setItem(`tree-sim:section:${id}`, o ? "1" : "0");
        } catch {}
      }}
    >
      <summary>{title}</summary>
      <div className={styles.sectionBody}>{children}</div>
    </details>
  );
}

/** Pin the inspector card beside its tile on screen: to the right of it,
 * flipping left near the right edge, clamped into the viewport. Positions
 * arrive in backing pixels. */
function placeInspector(card: HTMLDivElement | null, at: [number, number] | null, scale: number, view: DOMRect): void {
  if (!card) return;
  if (!at) {
    card.style.visibility = "hidden";
    return;
  }
  card.style.visibility = "";
  const [x, y] = [at[0] / scale, at[1] / scale];
  const w = card.offsetWidth;
  const h = card.offsetHeight;
  const gap = 26;
  let left = x + gap;
  if (left + w > view.width - 8) left = x - gap - w;
  left = Math.max(8, Math.min(view.width - w - 8, left));
  const top = Math.max(8, Math.min(view.height - h - 8, y - h / 2));
  card.style.transform = `translate(${left.toFixed(0)}px, ${top.toFixed(0)}px)`;
}

/** Draw this frame's place names over the canvas, reusing a pool of
 * spans (positions arrive in backing-store pixels). */
function drawLabels(layer: HTMLDivElement | null, labels: MapLabel[], scale: number, show: boolean): void {
  if (!layer) return;
  // Earlier labels win: drop any that would overlap one already placed.
  const placed: { x: number; y: number; w: number }[] = [];
  const items = (show ? labels : []).filter((l) => {
    const x = l.x / scale;
    const y = l.y / scale;
    const w = l.text.length * (l.landmark ? 7 : 9.5);
    if (placed.some((p) => Math.abs(p.x - x) < (p.w + w) / 2 + 8 && Math.abs(p.y - y) < 20)) return false;
    placed.push({ x, y, w });
    return true;
  });
  while (layer.children.length < items.length) layer.appendChild(document.createElement("span"));
  for (let k = 0; k < layer.children.length; k++) {
    const el = layer.children[k] as HTMLSpanElement;
    const l = items[k];
    if (!l) {
      el.style.display = "none";
      continue;
    }
    el.style.display = "";
    el.textContent = l.text;
    el.className = l.landmark ? "landmark" : "region";
    el.style.transform = `translate(${(l.x / scale).toFixed(1)}px, ${(l.y / scale).toFixed(1)}px) translate(-50%, -50%)`;
    el.style.opacity = l.alpha.toFixed(2);
  }
}

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
  const skyRef = useRef<HTMLDivElement | null>(null);
  const tipRef = useRef<HTMLDivElement | null>(null);
  const inspectRef = useRef<HTMLDivElement | null>(null);
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
  const placeRef = useRef<HTMLDivElement | null>(null);
  const labelsRef = useRef<HTMLDivElement | null>(null);
  const [names, setNames] = useState(true);
  const namesRef = useRef(true);
  namesRef.current = names;
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
    let lastBiomeEvents: number[] | null = null;
    // Recent cloud transitions, shown as emoji with tooltips.
    let lastCloudEvents: number[] | null = null;
    let sky: { icon: string; text: string; at: number }[] = [];
    let sharesAt = 0;
    let tickerShownAt = 0;
    const session = createSession(
      canvas,
      { basePath: basePath(), width: initial.w, height: initial.h, seed: DEFAULT_SEED },
      {
        onReady: (r) => setBackend(r.backend),
        onFrame: (f) => {
          snapshotRef.current = f.snapshot;
          if (hudRef.current) hudRef.current.textContent = statsLines(f.snapshot).join("\n");
          if (deathsRef.current) deathsRef.current.textContent = deathsText(f.snapshot);
          if (weatherRef.current) weatherRef.current.textContent = cloudsText(f.snapshot);
          if (biomeViewRef.current && f.snapshot.biomeSamples.length) {
            drawWhittaker(chartRef.current, f.snapshot.biomeSamples);
            if (performance.now() - sharesAt > 1000) {
              sharesAt = performance.now();
              setBiomeShares(f.snapshot.biomeShares);
            }
          }
          // The ticker: landscape events (fires, floods, blooms, beetle
          // waves), each shown for a few seconds. Cloud-by-cloud
          // transitions aren't announced (the census line shows the sky).
          const event = newestBiomeEvent(lastBiomeEvents, f.snapshot.biomeEvents, f.snapshot.eventPlaces);
          lastBiomeEvents = f.snapshot.biomeEvents;
          const fresh = cloudEventIcons(lastCloudEvents, f.snapshot.cloudEvents);
          lastCloudEvents = f.snapshot.cloudEvents;
          const now = performance.now();
          const kept = sky.filter((e) => now - e.at < 30000);
          if (fresh.length || kept.length !== sky.length) {
            sky = [...kept, ...fresh.map((e) => ({ ...e, at: now }))].slice(-10);
            const el = skyRef.current;
            if (el) {
              el.replaceChildren(
                ...sky.map((e) => {
                  const span = document.createElement("span");
                  span.textContent = e.icon;
                  span.dataset.tip = e.text;
                  return span;
                }),
              );
            }
          }
          const rect = canvas.getBoundingClientRect();
          placeInspector(inspectRef.current, f.snapshot.selection, rect.width > 0 ? lastSize.w / rect.width : 1, rect);
          drawLabels(labelsRef.current, f.snapshot.labels, rect.width > 0 ? lastSize.w / rect.width : 1, namesRef.current);
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
            if (placeRef.current) placeRef.current.textContent = w.place ? `📍 ${w.place}` : "";
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

    // ---- instant tooltips for the weather icons ----
    const skyEl = skyRef.current;
    const showTip = (e: PointerEvent) => {
      const el = e.target as HTMLElement | null;
      const tip = tipRef.current;
      if (!tip || !el?.dataset.tip) return;
      tip.textContent = el.dataset.tip;
      const r = el.getBoundingClientRect();
      tip.hidden = false;
      const w = tip.offsetWidth;
      tip.style.left = `${Math.max(8, Math.min(window.innerWidth - w - 8, r.left + r.width / 2 - w / 2))}px`;
      tip.style.top = `${r.bottom + 6}px`;
    };
    const hideTip = () => {
      if (tipRef.current) tipRef.current.hidden = true;
    };
    skyEl?.addEventListener("pointerover", showTip);
    skyEl?.addEventListener("pointerout", hideTip);

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
      skyEl?.removeEventListener("pointerover", showTip);
      skyEl?.removeEventListener("pointerout", hideTip);
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

  // The auto-plant toggle remembers the last rate it ran at.
  const autoRateRef = useRef(AUTO_PLANT_DEFAULT);
  if (params.autoPlant > 0) autoRateRef.current = params.autoPlant;
  const toggleAutoPlant = () => {
    const field = PARAM_FIELDS.find((f) => f.key === "autoPlant")!;
    const next = withFieldValue(field, params, params.autoPlant > 0 ? 0 : autoRateRef.current);
    setParams(next);
    setParamText((t) => ({ ...t, autoPlant: String(displayValue(field, next)) }));
    send({ type: "params", params: next });
  };

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
      <div ref={labelsRef} className={styles.labels} />
      <div className={styles.panel}>
        <p className={styles.title}>Tree Simulator {backend && <span className={styles.hint}>({backend})</span>}</p>
        <Section id="sim" title="▶ Simulation" open>
          <div className={styles.row}>
            <button onClick={togglePlay}>{playing ? "⏸ Pause" : "▶ Play"}</button>
            <button onClick={() => send({ type: "step" })}>Step</button>
            <button onClick={() => send({ type: "resetCamera" })}>Reset view</button>
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
            <span className={styles.label}>Seed</span>
            <input
              type="number"
              value={seed}
              onChange={(e) => setSeed(Number(e.target.value) >>> 0)}
            />
            <button onClick={() => send({ type: "reseed", seed })}>Reseed</button>
          </div>
        </Section>
        <Section id="plant" title="🌱 Plant & tools" open>
          <div className={styles.row}>
            <span className={styles.label}>Brush</span>
            <button
              className={params.autoPlant > 0 ? styles.active : ""}
              title={`plant a random tree or grass on a random open tile, ${autoRateRef.current} per tick (rate under Parameters)`}
              onClick={toggleAutoPlant}
            >
              🎲 Auto-plant
            </button>
            {(["fire", "clear", "inspect"] as Tool[]).map((b) => (
              <button key={b} className={b === brush ? styles.active : ""} onClick={() => setBrush(b)}>
                {b === "fire" ? "🔥 Fire" : b === "clear" ? "✕ Clear" : "🔍 Inspect"}
              </button>
            ))}
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
        </Section>
        <Section id="explore" title="🚶 Explore" open>
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
        </Section>
        <Section id="view" title="👁 View">
          <div className={styles.row}>
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
              className={names ? styles.active : ""}
              title="region and landmark names over the map"
              onClick={() => setNames(!names)}
            >
              🏷 Names
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
        </Section>
        <Section id="ground" title="⛰ Ground">
          <div className={styles.row}>
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
        </Section>
        <Section id="light" title="☀ Light & quality">
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
        </Section>
        <Section id="params" title="⚙ Parameters">
          <div className={styles.params}>
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
                  </div>
        </Section>
        <div className={styles.hint}>
          the world starts empty — paint life with the brushes · click: plant ·
          drag: orbit · shift/right-drag: pan · wheel: zoom
        </div>
      </div>
      {walking && (
        <>
          <div className={styles.crosshair} />
          <div className={styles.walkHud}>
            <div ref={placeRef} className={styles.place} />
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
      <div className={styles.side}>
        <div className={styles.hud}>
          <div ref={hudRef} className={styles.stats}>loading…</div>
          <div ref={weatherRef} className={styles.weather} />
          <div ref={skyRef} className={styles.sky} />
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
        </div>
    {inspectText && (
        <div ref={inspectRef} className={styles.inspect}>
          <button
          className={styles.close}
          onClick={() => {
            setInspectText(null);
            send({ type: "clearInspect" });
          }}
          aria-label="close"
        >
            ✕
          </button>
          {inspectText.split("\n").map((line, k) => (
            <div key={k} className={k === 0 ? styles.inspectTitle : undefined}>
              {line}
            </div>
          ))}
        </div>
      )}
      <div ref={tipRef} className={styles.tip} hidden />
      {error && <div className={styles.error}>engine failed to start:{"\n"}{error}</div>}
    </div>
  );
}
