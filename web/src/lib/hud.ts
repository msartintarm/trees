// Pure snapshot→string formatting for the HUD, kept out of the component so
// the exact readout is unit-tested.

import { BIOME_EVENTS, CLOUD_EVENTS, DEATH_CAUSES, type StatsSnapshot, type WalkSnapshot } from "./protocol.ts";

function group(n: number): string {
  return Math.floor(n).toLocaleString("en-US");
}

function fmtSpeed(v: number): string {
  if (v === Math.floor(v)) return String(v);
  return v < 0.1 ? v.toFixed(2) : v.toFixed(1);
}

export function speedLabel(s: StatsSnapshot): string {
  if (!s.playing) return "⏸ paused";
  const selected = `▶ ${fmtSpeed(s.selectedSpeed)}×`;
  // A big map at high speed runs as fast as the CPU allows: say so.
  if (s.actualSpeed < s.selectedSpeed * 0.8) {
    const actual = s.actualSpeed >= 10 ? Math.round(s.actualSpeed) : Math.round(s.actualSpeed * 10) / 10;
    return `${selected} (≈${fmtSpeed(actual)}×)`;
  }
  return selected;
}

/** Biodiversity as the effective number of plant types (of 8), whole map
 * and within a local 16×16 patch — the gap is how much the landscape's
 * regions differ. */
export function diversityLabel(s: StatsSnapshot): string {
  return `🌿 ${s.diversity.toFixed(1)} types (local ${s.localDiversity.toFixed(1)})`;
}

/** The sky by genus, e.g. "☁ 6 cumulus · 3 thunderheads · 2 rain sheets ·
 * 1 cirrus" (only genera present); empty under a clear sky. */
export function cloudsText(s: StatsSnapshot): string {
  const names = [
    ["cumulus", "cumulus"],
    ["thunderhead", "thunderheads"],
    ["rain sheet", "rain sheets"],
    ["cirrus", "cirrus"],
  ];
  const parts = s.clouds
    .map((n, k) => (n > 0 ? `${n} ${n === 1 ? names[k][0] : names[k][1]}` : ""))
    .filter((x) => x);
  return parts.length ? `☁ ${parts.join(" · ")}` : "";
}

/** The newest cloud transition between two snapshots' event tallies (the
 * weather ticker), or null if nothing noteworthy happened. Evaporation is
 * too routine to announce. */
export function newestCloudEvent(prev: number[] | null, next: number[]): string | null {
  if (!prev) return null;
  let found: string | null = null;
  next.forEach((n, k) => {
    if (n > (prev[k] ?? 0) && k !== 5) found = CLOUD_EVENTS[k] ?? null;
  });
  return found;
}

/** The leading causes of recent tree deaths (up to three), e.g.
 * "☠ pests 12 · drought 5 · fire 3" — empty when nothing is dying. */
export function deathsText(s: StatsSnapshot): string {
  const top = s.deathsRecent
    .map((n, k) => ({ n, cause: DEATH_CAUSES[k] ?? "?" }))
    .filter((d) => d.n >= 0.5)
    .sort((a, b) => b.n - a.n)
    .slice(0, 3);
  if (top.length === 0) return "";
  return `☠ ${top.map((d) => `${d.cause} ${Math.round(d.n)}`).join(" · ")}`;
}

/** The readout as stacked lines (the side panel): map and time, the
 * populations, diversity, climate, then one line per active condition. */
export function statsLines(s: StatsSnapshot): string[] {
  const pct = (v: number) => `${Math.round(v * 100)}%`;
  const lines = [
    `${s.gridWidth}×${s.gridHeight} · tick ${group(s.tick)} · ${speedLabel(s)}`,
    `🌳 ${group(s.trees)} trees · 🌾 ${group(s.grass)} grass · ${group(s.bare)} bare`,
    diversityLabel(s),
    `☀ ${pct(s.sun)} 💧 ${pct(s.moisture)}${s.mast ? " 🌰 mast year" : ""}`,
  ];
  if (s.storms > 0) lines.push(`⛈ ${group(s.storms)} storm${s.storms === 1 ? "" : "s"}`);
  if (s.flooding) lines.push("🌊 flood");
  if (s.burning > 0) lines.push(`🔥 ${group(s.burning)} burning`);
  if (s.infested > 0) lines.push(`🐛 ${group(s.infested)} infested`);
  if (s.grazers > 0) lines.push(`🦬 ${group(s.grazers)} grazers${s.wolves > 0 ? ` · 🐺 ${group(s.wolves)} wolves` : ""}`);
  if (s.houses > 0) lines.push(`🏠 ${group(s.houses)} house${s.houses === 1 ? "" : "s"}`);
  return lines;
}

export function statsText(s: StatsSnapshot): string {
  const fire = s.burning > 0 ? ` · 🔥 ${group(s.burning)}` : "";
  const storm = s.storms > 0 ? ` · ⛈ ${group(s.storms)}` : "";
  const pests = s.infested > 0 ? ` · 🐛 ${group(s.infested)}` : "";
  const flood = s.flooding ? " · 🌊 flood" : "";
  const houses = s.houses > 0 ? ` · 🏠 ${group(s.houses)}` : "";
  const animals = s.grazers > 0 ? ` · 🦬 ${group(s.grazers)}${s.wolves > 0 ? ` 🐺 ${group(s.wolves)}` : ""}` : "";
  const pct = (v: number) => `${Math.round(v * 100)}%`;
  const climate = ` · ☀ ${pct(s.sun)} 💧 ${pct(s.moisture)}${s.mast ? " 🌰" : ""}`;
  return `${s.gridWidth}×${s.gridHeight} · tick ${group(s.tick)} · trees ${group(s.trees)} · grass ${group(s.grass)} · bare ${group(s.bare)} · ${diversityLabel(s)}${climate}${storm}${flood}${fire}${pests}${animals}${houses} · ${speedLabel(s)}`;
}

const SEASONS = ["spring", "summer", "autumn", "winter"];

/** The season for a point in the year (0 = start of spring), e.g. "late
 * spring". */
export function seasonLabel(yearFrac: number): string {
  const f = ((yearFrac % 1) + 1) % 1;
  const k = Math.min(3, Math.floor(f * 4));
  const third = Math.min(2, Math.floor((f * 4 - k) * 3));
  return `${["early", "mid", "late"][third]} ${SEASONS[k]}`;
}

/** Days per real second above which days blur (matches the engine's
 * sky::BLUR_START). */
export const DAY_BLUR = 0.25;

/** How fast time flows, e.g. "⏱ 1.0 h/s", "⏩ 12 days/s", "⏩ 1.5 yr/s". */
export function paceLabel(daysPerSec: number): string {
  const hours = daysPerSec * 24;
  if (hours < 12) return `⏱ ${hours.toFixed(1)} h/s`;
  if (daysPerSec < 60) return `⏩ ${daysPerSec < 10 ? daysPerSec.toFixed(1) : Math.round(daysPerSec)} days/s`;
  return `⏩ ${(daysPerSec / 365).toFixed(1)} yr/s`;
}

/** Clock time for a day fraction, "14:05" — or a blur when days flash by. */
export function clockLabel(dayFrac: number, daysPerSec: number): string {
  if (daysPerSec >= DAY_BLUR) return "days blur past";
  const minutes = Math.floor((((dayFrac % 1) + 1) % 1) * 24 * 60);
  const hh = String(Math.floor(minutes / 60)).padStart(2, "0");
  const mm = String(minutes % 60).padStart(2, "0");
  return `${hh}:${mm}`;
}

/** The wanderer's readout: "🧭 year 3 · late spring · 14:05 · ⏱ 1.0 h/s ·
 * 🪵 4.2 timber" (+ wading). */
export function walkText(w: WalkSnapshot): string {
  const year = Math.floor(w.years) + 1;
  const parts = [
    `🧭 year ${year}`,
    seasonLabel(w.yearFrac),
    clockLabel(w.dayFrac, w.daysPerSec),
    paceLabel(w.daysPerSec),
    `🪵 ${w.wood.toFixed(1)} timber`,
  ];
  if (w.wading) parts.push("🌊 wading");
  return parts.join(" · ");
}

/** The newest landscape event between two snapshots' tallies, named by the
 * region it happened in ("the land" when unnamed), or null. */
export function newestBiomeEvent(prev: number[] | null, next: number[], places: string[]): string | null {
  if (!prev) return null;
  let found: string | null = null;
  next.forEach((n, k) => {
    if (n > (prev[k] ?? 0) && BIOME_EVENTS[k]) {
      found = BIOME_EVENTS[k].replace("{place}", places[k] || "the land");
    }
  });
  return found;
}

/** Every cloud transition since the last snapshot (not just the newest),
 * as an emoji icon with its full description for a tooltip. Evaporation
 * is too routine to show. */
export function cloudEventIcons(prev: number[] | null, next: number[]): { icon: string; text: string }[] {
  if (!prev) return [];
  const out: { icon: string; text: string }[] = [];
  next.forEach((n, k) => {
    const text = CLOUD_EVENTS[k];
    if (k === 5 || !text) return;
    for (let c = prev[k] ?? 0; c < n && out.length < 12; c++) {
      const space = text.indexOf(" ");
      out.push({ icon: space > 0 ? text.slice(0, space) : text, text: text.slice(space + 1) });
    }
  });
  return out;
}
