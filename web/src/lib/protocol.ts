// The typed main-thread ↔ worker contract, centralized so a renamed field
// fails protocol.test.ts rather than silently no-op'ing across postMessage.
// Every Control is plain data — no wasm handles cross the boundary.

import type { SimParams } from "./simParams.ts";

export type Brush = "tree" | "grass" | "clear" | "fire";

/** Tree varieties, matching the engine's archetype table. */
export type TreeSpecies = 0 | 1 | 2 | 3;
export const SPECIES_NAMES = ["Acacia", "Oak", "Pine", "Willow"] as const;

/** Grass functional types, matching the engine's GrassKind table. */
export type GrassKind = 0 | 1 | 2 | 3;
export const GRASS_NAMES = ["Bunchgrass", "Sod grass", "Sedge", "Annual"] as const;

export const BRUSH_CODES: Record<Brush, number> = { clear: 0, grass: 1, tree: 2, fire: 3 };

export type Control =
  | { type: "play" }
  | { type: "pause" }
  | { type: "step" }
  | { type: "speed"; value: number }
  | { type: "reseed"; seed: number }
  | { type: "params"; params: SimParams }
  | { type: "paint"; bx: number; by: number; brush: Brush; species?: TreeSpecies; grass?: GrassKind }
  | { type: "orbit"; dyaw: number; dpitch: number }
  | { type: "panBy"; dx: number; dy: number }
  | { type: "zoom"; factor: number }
  | { type: "resetCamera" }
  | { type: "resize"; w: number; h: number };

export const CONTROL_TYPES: ReadonlySet<string> = new Set([
  "play",
  "pause",
  "step",
  "speed",
  "reseed",
  "params",
  "paint",
  "orbit",
  "panBy",
  "zoom",
  "resetCamera",
  "resize",
]);

export function isControl(m: unknown): m is Control {
  return (
    typeof m === "object" &&
    m !== null &&
    "type" in m &&
    CONTROL_TYPES.has((m as { type: unknown }).type as string)
  );
}

/** Per-frame HUD payload. */
export type StatsSnapshot = {
  tick: number;
  bare: number;
  grass: number;
  trees: number;
  /** Tiles currently on fire (also counted under their state above). */
  burning: number;
  /** Thunderclouds currently crossing the map. */
  storms: number;
  /** Climate signals, 0..1. */
  sun: number;
  moisture: number;
  /** An oak mast year is under way. */
  mast: boolean;
  /** Trees carrying a pest/pathogen outbreak (oak wilt, bark beetles). */
  infested: number;
  /** Effective types within 16×16-tile windows (local, α diversity). */
  localDiversity: number;
  /** A river flood pulse is under way. */
  flooding: boolean;
  /** Effective number of plant types (e^Shannon over trees + grass kinds). */
  diversity: number;
  playing: boolean;
  selectedSpeed: number;
  /** Achieved speed; below selectedSpeed when a big map can't keep up. */
  actualSpeed: number;
  /** Current map size in tiles. */
  gridWidth: number;
  gridHeight: number;
  seed: number;
};

export type FromWorker =
  | { type: "ready"; backend: string; seed: number }
  | { type: "frame"; snapshot: StatsSnapshot }
  | { type: "fatal"; message: string };

/** One-shot boot blob; the canvas travels in the postMessage transfer list. */
export type InitConfig = {
  basePath: string;
  width: number;
  height: number;
  seed: number;
};

export type InitMsg = { type: "init"; canvas: OffscreenCanvas; config: InitConfig };
