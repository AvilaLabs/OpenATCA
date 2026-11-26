import { createHash } from "node:crypto";
import { open, writeFile } from "node:fs/promises";

const MAX_TRACE_BYTES = 4 * 1024 * 1024;
const MAX_MANIFEST_BYTES = 1024 * 1024;
const MAX_ASSET_BYTES = 16 * 1024 * 1024;
const MAX_WASM_BYTES = 128 * 1024 * 1024;
const MAX_EVENTS = 16_384;
const PACKET_HEADER_BYTES = 24;
const PACKET_BYTES = PACKET_HEADER_BYTES + 1280 * 720 * 4;
const SCENARIO_ID = "ord-normal-push-v1";
const SCENARIO_SEED = "0x4F52442D4E505631";

function requireValue(condition, message) {
  if (!condition) throw new Error(message);
}

function hash(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

async function readBounded(path, cap, label) {
  const handle = await open(path, "r");
  try {
    const metadata = await handle.stat();
    requireValue(metadata.isFile() && metadata.size > 0 && metadata.size <= cap, `${label} is not a bounded regular file`);
    const buffer = Buffer.alloc(cap + 1);
    let used = 0;
    while (used <= cap) {
      const request = Math.min(64 * 1024, cap + 1 - used);
      const result = await handle.read(buffer, used, request, null);
      if (result.bytesRead === 0) break;
      used += result.bytesRead;
    }
    requireValue(used <= cap, `${label} grew beyond its byte cap while reading`);
    return buffer.subarray(0, used);
  } finally {
    await handle.close();
  }
}

function parseArgs(argv) {
  const out = {};
  for (let index = 0; index < argv.length; index += 2) {
    const key = argv[index];
    const value = argv[index + 1];
    requireValue(key?.startsWith("--") && value !== undefined && !(key.slice(2) in out), "invalid or duplicate CLI argument");
    out[key.slice(2)] = value;
  }
  for (const key of ["wasm", "manifest", "scenario", "trace", "out"]) {
    requireValue(typeof out[key] === "string" && out[key].length > 0, `missing --${key}`);
  }
  requireValue(Object.keys(out).length === 5, "unexpected CLI argument");
  return out;
}

function readAsciiLines(bytes, cap, label) {
  requireValue(bytes.byteLength <= cap, `${label} exceeds its byte cap`);
  requireValue(bytes.byteLength > 0 && bytes.at(-1) === 10, `${label} must end with LF`);
  for (const byte of bytes) requireValue(byte <= 0x7f && byte !== 0x0d, `${label} must be ASCII with LF line endings`);
  const text = new TextDecoder("ascii", { fatal: true }).decode(bytes);
  const lines = text.slice(0, -1).split("\n");
  requireValue(lines.every((line) => line.length > 0), `${label} contains an empty line`);
  return lines;
}

function parseUnsigned(text, max, label) {
  requireValue(/^(0|[1-9][0-9]*)$/.test(text), `${label} is not canonical unsigned decimal`);
  const value = Number(text);
  requireValue(Number.isSafeInteger(value) && value <= max, `${label} is out of range`);
  return value;
}

function parseSigned(text, min, max, label) {
  requireValue(/^(0|-?[1-9][0-9]*)$/.test(text), `${label} is not canonical signed decimal`);
  const value = Number(text);
  requireValue(Number.isSafeInteger(value) && value >= min && value <= max, `${label} is out of range`);
  return value;
}

function parseTrace(bytes) {
  const lines = readAsciiLines(bytes, MAX_TRACE_BYTES, "trace");
  requireValue(lines.length >= 7 && lines[0] === "F2TRACE1", "unsupported trace header");
  const headers = lines.slice(1, 7).map((line) => line.split("\t"));
  requireValue(headers.every((fields) => fields.length === (fields[0] === "FRAME" ? 3 : 2)), "malformed trace headers");
  const [build, source, asset, scenario, seed, initialFrame] = headers;
  requireValue(build[0] === "BUILD" && /^[0-9a-f]{64}$/.test(build[1]), "bad BUILD header");
  requireValue(source[0] === "SOURCE" && /^[0-9a-f]{40}$/.test(source[1]), "bad SOURCE header");
  requireValue(asset[0] === "ASSET" && /^[0-9a-f]{64}$/.test(asset[1]), "bad ASSET header");
  requireValue(scenario[0] === "SCENARIO" && scenario[1] === SCENARIO_ID, "wrong scenario ID");
  requireValue(seed[0] === "SEED" && seed[1] === SCENARIO_SEED, "wrong baseline seed");
  requireValue(initialFrame[0] === "FRAME" && initialFrame[1] === "0" && /^[0-9a-f]{64}$/.test(initialFrame[2]), "bad initial frame record");

  const events = [];
  let cursor = 7;
  let currentMs = 0;
  while (cursor < lines.length) {
    requireValue(events.length < MAX_EVENTS, "trace exceeds event cap");
    const input = lines[cursor++];
    const fields = input.split("\t");
    const kind = fields[0];
    if (kind === "A") {
      requireValue(fields.length === 2, "bad advance input");
      const target = parseUnsigned(fields[1], 3_600_000, "advance timestamp");
      requireValue(target > currentMs, "advance timestamps must be strictly increasing");
    } else if (kind === "K") {
      requireValue(fields.length === 5, "bad key input");
      parseUnsigned(fields[1], 0xffff, "HID usage");
      parseUnsigned(fields[2], 0x0f, "modifier mask");
      parseUnsigned(fields[3], 1, "key-down flag");
      parseUnsigned(fields[4], 1, "repeat flag");
    } else if (kind === "P") {
      requireValue(fields.length === 5, "bad pointer input");
      parseSigned(fields[1], 0, 1279, "pointer x");
      parseSigned(fields[2], 0, 719, "pointer y");
      parseUnsigned(fields[3], 0xffff_ffff, "pointer buttons");
      parseUnsigned(fields[4], 4, "pointer kind");
      requireValue(fields[4] !== "0", "pointer kind is outside 1..4");
    } else {
      throw new Error("unknown trace input record");
    }
    const result = lines[cursor++]?.split("\t");
    requireValue(result?.length === 2 && result[0] === "RESULT", "missing input result");
    const status = parseSigned(result[1], -0x8000_0000, 0x7fff_ffff, "result status");
    const frame = lines[cursor++]?.split("\t");
    requireValue(frame?.length === 3 && frame[0] === "FRAME", "missing frame digest");
    requireValue(frame[1] === String(events.length + 1) && /^[0-9a-f]{64}$/.test(frame[2]), "bad frame index or digest");
    events.push({ input, fields, status, frameSha256: frame[2] });
    if (kind === "A") {
      events.at(-1).targetMs = parseUnsigned(fields[1], 3_600_000, "advance timestamp");
      if (status === 0) currentMs = events.at(-1).targetMs;
    }
  }
  return { build: build[1], source: source[1], asset: asset[1], initialFrame: initialFrame[2], events };
}

function frameBytes(exports) {
  const packed = exports.f2_frame();
  requireValue(typeof packed === "bigint" && packed !== 0n, "engine returned no frame range");
  const offset = Number((packed >> 32n) & 0xffff_ffffn);
  const length = Number(packed & 0xffff_ffffn);
  requireValue(length === PACKET_BYTES, "engine frame has wrong bounded size");
  const memory = exports.memory;
  requireValue(memory instanceof WebAssembly.Memory, "engine did not export linear memory");
  const buffer = memory.buffer;
  requireValue(offset <= buffer.byteLength && length <= buffer.byteLength - offset, "engine frame range is outside memory");
  const packet = new Uint8Array(buffer, offset, length).slice();
  const view = new DataView(packet.buffer, packet.byteOffset, packet.byteLength);
  requireValue(packet[0] === 0x4f && packet[1] === 0x41 && packet[2] === 0x46 && packet[3] === 0x32, "bad OAF2 magic");
  requireValue(view.getUint16(4, true) === 1 && view.getUint16(6, true) === 1, "unsupported OAF2 version or format");
  requireValue(view.getUint32(8, true) === 1280 && view.getUint32(12, true) === 720, "unexpected OAF2 dimensions");
  requireValue(view.getUint32(16, true) === 5120 && view.getUint32(20, true) === 3_686_400, "invalid OAF2 stride or payload length");
  return packet;
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  const manifestBytes = await readBounded(args.manifest, MAX_MANIFEST_BYTES, "manifest");
  const wasmBytes = await readBounded(args.wasm, MAX_WASM_BYTES, "WASM binary");
  const assetBytes = await readBounded(args.scenario, MAX_ASSET_BYTES, "scenario asset");
  const traceBytes = await readBounded(args.trace, MAX_TRACE_BYTES, "trace");
  const manifest = JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(manifestBytes));
  const trace = parseTrace(traceBytes);
  requireValue(manifest.schema === "oa-f2-wasm-build-v1", "unsupported WASM manifest schema");
  requireValue(manifest.build_id === trace.build && manifest.wasm_sha256 === trace.build, "trace build ID differs from manifest");
  requireValue(manifest.source_commit === trace.source && /^[0-9a-f]{40}$/.test(trace.source), "trace source commit differs from manifest");
  requireValue(manifest.scenario_asset_sha256 === trace.asset && hash(assetBytes) === trace.asset, "trace scenario asset SHA differs from file/manifest");
  requireValue(manifest.scenario_id === SCENARIO_ID && manifest.scenario_seed === SCENARIO_SEED, "manifest scenario identity mismatch");
  requireValue(hash(wasmBytes) === trace.build, "WASM SHA differs from trace build ID");

  const { instance } = await WebAssembly.instantiate(wasmBytes, {});
  const exports = instance.exports;
  const expectedExports = ["memory", "f2_reset", "f2_advance_to", "f2_key", "f2_pointer", "f2_frame", "__data_end", "__heap_base"].sort();
  const actualExports = Object.keys(exports).sort();
  requireValue(JSON.stringify(actualExports) === JSON.stringify(expectedExports), "WASM exports differ from the fixed scalar ABI");
  const resetStatus = exports.f2_reset(BigInt(SCENARIO_SEED));
  requireValue(resetStatus === 0, `WASM reset returned ${resetStatus}`);
  let packet = frameBytes(exports);
  let digest = hash(packet);
  requireValue(digest === trace.initialFrame, "initial frame digest mismatch");
  const output = ["F2RESULT1", `SOURCE\t${trace.source}`, `ASSET\t${trace.asset}`, `SCENARIO\t${SCENARIO_ID}`,
    `SEED\t${SCENARIO_SEED}`, `RESET\t0`, `FRAME\t0\t${digest}`, `EVENTS\t${trace.events.length}`];
  let currentMs = 0;
  for (let index = 0; index < trace.events.length; index += 1) {
    const event = trace.events[index];
    const fields = event.fields;
    let status;
    if (fields[0] === "A") {
      const target = event.targetMs;
      status = exports.f2_advance_to(BigInt(target));
      if (status === 0) currentMs = target;
    } else if (fields[0] === "K") {
      status = exports.f2_key(Number(fields[1]), Number(fields[2]), Number(fields[3]), Number(fields[4]));
    } else {
      status = exports.f2_pointer(Number(fields[1]), Number(fields[2]), Number(fields[3]), Number(fields[4]));
    }
    requireValue(Number.isInteger(status) && status === event.status, `status mismatch at event ${index + 1} (virtual ${currentMs} ms)`);
    packet = frameBytes(exports);
    digest = hash(packet);
    requireValue(digest === event.frameSha256, `frame mismatch at event ${index + 1} (virtual ${currentMs} ms)`);
    output.push(event.input, `RESULT\t${index + 1}\t${status}`, `FRAME\t${index + 1}\t${digest}`);
  }
  const result = `${output.join("\n")}\n`;
  requireValue(Buffer.byteLength(result, "ascii") <= MAX_TRACE_BYTES, "result exceeds byte cap");
  await writeFile(args.out, result, { encoding: "ascii", flag: "wx", mode: 0o600 });
  process.stdout.write(JSON.stringify({ events: trace.events.length, result: args.out, status: "matched" }) + "\n");
}

main().catch((error) => {
  process.stderr.write(`F2 WASM replay failed: ${error.message}\n`);
  process.exitCode = 1;
});
