const MAX_FRAME_BYTES = 24 + 1280 * 720 * 4;
const FRAME_HEADER_BYTES = 24;
const FRAME_MAGIC = [0x4f, 0x41, 0x46, 0x32]; // OAF2
const FRAME_VERSION = 1;
const FRAME_FORMAT_BGRX8_LE = 1;
const TRANSCRIPT_LIMIT_BYTES = 1024 * 1024;
const TRANSCRIPT_LIMIT_EVENTS = 16_384;
const TRACE_LIMIT_BYTES = 4 * 1024 * 1024;
const FIDELITY_LIMIT_ENTRIES = 256;
const FIDELITY_LIMIT_SOURCES = 4;
const FIDELITY_TEXT_LIMIT = 2_000;
const FIDELITY_RESPONSE_LIMIT_BYTES = 1024 * 1024;
const BUILD_MANIFEST_LIMIT_BYTES = 16 * 1024;
const WASM_LIMIT_BYTES = 64 * 1024 * 1024;
const STEP_MS = 5000;
const MAX_SCENARIO_MS = 60 * 60 * 1000;
const SCENARIO_ID = "ord-normal-push-v1";
const SCENARIO_SEED = "0x4F52442D4E505631";
const BUILD_MANIFEST_URL = "./engine/build.json";
const ENGINE_URL = "./engine/oa-f2-engine.wasm";

const canvas = document.querySelector("#scope");
const context = canvas.getContext("2d", { alpha: false });
const statusNode = document.querySelector("#status");
const buildNode = document.querySelector("#build-id");
const timeNode = document.querySelector("#virtual-time");
const hashNode = document.querySelector("#frame-hash");
const advanceButton = document.querySelector("#advance");
const playButton = document.querySelector("#play-pause");
const speedSelect = document.querySelector("#speed");
const jumpMinuteButton = document.querySelector("#jump-minute");
const jumpFiveButton = document.querySelector("#jump-five");
const saveButton = document.querySelector("#save-transcript");

const transcript = [];
const frameHashes = [];
const pendingDigestJobs = [];
let transcriptBytes = 0;
let eventCount = 0;
let virtualMs = 0n;
let engine = null;
let failed = false;
let digestGeneration = 0;
let pendingDigestCount = 0;
let playing = false;
let playbackFrame = 0;
let playbackAnchorWall = 0;
let playbackAnchorVirtual = 0n;
let playbackSpeed = 1;

const keyboardUsage = new Map([
  ["Enter", 0x28], ["Escape", 0x29], ["Backspace", 0x2a], ["Tab", 0x2b],
  ["Space", 0x2c], ["Minus", 0x2d], ["Equal", 0x2e], ["BracketLeft", 0x2f],
  ["BracketRight", 0x30], ["Backslash", 0x31], ["Semicolon", 0x33],
  ["Quote", 0x34], ["Backquote", 0x35], ["Comma", 0x36], ["Period", 0x37],
  ["Slash", 0x38], ["CapsLock", 0x39], ["ArrowRight", 0x4f], ["ArrowLeft", 0x50],
  ["ArrowDown", 0x51], ["ArrowUp", 0x52], ["Delete", 0x4c], ["Home", 0x4a],
  ["End", 0x4d], ["PageUp", 0x4b], ["PageDown", 0x4e], ["Insert", 0x49],
  ["F1", 0x3a], ["F2", 0x3b], ["F3", 0x3c], ["F4", 0x3d], ["F5", 0x3e],
  ["F6", 0x3f], ["F7", 0x40], ["F8", 0x41], ["F9", 0x42], ["F10", 0x43],
  ["F11", 0x44], ["F12", 0x45],
]);
for (let i = 0; i < 26; i += 1) keyboardUsage.set(`Key${String.fromCharCode(65 + i)}`, 0x04 + i);
for (let i = 0; i < 9; i += 1) keyboardUsage.set(`Digit${i + 1}`, 0x1e + i);
keyboardUsage.set("Digit0", 0x27);
for (let i = 0; i < 9; i += 1) keyboardUsage.set(`Numpad${i + 1}`, 0x59 + i);
keyboardUsage.set("Numpad0", 0x62);

function setStatus(message, isError = false) {
  statusNode.textContent = message;
  statusNode.dataset.error = isError ? "true" : "false";
}

function assert(condition, message) {
  if (!condition) throw new Error(message);
}

async function readBoundedResponse(response, limitBytes, what) {
  const declaredLength = response.headers.get("content-length");
  if (declaredLength !== null) {
    const declared = Number(declaredLength);
    assert(Number.isSafeInteger(declared) && declared >= 0 && declared <= limitBytes,
      `${what} exceeds its ${limitBytes}-byte cap.`);
  }
  assert(response.body && typeof response.body.getReader === "function",
    `${what} does not support bounded streaming reads.`);
  const reader = response.body.getReader();
  const chunks = [];
  let totalBytes = 0;
  while (true) {
    const { done, value } = await reader.read();
    if (done) break;
    totalBytes += value.byteLength;
    if (totalBytes > limitBytes) {
      await reader.cancel();
      throw new Error(`${what} exceeds its ${limitBytes}-byte cap.`);
    }
    chunks.push(value);
  }
  // The bounded chunks and final contiguous buffer coexist briefly, so temporary storage is
  // at most twice the named byte cap (plus the browser's bounded stream chunk).
  const bytes = new Uint8Array(totalBytes);
  let offset = 0;
  for (const chunk of chunks) {
    bytes.set(chunk, offset);
    offset += chunk.byteLength;
  }
  return bytes;
}

function boundedText(value, what) {
  assert(typeof value === "string" && value.length <= FIDELITY_TEXT_LIMIT,
    `Fidelity registry ${what} is invalid or too long.`);
  return value;
}

function appendText(parent, tag, text, className = "") {
  const node = document.createElement(tag);
  if (className) node.className = className;
  node.textContent = text;
  parent.append(node);
  return node;
}

async function loadFidelity() {
  const response = await fetch("./fidelity.json", { cache: "no-store" });
  assert(response.ok, "The local fidelity registry could not be loaded.");
  const registryBytes = await readBoundedResponse(response, FIDELITY_RESPONSE_LIMIT_BYTES,
    "Fidelity registry");
  const registry = JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(registryBytes));
  assert(registry.schema === "openatca-demo-fidelity-v1" && Array.isArray(registry.groups),
    "Unsupported fidelity registry schema.");
  let total = 0;
  const traceIds = new Set();
  const list = document.querySelector("#fidelity-list");
  list.replaceChildren();
  for (const group of registry.groups) {
    boundedText(group.id, "group id");
    boundedText(group.title, "group title");
    assert(Array.isArray(group.entries), "Fidelity registry group has no entries.");
    total += group.entries.length;
    assert(total <= FIDELITY_LIMIT_ENTRIES, "Fidelity registry entry count exceeds its cap.");
    const section = document.createElement("section");
    section.className = "fidelity-group-section";
    appendText(section, "h3", group.title, "fidelity-group-title");
    const entries = document.createElement("ul");
    entries.className = "fidelity-group";
    section.append(entries);
    for (const entry of group.entries) {
      boundedText(entry.id, "trace id");
      assert(!traceIds.has(entry.id), `Duplicate fidelity trace ID ${entry.id}.`);
      traceIds.add(entry.id);
      boundedText(entry.label, "element label");
      boundedText(entry.confidence, "confidence");
      boundedText(entry.availability, "availability");
      boundedText(entry.description, "description");
      assert(Array.isArray(entry.rows) && Array.isArray(entry.sources) &&
        entry.sources.length <= FIDELITY_LIMIT_SOURCES, "Fidelity registry source list is invalid.");
      const item = document.createElement("li");
      item.className = "fidelity-item";
      item.dataset.fidelityId = entry.id;
      appendText(item, "h4", entry.label);
      const meta = document.createElement("div");
      meta.className = "fidelity-meta";
      appendText(meta, "span", entry.id, "trace-id");
      appendText(meta, "span", entry.confidence, "confidence");
      appendText(meta, "span", entry.availability, "availability");
      item.append(meta);
      if (entry.rows.length > 0) appendText(item, "p", `Fidelity rows: ${entry.rows.join(", ")}.`);
      appendText(item, "p", entry.description);
      const sources = document.createElement("div");
      sources.className = "fidelity-sources";
      for (const source of entry.sources) {
        boundedText(source.label, "source label");
        boundedText(source.confidence, "source confidence");
        boundedText(source.href, "source link");
        const url = new URL(source.href);
        assert(url.protocol === "https:", "Fidelity registry links must use HTTPS.");
        const link = document.createElement("a");
        link.href = url.href;
        link.target = "_blank";
        link.rel = "noopener noreferrer";
        link.textContent = `${source.label} · ${source.confidence}`;
        sources.append(link);
      }
      if (entry.sources.length > 0) item.append(sources);
      entries.append(item);
    }
    list.append(section);
  }
  for (const node of document.querySelectorAll("[data-fidelity-id]")) {
    assert(traceIds.has(node.dataset.fidelityId),
      `Visible element has no fidelity entry: ${node.dataset.fidelityId}.`);
  }
  for (const node of document.querySelectorAll("[data-fidelity-ids]")) {
    for (const id of node.dataset.fidelityIds.split(/\s+/).filter(Boolean)) {
      assert(traceIds.has(id), `Visible element has no fidelity entry: ${id}.`);
    }
  }
  document.querySelector("#fidelity-count").textContent = `${total} trace entries`;
  document.querySelector("#fidelity-status").textContent =
    `${total} visible elements, unavailable actions, and app controls are mapped in this registry.`;
}

function modifiersFor(event) {
  return (event.ctrlKey ? 1 : 0) | (event.altKey ? 2 : 0) |
    (event.shiftKey ? 4 : 0) | (event.metaKey ? 8 : 0);
}

function appendTranscript(line, status) {
  const eventLine = `${line}\n`;
  const resultLine = `RESULT\t${status}\n`;
  const addedBytes = new TextEncoder().encode(eventLine + resultLine).byteLength;
  assert(eventCount < TRANSCRIPT_LIMIT_EVENTS, "Input transcript reached its event limit.");
  assert(transcriptBytes + addedBytes <= TRANSCRIPT_LIMIT_BYTES, "Input transcript reached its byte limit.");
  transcript.push(line, `RESULT\t${status}`);
  transcriptBytes += addedBytes;
  eventCount += 1;
  saveButton.disabled = eventCount === 0 || pendingDigestCount !== 0;
}

async function recordAndRender(line, apply) {
  assert(engine && !failed, "Rust engine is not available.");
  checkTranscriptCapacity(line);
  const status = apply();
  assert(Number.isInteger(status), "Rust engine returned an invalid input status.");
  const packet = readFramePacket();
  appendTranscript(line, status);
  paintFrame(packet);
  if (status !== 0) setStatus(`Rust reported input status ${status}; frame retained for replay.`);
  pendingDigestJobs.push(updateDigest(packet.bytes, eventCount));
  await pendingDigestJobs[pendingDigestJobs.length - 1];
}

function checkTranscriptCapacity(line) {
  const lines = `${line}\nRESULT\t-2147483648\n`;
  const lineBytes = new TextEncoder().encode(lines).byteLength;
  assert(eventCount < TRANSCRIPT_LIMIT_EVENTS, "Input transcript reached its event limit.");
  assert(transcriptBytes + lineBytes <= TRANSCRIPT_LIMIT_BYTES, "Input transcript reached its byte limit.");
}

function readFramePacket() {
  const packed = engine.f2_frame();
  assert(typeof packed === "bigint" && packed !== 0n, "Rust engine returned no frame packet.");
  const offset = Number((packed >> 32n) & 0xffffffffn);
  const length = Number(packed & 0xffffffffn);
  assert(Number.isSafeInteger(offset) && Number.isSafeInteger(length), "Frame range is not safely representable.");
  assert(length >= FRAME_HEADER_BYTES && length <= MAX_FRAME_BYTES, "Frame packet length is outside its cap.");
  // Reacquire the memory view after f2_frame: Rust may have grown linear memory.
  const memory = engine.memory;
  assert(memory instanceof WebAssembly.Memory, "Rust engine did not export linear memory.");
  const buffer = memory.buffer;
  assert(offset <= buffer.byteLength && length <= buffer.byteLength - offset, "Frame packet is outside WASM memory.");
  const bytes = new Uint8Array(buffer, offset, length).slice();
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  for (let i = 0; i < FRAME_MAGIC.length; i += 1) assert(bytes[i] === FRAME_MAGIC[i], "Bad frame packet magic.");
  assert(view.getUint16(4, true) === FRAME_VERSION, "Unsupported frame packet version.");
  assert(view.getUint16(6, true) === FRAME_FORMAT_BGRX8_LE, "Unsupported frame pixel format.");
  const width = view.getUint32(8, true);
  const height = view.getUint32(12, true);
  const stride = view.getUint32(16, true);
  const pixelBytes = view.getUint32(20, true);
  assert(width === 1280 && height === 720, "Frame geometry differs from the bounded 1280x720 profile.");
  assert(stride === width * 4 && pixelBytes === stride * height, "Frame packet has invalid stride or pixel length.");
  assert(length === FRAME_HEADER_BYTES + pixelBytes, "Frame packet contains unexpected trailing or missing bytes.");
  return { bytes, width, height, stride, pixelBytes };
}

function paintFrame(packet) {
  canvas.width = packet.width;
  canvas.height = packet.height;
  const image = context.createImageData(packet.width, packet.height);
  const src = packet.bytes;
  const dst = image.data;
  for (let p = 0, i = FRAME_HEADER_BYTES; p < packet.pixelBytes; p += 4, i += 4) {
    dst[p] = src[i + 2];
    dst[p + 1] = src[i + 1];
    dst[p + 2] = src[i];
    dst[p + 3] = 255;
  }
  context.putImageData(image, 0, 0);
}

async function updateDigest(bytes, stepIndex) {
  const generation = ++digestGeneration;
  pendingDigestCount += 1;
  saveButton.disabled = true;
  try {
    const digest = await crypto.subtle.digest("SHA-256", bytes);
    const hex = [...new Uint8Array(digest)].map((v) => v.toString(16).padStart(2, "0")).join("");
    frameHashes[stepIndex] = hex;
    if (generation === digestGeneration) hashNode.textContent = `Demo frame packet SHA-256: ${hex}`;
  } catch (error) {
    setStatus(`Frame painted; digest unavailable: ${error.message}`, true);
    throw error;
  } finally {
    pendingDigestCount -= 1;
    saveButton.disabled = transcript.length === 0 || pendingDigestCount !== 0;
  }
}

function pointerPixel(event) {
  const rect = canvas.getBoundingClientRect();
  assert(rect.width > 0 && rect.height > 0, "Canvas has no visible area.");
  return {
    x: Math.trunc((event.clientX - rect.left) * canvas.width / rect.width),
    y: Math.trunc((event.clientY - rect.top) * canvas.height / rect.height),
  };
}

function pointerKind(type) {
  return ({ pointerdown: 1, pointerup: 2, pointermove: 3, pointercancel: 4 })[type] ?? 0;
}

function formatVirtualTime(value) {
  const totalSeconds = Number(value / 1000n);
  const minutes = Math.floor(totalSeconds / 60);
  const seconds = totalSeconds % 60;
  return `${String(minutes).padStart(2, "0")}:${String(seconds).padStart(2, "0")}`;
}

function updateClockControls() {
  const atEnd = virtualMs >= BigInt(MAX_SCENARIO_MS);
  timeNode.textContent = `Demo virtual time: ${formatVirtualTime(virtualMs)} (${virtualMs} ms)`;
  advanceButton.disabled = failed || atEnd || playing;
  jumpMinuteButton.disabled = failed || atEnd || playing;
  jumpFiveButton.disabled = failed || atEnd || playing;
  playButton.disabled = failed || atEnd;
  speedSelect.disabled = failed || atEnd;
  if (atEnd && playing) pausePlayback();
}

function advanceVirtualTo(target, source = "explicit") {
  const max = BigInt(MAX_SCENARIO_MS);
  if (target > max) target = max;
  assert(target > virtualMs, "Virtual time must advance monotonically.");
  assert(target <= max, "Virtual time exceeds the scenario duration.");
  const input = `A\t${target}`;
  checkTranscriptCapacity(input);
  const status = engine.f2_advance_to(target);
  assert(Number.isInteger(status), "Rust engine returned an invalid clock status.");
  if (status === 0) virtualMs = target;
  const packet = readFramePacket();
  appendTranscript(input, status);
  paintFrame(packet);
  if (status !== 0) setStatus(`Rust refused virtual-time advance (status ${status}); prior time retained.`);
  else if (source === "playback") setStatus("Demo playback is advancing the Rust scenario from monotonic host time.");
  updateClockControls();
  pendingDigestJobs.push(updateDigest(packet.bytes, eventCount));
  return status;
}

function pausePlayback() {
  if (!playing) return;
  playing = false;
  if (playbackFrame !== 0) cancelAnimationFrame(playbackFrame);
  playbackFrame = 0;
  playButton.textContent = "Play demo";
  advanceButton.disabled = failed || virtualMs >= BigInt(MAX_SCENARIO_MS);
  jumpMinuteButton.disabled = failed || virtualMs >= BigInt(MAX_SCENARIO_MS);
  jumpFiveButton.disabled = failed || virtualMs >= BigInt(MAX_SCENARIO_MS);
}

function playbackTick(now) {
  if (!playing || failed) return;
  const elapsed = Math.max(0, now - playbackAnchorWall);
  const anchorMs = Number(playbackAnchorVirtual);
  const projectedMs = Math.min(MAX_SCENARIO_MS, anchorMs + Math.floor(elapsed * playbackSpeed));
  const targetMs = Math.floor(projectedMs / STEP_MS) * STEP_MS;
  let target = BigInt(targetMs);
  const max = BigInt(MAX_SCENARIO_MS);
  if (target > max) target = max;
  if (target > virtualMs) {
    try {
      const status = advanceVirtualTo(target, "playback");
      if (status !== 0) pausePlayback();
    } catch (error) {
      fail(error);
      return;
    }
  }
  if (virtualMs >= max) {
    pausePlayback();
    return;
  }
  playbackFrame = requestAnimationFrame(playbackTick);
}

function synchronizeRunningClock() {
  if (!playing) return true;
  const elapsed = Math.max(0, performance.now() - playbackAnchorWall);
  const elapsedMs = Math.min(MAX_SCENARIO_MS, Math.floor(elapsed * playbackSpeed));
  const target = playbackAnchorVirtual + BigInt(elapsedMs);
  if (target <= virtualMs) return true;
  try {
    const status = advanceVirtualTo(target, "input-clock");
    if (status === 0) return true;
    pausePlayback();
    return false;
  } catch (error) {
    fail(error);
    return false;
  }
}

function startPlayback() {
  if (failed || virtualMs >= BigInt(MAX_SCENARIO_MS)) return;
  playing = true;
  playbackSpeed = Number(speedSelect.value);
  playbackAnchorWall = performance.now();
  playbackAnchorVirtual = virtualMs;
  playButton.textContent = "Pause demo";
  advanceButton.disabled = true;
  jumpMinuteButton.disabled = true;
  jumpFiveButton.disabled = true;
  playbackFrame = requestAnimationFrame(playbackTick);
}

function fail(error) {
  pausePlayback();
  failed = true;
  advanceButton.disabled = true;
  playButton.disabled = true;
  speedSelect.disabled = true;
  jumpMinuteButton.disabled = true;
  jumpFiveButton.disabled = true;
  saveButton.disabled = transcript.length === 0;
  setStatus(`Demo host stopped: ${error.message}`, true);
}

advanceButton.addEventListener("click", async () => {
  try {
    const next = virtualMs + BigInt(STEP_MS);
    advanceVirtualTo(next);
    await pendingDigestJobs[pendingDigestJobs.length - 1];
  } catch (error) { fail(error); }
});

playButton.addEventListener("click", () => {
  if (playing) {
    synchronizeRunningClock();
    pausePlayback();
  }
  else startPlayback();
  updateClockControls();
});

speedSelect.addEventListener("change", () => {
  if (!playing) return;
  if (!synchronizeRunningClock()) return;
  playbackSpeed = Number(speedSelect.value);
  playbackAnchorWall = performance.now();
  playbackAnchorVirtual = virtualMs;
});

jumpMinuteButton.addEventListener("click", async () => {
  try {
    advanceVirtualTo(virtualMs + 60_000n);
    await pendingDigestJobs[pendingDigestJobs.length - 1];
  } catch (error) { fail(error); }
});

jumpFiveButton.addEventListener("click", async () => {
  try {
    advanceVirtualTo(virtualMs + 300_000n);
    await pendingDigestJobs[pendingDigestJobs.length - 1];
  } catch (error) { fail(error); }
});

canvas.addEventListener("keydown", async (event) => {
  const usage = keyboardUsage.get(event.code);
  if (usage === undefined) return;
  event.preventDefault();
  try {
    if (!synchronizeRunningClock()) return;
    const modifiers = modifiersFor(event);
    await recordAndRender(`K\t${usage}\t${modifiers}\t1\t${event.repeat ? 1 : 0}`,
      () => engine.f2_key(usage, modifiers, 1, event.repeat ? 1 : 0));
  } catch (error) { fail(error); }
});

canvas.addEventListener("keyup", async (event) => {
  const usage = keyboardUsage.get(event.code);
  if (usage === undefined) return;
  event.preventDefault();
  try {
    if (!synchronizeRunningClock()) return;
    const modifiers = modifiersFor(event);
    await recordAndRender(`K\t${usage}\t${modifiers}\t0\t0`,
      () => engine.f2_key(usage, modifiers, 0, 0));
  } catch (error) { fail(error); }
});

for (const type of ["pointerdown", "pointerup", "pointermove", "pointercancel"]) {
  canvas.addEventListener(type, async (event) => {
    try {
      if (!synchronizeRunningClock()) return;
      const { x, y } = pointerPixel(event);
      const buttons = event.buttons >>> 0;
      const kind = pointerKind(type);
      await recordAndRender(`P\t${x}\t${y}\t${buttons}\t${kind}`,
        () => engine.f2_pointer(x, y, buttons, kind));
      if (type === "pointerdown") canvas.focus({ preventScroll: true });
    } catch (error) { fail(error); }
  });
}

saveButton.addEventListener("click", async () => {
  if (!engine || transcript.length === 0) return;
  try {
    await Promise.all(pendingDigestJobs);
    assert(frameHashes.length === eventCount + 1, "Not every input step has a frame hash yet.");
    for (let i = 0; i < frameHashes.length; i += 1) {
      assert(/^[0-9a-f]{64}$/.test(frameHashes[i] ?? ""), `Frame hash ${i} is missing or invalid.`);
    }
    const lines = ["F2TRACE1", `BUILD\t${engine.buildId}`, `SOURCE\t${engine.sourceCommit}`,
      `ASSET\t${engine.assetSha256}`,
      `SCENARIO\t${engine.scenarioId}`, `SEED\t${engine.seedText}`,
      `FRAME\t0\t${frameHashes[0]}`];
    for (let i = 0; i < eventCount; i += 1) {
      lines.push(transcript[i * 2], transcript[i * 2 + 1], `FRAME\t${i + 1}\t${frameHashes[i + 1]}`);
    }
    const body = `${lines.join("\n")}\n`;
    assert(new TextEncoder().encode(body).byteLength <= TRACE_LIMIT_BYTES, "Frame trace exceeds its byte cap.");
    const blob = new Blob([body], { type: "text/plain;charset=utf-8" });
    const url = URL.createObjectURL(blob);
    const link = document.createElement("a");
    link.href = url;
    link.download = "f2-frame-trace.f2trace";
    link.click();
    URL.revokeObjectURL(url);
  } catch (error) { fail(error); }
});

async function loadEngine() {
  const response = await fetch(BUILD_MANIFEST_URL, { cache: "no-store" });
  assert(response.ok, "No verified engine build manifest found. Build/copy the Rust WASM artifact first.");
  const manifestBytes = await readBoundedResponse(response, BUILD_MANIFEST_LIMIT_BYTES,
    "WASM build manifest");
  const manifest = JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(manifestBytes));
  assert(manifest.schema === "oa-f2-wasm-build-v1", "Unsupported WASM build manifest schema.");
  assert(manifest.scenario_id === SCENARIO_ID, "WASM build is not the named Scenario 1 engine.");
  assert(typeof manifest.build_id === "string" && /^[0-9a-f]{64}$/.test(manifest.build_id), "Build ID must be a SHA-256 hex digest.");
  assert(manifest.wasm_sha256 === manifest.build_id, "Build ID must equal the bound WASM binary SHA-256.");
  assert(manifest.scenario_seed === SCENARIO_SEED, "WASM build does not use the frozen Scenario 1 baseline seed.");
  assert(typeof manifest.scenario_asset_sha256 === "string" && /^[0-9a-f]{64}$/.test(manifest.scenario_asset_sha256),
    "WASM build manifest must bind the exact prepared OASD scenario asset SHA-256.");
  assert(typeof manifest.source_commit === "string" && /^[0-9a-f]{40}$/.test(manifest.source_commit),
    "WASM build manifest must bind its source commit.");
  const wasmResponse = await fetch(ENGINE_URL, { cache: "no-store" });
  assert(wasmResponse.ok, "The WASM engine file is missing.");
  const wasmBytes = await readBoundedResponse(wasmResponse, WASM_LIMIT_BYTES, "WASM engine");
  const digest = await crypto.subtle.digest("SHA-256", wasmBytes);
  const actual = [...new Uint8Array(digest)].map((v) => v.toString(16).padStart(2, "0")).join("");
  assert(actual === manifest.wasm_sha256, "WASM file does not match the build manifest.");
  const result = await WebAssembly.instantiate(wasmBytes, {});
  const exports = result.instance.exports;
  const expectedExports = ["memory", "f2_reset", "f2_advance_to", "f2_key", "f2_pointer", "f2_frame", "__data_end", "__heap_base"].sort();
  const actualExports = Object.keys(exports).sort();
  assert(JSON.stringify(actualExports) === JSON.stringify(expectedExports),
    "WASM exports differ from the fixed reviewed scalar ABI.");
  engine = {
    ...exports,
    buildId: manifest.build_id,
    sourceCommit: manifest.source_commit,
    assetSha256: manifest.scenario_asset_sha256,
    scenarioId: manifest.scenario_id,
    seedText: manifest.scenario_seed,
  };
  assert(engine.f2_reset(BigInt(manifest.scenario_seed)) === 0, "Rust engine reset failed.");
  const packet = readFramePacket();
  paintFrame(packet);
  buildNode.textContent = `Build ${manifest.build_id}`;
  setStatus("Rust/WASM demo frame loaded. Playback and time skips advance only the fixed synthetic scenario.");
  pendingDigestJobs.push(updateDigest(packet.bytes, 0));
  await pendingDigestJobs[pendingDigestJobs.length - 1];
  playButton.disabled = false;
  speedSelect.disabled = false;
  updateClockControls();
}

loadFidelity().catch((error) => {
  document.querySelector("#fidelity-status").textContent = `Fidelity registry unavailable: ${error.message}`;
  document.querySelector("#fidelity-status").dataset.error = "true";
});
loadEngine().catch(fail);
