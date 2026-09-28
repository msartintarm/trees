// Pure snapshot→string formatting for the HUD, kept out of the component so
// the exact readout is unit-tested.

import { CLOUD_EVENTS, DEATH_CAUSES, type StatsSnapshot } from "./protocol.ts";

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

export function statsText(s: StatsSnapshot): string {
  const fire = s.burning > 0 ? ` · 🔥 ${group(s.burning)}` : "";
  const storm = s.storms > 0 ? ` · ⛈ ${group(s.storms)}` : "";
  const pests = s.infested > 0 ? ` · 🐛 ${group(s.infested)}` : "";
  const flood = s.flooding ? " · 🌊 flood" : "";
  const pct = (v: number) => `${Math.round(v * 100)}%`;
  const climate = ` · ☀ ${pct(s.sun)} 💧 ${pct(s.moisture)}${s.mast ? " 🌰" : ""}`;
  return `${s.gridWidth}×${s.gridHeight} · tick ${group(s.tick)} · trees ${group(s.trees)} · grass ${group(s.grass)} · bare ${group(s.bare)} · ${diversityLabel(s)}${climate}${storm}${flood}${fire}${pests} · ${speedLabel(s)}`;
}
