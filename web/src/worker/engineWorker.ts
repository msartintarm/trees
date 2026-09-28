// The engine worker: it owns the transferred OffscreenCanvas, wasm, the sim,
// and the render loop, so all of that runs off the main thread. Deliberately
// thin — every decision lives in the tested `engineSession` / `protocol`
// modules — it relays session callbacks out as `FromWorker` messages and
// inbound `Control`s into the session, buffering any that arrive before boot.

import { startEngineSession, type SessionHandle } from "../lib/engineSession.ts";
import { type Control, type FromWorker, type InitMsg, isControl } from "../lib/protocol.ts";

type WorkerScope = {
  onmessage: ((e: MessageEvent) => void) | null;
  postMessage(m: unknown): void;
};
const ctx = self as unknown as WorkerScope;
const post = (m: FromWorker) => ctx.postMessage(m);

let session: SessionHandle | null = null;
const pending: Control[] = [];

ctx.onmessage = async (e: MessageEvent) => {
  const msg = e.data as InitMsg | Control | undefined;
  if (msg && msg.type === "init") {
    try {
      session = await startEngineSession(msg.canvas, true, msg.config, {
        onReady: (r) => post({ type: "ready", ...r }),
        onFrame: (f) => post({ type: "frame", ...f }),
        onFatal: (message) => post({ type: "fatal", message }),
        onInspect: (text) => post({ type: "inspect", text }),
      });
      for (const c of pending.splice(0)) session.applyControl(c);
    } catch (err) {
      post({ type: "fatal", message: String(err) });
    }
    return;
  }
  if (!isControl(msg)) return;
  if (session) session.applyControl(msg);
  else pending.push(msg);
};
