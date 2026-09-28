// Pure snapshot→string formatting for the HUD, kept out of the component so
// the exact readout is unit-tested.

import type { StatsSnapshot } from "./protocol.ts";

function group(n: number): string {
  return Math.floor(n).toLocaleString("en-US");
}

function fmtSpeed(v: number): string {
  return v === Math.floor(v) ? String(v) : v.toFixed(1);
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

export function statsText(s: StatsSnapshot): string {
  const fire = s.burning > 0 ? ` · 🔥 ${group(s.burning)}` : "";
  const storm = s.storms > 0 ? ` · ⛈ ${group(s.storms)}` : "";
  const pests = s.infested > 0 ? ` · 🐛 ${group(s.infested)}` : "";
  const flood = s.flooding ? " · 🌊 flood" : "";
  const pct = (v: number) => `${Math.round(v * 100)}%`;
  const climate = ` · ☀ ${pct(s.sun)} 💧 ${pct(s.moisture)}${s.mast ? " 🌰" : ""}`;
  return `${s.gridWidth}×${s.gridHeight} · tick ${group(s.tick)} · trees ${group(s.trees)} · grass ${group(s.grass)} · bare ${group(s.bare)} · ${diversityLabel(s)}${climate}${storm}${flood}${fire}${pests} · ${speedLabel(s)}`;
}
