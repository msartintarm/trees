// Build the wasm package via wasm-pack, but skip the whole thing (wasm-bindgen +
// wasm-opt) when the inputs are unchanged since the last successful build.
// cargo already caches compilation; wasm-pack does not skip its post-processing,
// so this content-hash guard is what makes repeated `npm run dev` startups fast.
// Trimmed from the sibling traffic project (no threads variant here).

import { execSync } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const engineDir = join(here, "..", "..", "engine");
const outDir = join(here, "..", "public", "wasm-pkg");
const wasmFile = join(outDir, "engine_bg.wasm");
// Stamp lives under engine/target (gitignored, never touched by wasm-pack).
const stampFile = join(engineDir, "target", ".wasm-build.hash");

function walk(dir, exts, acc = []) {
  for (const e of readdirSync(dir, { withFileTypes: true })) {
    if (e.name === "target") continue;
    const p = join(dir, e.name);
    if (e.isDirectory()) walk(p, exts, acc);
    else if (exts.some((x) => e.name.endsWith(x))) acc.push(p);
  }
  return acc;
}

const inputs = [
  ...walk(join(engineDir, "src"), [".rs", ".wgsl"]),
  join(engineDir, "Cargo.toml"),
  join(engineDir, "Cargo.lock"),
]
  .filter(existsSync)
  .sort();

const h = createHash("sha256");
for (const f of inputs) h.update(readFileSync(f));
const hash = h.digest("hex");

// The build fingerprint the client fetches (never cached) to decide the `?v=` on
// the wasm/glue URLs — a new build always wins the browser cache, an unchanged
// one keeps it.
function writeVersion() {
  writeFileSync(join(outDir, "version.txt"), hash);
}

if (existsSync(wasmFile) && existsSync(stampFile) && readFileSync(stampFile, "utf8").trim() === hash) {
  writeVersion();
  console.log("wasm unchanged — using cached build");
  process.exit(0);
}

console.log("building wasm…");
execSync("wasm-pack build --target web --out-dir ../web/public/wasm-pkg --out-name engine", {
  cwd: engineDir,
  stdio: "inherit",
  shell: "/bin/bash",
});
writeVersion();
writeFileSync(stampFile, hash);
