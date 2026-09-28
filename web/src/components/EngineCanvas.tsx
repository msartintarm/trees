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
import { statsText } from "../lib/hud.ts";
import { SPECIES_NAMES, type Brush, type StatsSnapshot, type TreeSpecies } from "../lib/protocol.ts";
import { createSession, type Session } from "../lib/session.ts";
import {
  displayValue,
  matchingPreset,
  withFieldValue,
  DEFAULT_PARAMS,
  PARAM_FIELDS,
  PRESETS,
  type ParamField,
  type SimParams,
} from "../lib/simParams.ts";
import styles from "./EngineCanvas.module.css";

const SPEEDS = [0.5, 1, 2, 8, 32];
const DEFAULT_SEED = 7;

export default function EngineCanvas() {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const sessionRef = useRef<Session | null>(null);
  const bootedRef = useRef(false);
  const hudRef = useRef<HTMLDivElement | null>(null);
  const snapshotRef = useRef<StatsSnapshot | null>(null);

  const [error, setError] = useState<string | null>(null);
  const [backend, setBackend] = useState<string>("");
  const [playing, setPlaying] = useState(true);
  const [speed, setSpeed] = useState(1);
  const [brush, setBrush] = useState<Brush>("tree");
  const [treeSpecies, setTreeSpecies] = useState<TreeSpecies>(0);
  const [seed, setSeed] = useState(DEFAULT_SEED);
  const [params, setParams] = useState<SimParams>(DEFAULT_PARAMS);
  // Raw input text per field, so partially-typed values ("0.", "") don't
  // snap back mid-keystroke; params only update on parseable input.
  const [paramText, setParamText] = useState<Record<string, string>>(() =>
    Object.fromEntries(PARAM_FIELDS.map((f) => [f.key, String(displayValue(f, DEFAULT_PARAMS))])),
  );
  const brushRef = useRef<Brush>(brush);
  brushRef.current = brush;
  const speciesRef = useRef<TreeSpecies>(treeSpecies);
  speciesRef.current = treeSpecies;

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas || bootedRef.current) return;
    // The OffscreenCanvas transfer is one-shot; the guard keeps React
    // StrictMode's dev double-mount from booting twice.
    bootedRef.current = true;

    const initial = backingSize(canvas.clientWidth, canvas.clientHeight, devicePixelRatio);
    const session = createSession(
      canvas,
      { basePath: basePath(), width: initial.w, height: initial.h, seed: DEFAULT_SEED },
      {
        onReady: (r) => setBackend(r.backend),
        onFrame: (f) => {
          snapshotRef.current = f.snapshot;
          if (hudRef.current) hudRef.current.textContent = statsText(f.snapshot);
          setPlaying((p) => (p === f.snapshot.playing ? p : f.snapshot.playing));
        },
        onFatal: (message) => setError(message),
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
      session.applyControl({ type: "paint", bx, by, brush: brushRef.current, species: speciesRef.current });
    };

    const onWheel = (e: WheelEvent) => {
      e.preventDefault();
      session.applyControl({ type: "zoom", factor: wheelZoomFactor(e.deltaY) });
    };

    const onContextMenu = (e: Event) => e.preventDefault();

    canvas.addEventListener("pointerdown", onPointerDown);
    canvas.addEventListener("pointermove", onPointerMove);
    canvas.addEventListener("pointerup", onPointerUp);
    canvas.addEventListener("pointercancel", onPointerUp);
    canvas.addEventListener("wheel", onWheel, { passive: false });
    canvas.addEventListener("contextmenu", onContextMenu);

    return () => {
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

  const applyPreset = (key: string) => {
    const preset = PRESETS.find((p) => p.key === key);
    if (!preset) return;
    setParams(preset.params);
    setParamText(
      Object.fromEntries(PARAM_FIELDS.map((f) => [f.key, String(displayValue(f, preset.params))])),
    );
    send({ type: "params", params: preset.params });
  };

  const activePreset = matchingPreset(params);

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
          <span className={styles.label}>Speed</span>
          {SPEEDS.map((v) => (
            <button key={v} className={v === speed ? styles.active : ""} onClick={() => pickSpeed(v)}>
              {v}×
            </button>
          ))}
        </div>
        <div className={styles.row}>
          <span className={styles.label}>Brush</span>
          {(["grass", "fire", "clear"] as Brush[]).map((b) => (
            <button key={b} className={b === brush ? styles.active : ""} onClick={() => setBrush(b)}>
              {b === "grass" ? "🌱 Grass" : b === "fire" ? "🔥 Fire" : "✕ Clear"}
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
              onChange={(e) => applyPreset(e.target.value)}
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
      <div ref={hudRef} className={styles.hud}>
        loading…
      </div>
      {error && <div className={styles.error}>engine failed to start:{"\n"}{error}</div>}
    </div>
  );
}
