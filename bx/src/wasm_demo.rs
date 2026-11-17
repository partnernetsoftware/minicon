//! Self-contained browser demo generator for the wasm target.
//!
//! `bx build --wasm --html script.bxs` writes `<name>.html` plus a sibling
//! `<name>.js`. The module is embedded as base64, so the HTML works when opened
//! directly from disk, and the same JavaScript runs under node for verification.

use crate::wasm_target;

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn base64_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(B64[(triple >> 18) as usize & 63] as char);
        out.push(B64[(triple >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            B64[(triple >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            B64[triple as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// `(module_bytes, js, html)` for one script, ready to write to disk.
pub fn build_demo(source: &str) -> Result<(Vec<u8>, String, String), String> {
    let module = wasm_target::compile_to_wasm(source)?;
    let encoded = base64_encode(&module);
    let js = GLUE.replace("__WASM_BASE64__", &encoded);
    let html = HTML.replace("__WASM_JS__", &js);
    Ok((module, js, html))
}

const GLUE: &str = r#""use strict";
const WASM_BASE64 = "__WASM_BASE64__";
function decodeBase64(text) {
  if (typeof Buffer !== "undefined") {
    return new Uint8Array(Buffer.from(text, "base64"));
  }
  const raw = atob(text);
  const bytes = new Uint8Array(raw.length);
  for (let i = 0; i < raw.length; i++) bytes[i] = raw.charCodeAt(i);
  return bytes;
}
function emit(text) {
  if (typeof process !== "undefined" && process.stdout) {
    process.stdout.write(text + "\n");
    return;
  }
  if (typeof document !== "undefined") {
    const out = document.getElementById("out");
    if (out) out.textContent += text + "\n";
    return;
  }
  console.log(text);
}
function formatFloat(value) {
  if (!Number.isFinite(value)) return "null";
  return String(value);
}
let instance = null;
let nextArg = 65536;
let nextStr = 262144;
function writeStringBlock(text) {
  const bytes = new TextEncoder().encode(text);
  const start = nextStr;
  new DataView(instance.exports.memory.buffer).setUint32(start, bytes.length, true);
  new Uint8Array(instance.exports.memory.buffer, start + 4, bytes.length).set(bytes);
  nextStr = (start + 4 + bytes.length + 7) & ~7;
  return start;
}
const gl = { width: 0, height: 0, clear: [0, 0, 0, 1], pixels: new Uint8Array(0) };
const maps = [new Map()];
const queryArgs = (() => {
  if (typeof process !== "undefined" && Array.isArray(process.argv)) {
    return process.argv.slice(2);
  }
  if (typeof location !== "undefined" && location.search) {
    return Array.from(new URLSearchParams(location.search).getAll("arg"));
  }
  return [];
})();
const imports = {
  env: {
    print_i64: (value) => { emit(value.toString()); return value; },
    print_utf8: (ptr, len) => {
      const bytes = new Uint8Array(instance.exports.memory.buffer, ptr, len);
      emit(new TextDecoder("utf-8").decode(bytes));
    },    eprint: (ptr, len) => {
      const bytes = new Uint8Array(instance.exports.memory.buffer, ptr, len);
      const line = new TextDecoder("utf-8").decode(bytes);
      if (typeof console !== "undefined" && console.error) console.error(line);
      else emit(line);
    },
    print_f64: (value) => { emit(formatFloat(value)); },
    print_arr_i64: (ptr) => {
      const view = new DataView(instance.exports.memory.buffer);
      const count = view.getUint32(ptr, true);
      const pieces = [];
      for (let i = 0; i < count; i++) pieces.push(view.getBigInt64(ptr + 4 + i * 8, true).toString());
      emit("[" + pieces.join(",") + "]");
    },
    print_arr_str: (ptr) => {
      const view = new DataView(instance.exports.memory.buffer);
      const decode = (p, l) => new TextDecoder("utf-8").decode(new Uint8Array(instance.exports.memory.buffer, p, l));
      const count = view.getUint32(ptr, true);
      const pieces = [];
      for (let i = 0; i < count; i++) {
        const data = view.getUint32(ptr + 4 + i * 8, true);
        const size = view.getUint32(ptr + 8 + i * 8, true);
        pieces.push('"' + decode(data, size) + '"');
      }
      emit("[" + pieces.join(",") + "]");
    },
    print_record: (table, count, rec) => {
      const view = new DataView(instance.exports.memory.buffer);
      const decode = (p, l) => new TextDecoder("utf-8").decode(new Uint8Array(instance.exports.memory.buffer, p, l));
      const pieces = [];
      for (let i = 0; i < count; i++) {
        const block = table + view.getUint32(table + i * 4, true);
        const size = view.getUint32(block, true);
        const key = decode(block + 4, size);
        pieces.push(JSON.stringify(key) + ":" + view.getBigInt64(rec + i * 8, true).toString());
      }
      emit("{" + pieces.join(",") + "}");
    },
    exit: (code) => {
      throw new Error("bx exit " + code);
    },
    error: (ptr, len) => {
      const text = new TextDecoder("utf-8").decode(new Uint8Array(instance.exports.memory.buffer, ptr, len));
      throw new Error(text);
    },    str_to_float: (ptr, len) => {
      const text = new TextDecoder("utf-8").decode(new Uint8Array(instance.exports.memory.buffer, ptr, len)).trim();
      const value = Number(text);
      if (Number.isNaN(value)) throw new Error("cannot convert str to float");
      return value;
    },    str_len: (ptr, len) => {
      const text = new TextDecoder("utf-8").decode(new Uint8Array(instance.exports.memory.buffer, ptr, len));
      return BigInt(Array.from(text).length);
    },    print_record_typed: (kinds, table, count, rec) => {
      const view = new DataView(instance.exports.memory.buffer);
      const bytes = new Uint8Array(instance.exports.memory.buffer);
      const decode = (p, l) => new TextDecoder("utf-8").decode(new Uint8Array(instance.exports.memory.buffer, p, l));
      const pieces = [];
      for (let i = 0; i < count; i++) {
        const block = table + view.getUint32(table + i * 4, true);
        const key = decode(block + 4, view.getUint32(block, true));
        const kind = bytes[kinds + i];
        const field = rec + i * 8;
        let rendered;
        if (kind === 1) {
          const ptr = view.getUint32(field, true);
          const len = view.getUint32(field + 4, true);
          rendered = JSON.stringify(decode(ptr, len));
        } else if (kind === 2) {
          rendered = formatFloat(view.getFloat64(field, true));
        } else if (kind === 3) {
          rendered = view.getBigInt64(field, true) !== 0n ? "true" : "false";
        } else {
          rendered = view.getBigInt64(field, true).toString();
        }
        pieces.push(JSON.stringify(key) + ":" + rendered);
      }
      emit("{" + pieces.join(",") + "}");
    },
    print_arr_obj_typed: (kinds, table, count, arr) => {
      const view = new DataView(instance.exports.memory.buffer);
      const bytes = new Uint8Array(instance.exports.memory.buffer);
      const decode = (p, l) => new TextDecoder("utf-8").decode(new Uint8Array(instance.exports.memory.buffer, p, l));
      const formatOne = (rec) => {
        const pieces = [];
        for (let i = 0; i < count; i++) {
          const block = table + view.getUint32(table + i * 4, true);
          const key = decode(block + 4, view.getUint32(block, true));
          const kind = bytes[kinds + i];
          const field = rec + i * 8;
          let rendered;
          if (kind === 1) {
            const ptr = view.getUint32(field, true);
            const len = view.getUint32(field + 4, true);
            rendered = JSON.stringify(decode(ptr, len));
          } else if (kind === 2) {
            rendered = formatFloat(view.getFloat64(field, true));
          } else if (kind === 3) {
            rendered = view.getBigInt64(field, true) !== 0n ? "true" : "false";
          } else {
            rendered = view.getBigInt64(field, true).toString();
          }
          pieces.push(JSON.stringify(key) + ":" + rendered);
        }
        return "{" + pieces.join(",") + "}";
      };
      const len = view.getUint32(arr, true);
      const rows = [];
      for (let i = 0; i < len; i++) rows.push(formatOne(view.getUint32(arr + 4 + i * 8, true)));
      emit("[" + rows.join(",") + "]");
    },    print_arr_strarr: (ptr) => {
      const view = new DataView(instance.exports.memory.buffer);
      const decode = (p, l) => new TextDecoder("utf-8").decode(new Uint8Array(instance.exports.memory.buffer, p, l));
      const formatRow = (inner) => {
        const count = view.getUint32(inner, true);
        const pieces = [];
        for (let i = 0; i < count; i++) {
          const data = view.getUint32(inner + 4 + i * 8, true);
          const size = view.getUint32(inner + 8 + i * 8, true);
          pieces.push(JSON.stringify(decode(data, size)));
        }
        return "[" + pieces.join(",") + "]";
      };
      const count = view.getUint32(ptr, true);
      const rows = [];
      for (let i = 0; i < count; i++) rows.push(formatRow(view.getUint32(ptr + 4 + i * 8, true)));
      emit("[" + rows.join(",") + "]");
    },    print_arr_f64: (ptr) => {
      const view = new DataView(instance.exports.memory.buffer);
      const count = view.getUint32(ptr, true);
      const pieces = [];
      for (let i = 0; i < count; i++) pieces.push(formatFloat(view.getFloat64(ptr + 4 + i * 8, true)));
      emit("[" + pieces.join(",") + "]");
    },
    arr_sum_f: (ptr) => {
      const view = new DataView(instance.exports.memory.buffer);
      const count = view.getUint32(ptr, true);
      let total = 0;
      for (let i = 0; i < count; i++) total += view.getFloat64(ptr + 4 + i * 8, true);
      return total;
    },
    arr_minmax_f: (ptr, wantMin) => {
      const view = new DataView(instance.exports.memory.buffer);
      const count = view.getUint32(ptr, true);
      let best = null;
      for (let i = 0; i < count; i++) {
        const value = view.getFloat64(ptr + 4 + i * 8, true);
        if (best === null) best = value;
        else if (wantMin !== 0 ? value < best : value > best) best = value;
      }
      return best === null ? 0 : best;
    },
    arr_index_of_f: (ptr, needle) => {
      const view = new DataView(instance.exports.memory.buffer);
      const count = view.getUint32(ptr, true);
      for (let i = 0; i < count; i++) {
        if (view.getFloat64(ptr + 4 + i * 8, true) === needle) return BigInt(i);
      }
      return -1n;
    },    arr_index_of: (ptr, needle) => {
      const view = new DataView(instance.exports.memory.buffer);
      const count = view.getUint32(ptr, true);
      for (let i = 0; i < count; i++) {
        if (view.getBigInt64(ptr + 4 + i * 8, true) === needle) return BigInt(i);
      }
      return -1n;
    },
    arr_index_of_str: (ptr, keyPtr, keyLen) => {
      const view = new DataView(instance.exports.memory.buffer);
      const decode = (p, l) => new TextDecoder("utf-8").decode(new Uint8Array(instance.exports.memory.buffer, p, l));
      const needle = decode(keyPtr, keyLen);
      const count = view.getUint32(ptr, true);
      for (let i = 0; i < count; i++) {
        const data = view.getUint32(ptr + 4 + i * 8, true);
        const size = view.getUint32(ptr + 8 + i * 8, true);
        if (decode(data, size) === needle) return BigInt(i);
      }
      return -1n;
    },
    arr_minmax: (ptr, wantMin) => {
      const view = new DataView(instance.exports.memory.buffer);
      const count = view.getUint32(ptr, true);
      let best = null;
      for (let i = 0; i < count; i++) {
        const value = view.getBigInt64(ptr + 4 + i * 8, true);
        if (best === null) best = value;
        else if (wantMin !== 0 ? value < best : value > best) best = value;
      }
      return best === null ? 0n : best;
    },
    arr_minmax_str: (ptr, wantMin) => {
      const view = new DataView(instance.exports.memory.buffer);
      const decode = (p, l) => new TextDecoder("utf-8").decode(new Uint8Array(instance.exports.memory.buffer, p, l));
      const count = view.getUint32(ptr, true);
      let best = null;
      for (let i = 0; i < count; i++) {
        const data = view.getUint32(ptr + 4 + i * 8, true);
        const size = view.getUint32(ptr + 8 + i * 8, true);
        const text = decode(data, size);
        if (best === null) best = text;
        else if (wantMin !== 0 ? text < best : text > best) best = text;
      }
      return writeStringBlock(best === null ? "" : best);
    },    print_map: (ptr) => {
      // The browser glue keeps maps in a JS Map table keyed by the map id.
      const map = maps[ptr];
      const pieces = [];
      if (map) {
        for (const [key, value] of map) {
          pieces.push(JSON.stringify(key) + ":" + value.toString());
        }
      }
      emit("{" + pieces.join(",") + "}");
    },    print_arr_arr: (ptr) => {
      const view = new DataView(instance.exports.memory.buffer);
      const formatInner = (inner) => {
        const count = view.getUint32(inner, true);
        const pieces = [];
        for (let i = 0; i < count; i++) pieces.push(view.getBigInt64(inner + 4 + i * 8, true).toString());
        return "[" + pieces.join(",") + "]";
      };
      const len = view.getUint32(ptr, true);
      const pieces = [];
      for (let i = 0; i < len; i++) pieces.push(formatInner(view.getUint32(ptr + 4 + i * 8, true)));
      emit("[" + pieces.join(",") + "]");
    },    print_arr_obj: (table, count, arr) => {
      const view = new DataView(instance.exports.memory.buffer);
      const decode = (p, l) => new TextDecoder("utf-8").decode(new Uint8Array(instance.exports.memory.buffer, p, l));
      const record = (rec) => {
        const pieces = [];
        for (let i = 0; i < count; i++) {
          const block = table + view.getUint32(table + i * 4, true);
          const size = view.getUint32(block, true);
          const key = decode(block + 4, size);
          pieces.push(JSON.stringify(key) + ":" + view.getBigInt64(rec + i * 8, true).toString());
        }
        return "{" + pieces.join(",") + "}";
      };
      const len = view.getUint32(arr, true);
      const pieces = [];
      for (let i = 0; i < len; i++) pieces.push(record(view.getUint32(arr + 4 + i * 8, true)));
      emit("[" + pieces.join(",") + "]");
    },
    fmod: (a, b) => a % b,
    int_to_str: (value) => writeStringBlock(value.toString()),
    bool_to_str: (value) => writeStringBlock(value ? "true" : "false"),
    f64_to_str: (value) => writeStringBlock(formatFloat(value)),
    str_to_int: (ptr, len) => {
      const text = new TextDecoder("utf-8").decode(new Uint8Array(instance.exports.memory.buffer, ptr, len)).trim();
      return BigInt(parseInt(text, 10));
    },
    map_new: () => { maps.push(new Map()); return maps.length - 1; },
    map_set: (id, ptr, len, value) => {
      const key = new TextDecoder("utf-8").decode(new Uint8Array(instance.exports.memory.buffer, ptr, len));
      maps[id].set(key, value);
      return id;
    },
    map_get: (id, ptr, len) => {
      const key = new TextDecoder("utf-8").decode(new Uint8Array(instance.exports.memory.buffer, ptr, len));
      const value = maps[id] && maps[id].get(key);
      return value === undefined ? 0n : BigInt(value);
    },
    map_has: (id, ptr, len) => {
      const key = new TextDecoder("utf-8").decode(new Uint8Array(instance.exports.memory.buffer, ptr, len));
      return maps[id] && maps[id].has(key) ? 1 : 0;
    },
    map_len: (id) => (maps[id] ? maps[id].size : 0),
    map_key_at: (id, index) => {
      const entries = maps[id] ? Array.from(maps[id].keys()) : [];
      return index < entries.length ? writeStringBlock(entries[index]) : 0;
    },
    map_value_at: (id, index) => {
      const entries = maps[id] ? Array.from(maps[id].values()) : [];
      return index < entries.length ? BigInt(entries[index]) : 0n;
    },
    now_ms: () => BigInt(Date.now()),
    sleep_ms: (millis) => {
      const until = Date.now() + Number(millis);
      while (Date.now() < until) { /* busy wait: demos are short */ }
    },
    cwd: () => writeStringBlock(typeof process !== "undefined" ? process.cwd() : "/"),
    env_get_default: () => writeStringBlock(""),
    env_get: () => writeStringBlock(typeof process !== "undefined" ? (process.env.PATH || "") : ""),
    mkdir: () => -1,
    mkdir_soft: () => 0,
    basename: () => 0,
    dirname: () => 0,
    file_exists: (ptr, len) => {
      try {
        if (typeof process !== "undefined" && process.versions && process.versions.node) {
          const path = new TextDecoder("utf-8").decode(
            new Uint8Array(instance.exports.memory.buffer, ptr, len),
          );
          const fs = require("fs");
          return fs.existsSync(path) ? 1 : 0;
        }
      } catch (error) {
        return 0;
      }
      return 0;
    },
    arr_range: (start, stop, step) => {
      const values = [];
      if (step > 0n) {
        for (let value = start; value < stop; value += step) values.push(value);
      }
      const base = nextStr;
      const size = 4 + 8 * values.length;
      nextStr = (base + size + 7) & ~7;
      const view = new DataView(instance.exports.memory.buffer);
      view.setUint32(base, values.length, true);
      values.forEach((value, index) => view.setBigInt64(base + 4 + index * 8, value, true));
      return base;
    },
    arr_sum: (ptr) => {
      const view = new DataView(instance.exports.memory.buffer);
      const count = view.getUint32(ptr, true);
      let total = 0n;
      for (let i = 0; i < count; i++) total += view.getBigInt64(ptr + 4 + i * 8, true);
      return total;
    },
    arr_reverse: (ptr) => {
      const view = new DataView(instance.exports.memory.buffer);
      const count = view.getUint32(ptr, true);
      for (let i = 0; i < Math.floor(count / 2); i++) {
        const left = ptr + 4 + i * 8;
        const right = ptr + 4 + (count - 1 - i) * 8;
        const a = view.getBigInt64(left, true);
        const b = view.getBigInt64(right, true);
        view.setBigInt64(left, b, true);
        view.setBigInt64(right, a, true);
      }
      return ptr;
    },
    arr_sort: (ptr) => {
      const view = new DataView(instance.exports.memory.buffer);
      const count = view.getUint32(ptr, true);
      const values = [];
      for (let i = 0; i < count; i++) values.push(view.getBigInt64(ptr + 4 + i * 8, true));
      values.sort((a, b) => (a < b ? -1 : a > b ? 1 : 0));
      values.forEach((value, index) => view.setBigInt64(ptr + 4 + index * 8, value, true));
      return ptr;
    },    str_split: (ptr, len, sepPtr, sepLen) => {
      const decode = (p, l) => new TextDecoder("utf-8").decode(new Uint8Array(instance.exports.memory.buffer, p, l));
      const text = decode(ptr, len);
      const sep = decode(sepPtr, sepLen);
      if (sep.length === 0) throw new Error("wasm target split() needs a non-empty separator");
      const pieces = text.split(sep);
      const base = nextStr;
      const size = 4 + 8 * pieces.length;
      nextStr = (base + size + 7) & ~7;
      const view = new DataView(instance.exports.memory.buffer);
      view.setUint32(base, pieces.length, true);
      const encoder = new TextEncoder();
      for (let i = 0; i < pieces.length; i++) {
        const block = writeStringBlock(pieces[i]);
        const bytes = encoder.encode(pieces[i]);
        view.setUint32(base + 4 + i * 8, block + 4, true);
        view.setUint32(base + 4 + i * 8 + 4, bytes.length, true);
      }
      return base;
    },
    arr_sort_f: (ptr) => {
      const view = new DataView(instance.exports.memory.buffer);
      const count = view.getUint32(ptr, true);
      const values = [];
      for (let i = 0; i < count; i++) values.push(view.getFloat64(ptr + 4 + i * 8, true));
      values.sort((a, b) => (a < b ? -1 : a > b ? 1 : 0));
      values.forEach((value, index) => view.setFloat64(ptr + 4 + index * 8, value, true));
      return ptr;
    },
    arr_sort_str: (ptr) => {
      const view = new DataView(instance.exports.memory.buffer);
      const decode = (p, l) => new TextDecoder("utf-8").decode(new Uint8Array(instance.exports.memory.buffer, p, l));
      const count = view.getUint32(ptr, true);
      const entries = [];
      for (let i = 0; i < count; i++) {
        entries.push([
          view.getUint32(ptr + 4 + i * 8, true),
          view.getUint32(ptr + 8 + i * 8, true),
        ]);
      }
      entries.sort((a, b) => {
        const left = decode(a[0], a[1]);
        const right = decode(b[0], b[1]);
        return left < right ? -1 : left > right ? 1 : 0;
      });
      entries.forEach((entry, index) => {
        view.setUint32(ptr + 4 + index * 8, entry[0], true);
        view.setUint32(ptr + 8 + index * 8, entry[1], true);
      });
      return ptr;
    },
    arr_join_int: (ptr, sepPtr, sepLen) => {
      const view = new DataView(instance.exports.memory.buffer);
      const decode = (p, l) => new TextDecoder("utf-8").decode(new Uint8Array(instance.exports.memory.buffer, p, l));
      const sep = decode(sepPtr, sepLen);
      const count = view.getUint32(ptr, true);
      const parts = [];
      for (let i = 0; i < count; i++) parts.push(view.getBigInt64(ptr + 4 + i * 8, true).toString());
      return writeStringBlock(parts.join(sep));
    },
    arr_join_f: (ptr, sepPtr, sepLen) => {
      const view = new DataView(instance.exports.memory.buffer);
      const decode = (p, l) => new TextDecoder("utf-8").decode(new Uint8Array(instance.exports.memory.buffer, p, l));
      const sep = decode(sepPtr, sepLen);
      const count = view.getUint32(ptr, true);
      const parts = [];
      for (let i = 0; i < count; i++) parts.push(formatFloat(view.getFloat64(ptr + 4 + i * 8, true)));
      return writeStringBlock(parts.join(sep));
    },    str_join: (arrayPtr, sepPtr, sepLen) => {
      const view = new DataView(instance.exports.memory.buffer);
      const decode = (p, l) => new TextDecoder("utf-8").decode(new Uint8Array(instance.exports.memory.buffer, p, l));
      const sep = decode(sepPtr, sepLen);
      const count = view.getUint32(arrayPtr, true);
      const pieces = [];
      for (let i = 0; i < count; i++) {
        const ptr = view.getUint32(arrayPtr + 4 + i * 8, true);
        const len = view.getUint32(arrayPtr + 4 + i * 8 + 4, true);
        pieces.push(decode(ptr, len));
      }
      return writeStringBlock(pieces.join(sep));
    },
    str_slice: (ptr, len, start, end) => {
      const text = new TextDecoder("utf-8").decode(new Uint8Array(instance.exports.memory.buffer, ptr, len));
      const chars = Array.from(text);
      const clamp = (value) => {
        let at = Number(value);
        if (at < 0) at += chars.length;
        return Math.max(0, Math.min(chars.length, at));
      };
      return writeStringBlock(chars.slice(clamp(start), clamp(end)).join(""));
    },
    arr_slice: (ptr, start, end) => {
      const view = new DataView(instance.exports.memory.buffer);
      const count = view.getUint32(ptr, true);
      const clamp = (value) => {
        let at = Number(value);
        if (at < 0) at += count;
        return Math.max(0, Math.min(count, at));
      };
      const from = clamp(start);
      const to = clamp(end);
      const keep = Math.max(0, to - from);
      const base = nextStr;
      const size = 4 + 8 * keep;
      nextStr = (base + size + 7) & ~7;
      view.setUint32(base, keep, true);
      for (let i = 0; i < keep; i++) {
        view.setBigInt64(base + 4 + i * 8, view.getBigInt64(ptr + 4 + (from + i) * 8, true), true);
      }
      return base;
    },    str_trim: (ptr, len) => {
      const text = new TextDecoder("utf-8").decode(new Uint8Array(instance.exports.memory.buffer, ptr, len));
      return writeStringBlock(text.trim());
    },
    str_upper: (ptr, len) => {
      const text = new TextDecoder("utf-8").decode(new Uint8Array(instance.exports.memory.buffer, ptr, len));
      return writeStringBlock(text.toUpperCase());
    },
    str_lower: (ptr, len) => {
      const text = new TextDecoder("utf-8").decode(new Uint8Array(instance.exports.memory.buffer, ptr, len));
      return writeStringBlock(text.toLowerCase());
    },
    str_replace: (ptr, len, fromPtr, fromLen, toPtr, toLen) => {
      const decode = (p, l) => new TextDecoder("utf-8").decode(new Uint8Array(instance.exports.memory.buffer, p, l));
      const text = decode(ptr, len);
      const from = decode(fromPtr, fromLen);
      const to = decode(toPtr, toLen);
      return writeStringBlock(text.split(from).join(to));
    },
    str_index_of: (ptr, len, needlePtr, needleLen) => {
      const decode = (p, l) => new TextDecoder("utf-8").decode(new Uint8Array(instance.exports.memory.buffer, p, l));
      return BigInt(decode(ptr, len).indexOf(decode(needlePtr, needleLen)));
    },
    str_repeat: (ptr, len, count) => {
      const text = new TextDecoder("utf-8").decode(new Uint8Array(instance.exports.memory.buffer, ptr, len));
      return writeStringBlock(text.repeat(Number(count)));
    },    str_char_at: (ptr, len, index) => {
      const text = new TextDecoder("utf-8").decode(
        new Uint8Array(instance.exports.memory.buffer, ptr, len),
      );
      const chars = Array.from(text);
      let at = index;
      if (at < 0) at += chars.length;
      if (at < 0 || at >= chars.length) throw new Error("index out of bounds");
      return writeStringBlock(chars[at]);
    },
    panic: (ptr, len) => {
      const text = new TextDecoder("utf-8").decode(
        new Uint8Array(instance.exports.memory.buffer, ptr, len),
      );
      throw new Error(text);
    },
    str_test: (ap, al, bp, bl, mode) => {
      const hay = new Uint8Array(instance.exports.memory.buffer, ap, al);
      const needle = new Uint8Array(instance.exports.memory.buffer, bp, bl);
      if (mode === 1 || mode === 2) {
        if (needle.length > hay.length) return 0;
        const start = mode === 1 ? 0 : hay.length - needle.length;
        for (let i = 0; i < needle.length; i++) if (hay[start + i] !== needle[i]) return 0;
        return 1;
      }
      for (let i = 0; i + needle.length <= hay.length; i++) {
        let hit = 1;
        for (let j = 0; j < needle.length; j++) if (hay[i + j] !== needle[j]) hit = 0;
        if (hit) return 1;
      }
      return 0;
    },
    str_compare: (ap, al, bp, bl) => {
      const bytesA = new Uint8Array(instance.exports.memory.buffer, ap, al);
      const bytesB = new Uint8Array(instance.exports.memory.buffer, bp, bl);
      const limit = Math.min(bytesA.length, bytesB.length);
      for (let i = 0; i < limit; i++) {
        if (bytesA[i] < bytesB[i]) return -1;
        if (bytesA[i] > bytesB[i]) return 1;
      }
      if (bytesA.length === bytesB.length) return 0;
      return bytesA.length < bytesB.length ? -1 : 1;
    },
  },
  bx: {
    argc: () => queryArgs.length,
    arg: (index) => {
      if (index < 0 || index >= queryArgs.length) return 0;
      const bytes = new TextEncoder().encode(queryArgs[index]);
      const start = nextArg;
      const view = new DataView(instance.exports.memory.buffer);
      view.setUint32(start, bytes.length, true);
      new Uint8Array(instance.exports.memory.buffer, start + 4, bytes.length).set(bytes);
      nextArg = start + 4 + bytes.length;
      return start;
    },
    read_file: () => 0,
    write_file: () => -1,
    read_file_soft: () => 0,
    write_file_soft: () => 0,
    run: () => -1,
    capture: () => 0,
    which: () => 0,
    exec_record: () => {
      throw new Error("exec() is not available in the browser demo");
    },
    exec_record_soft: () => {
      throw new Error("exec() is not available in the browser demo");
    },
  },
  webgl: {
    create: (width, height) => { gl.width = Math.max(1, width); gl.height = Math.max(1, height); gl.pixels = new Uint8Array(gl.width * gl.height * 4); return 1; },
    clear_color: (r, g, b, a) => { gl.clear = [r, g, b, a]; },
    clear: () => {
      const channel = (v) => Math.round(Math.min(1, Math.max(0, v)) * 255);
      const fill = [channel(gl.clear[0]), channel(gl.clear[1]), channel(gl.clear[2]), channel(gl.clear[3])];
      for (let i = 0; i < gl.pixels.length; i += 4) gl.pixels.set(fill, i);
    },
    read_pixel: (x, y) => {
      if (x < 0 || y < 0 || x >= gl.width || y >= gl.height) return 0;
      const offset = (y * gl.width + x) * 4;
      const packed = ((gl.pixels[offset] << 24) | (gl.pixels[offset + 1] << 16) | (gl.pixels[offset + 2] << 8) | gl.pixels[offset + 3]) >>> 0;
      return packed | 0;
    },
    to_png: () => -1,
  },
  web: {
    console_log_i64: (value) => { emit(value.toString()); },
    console_log_f64: (value) => { emit(formatFloat(value)); },
    console_log_utf8: (ptr, len) => {
      const bytes = new Uint8Array(instance.exports.memory.buffer, ptr, len);
      emit(new TextDecoder("utf-8").decode(bytes));
    },
    performance_now: () => (typeof performance !== "undefined" ? performance.now() : Date.now()),
  },
};
async function main() {
  const bytes = decodeBase64(WASM_BASE64);
  const result = await WebAssembly.instantiate(bytes, imports);
  instance = result.instance;
  instance.exports._start();
}
main().catch((error) => {
  emit("wasm demo failed: " + (error && error.message ? error.message : error));
  if (typeof process !== "undefined") process.exitCode = 1;
});
"#;

const HTML: &str = r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <title>bx wasm demo</title>
  <style>
    body { font-family: ui-monospace, monospace; margin: 2rem; background: #111; color: #eee; }
    pre { background: #1c1c1c; padding: 1rem; border-radius: 6px; }
    .hint { color: #888; }
  </style>
</head>
<body>
  <h1>bx wasm demo</h1>
  <p class="hint">stdout of the compiled script; query args are available as <code>arg(i)</code>.</p>
  <pre id="out"></pre>
  <script>
__WASM_JS__
  </script>
</body>
</html>
"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn base64_decode(text: &str) -> Vec<u8> {
        let mut table = [0u8; 256];
        for (index, byte) in B64.iter().enumerate() {
            table[*byte as usize] = index as u8;
        }
        let mut bytes = Vec::new();
        let mut buffer = 0u32;
        let mut bits = 0u32;
        for byte in text.bytes() {
            if byte == b'=' {
                break;
            }
            let value = table[byte as usize] as u32;
            buffer = (buffer << 6) | value;
            bits += 6;
            if bits >= 8 {
                bits -= 8;
                bytes.push((buffer >> bits) as u8);
            }
        }
        bytes
    }

    #[test]
    fn base64_round_trips() {
        for sample in [b"".as_slice(), b"a", b"ab", b"abc", b"\0asm\x01\0\0\0"] {
            assert_eq!(base64_decode(&base64_encode(sample)), sample);
        }
    }

    #[test]
    fn demo_embeds_a_valid_module() {
        let source = "print(6 * 7)\nprint(\"hi\")";
        let (module, js, html) = build_demo(source).expect("demo");
        assert_eq!(&module[0..4], b"\0asm");
        assert!(html.contains("WebAssembly.instantiate"));
        assert!(html.contains("print_utf8"));
        assert!(js.contains("decodeBase64"));
        // The embedded text decodes back to the exact module bytes.
        let start = js.find("__WASM_BASE64__").is_none();
        assert!(start);
        let marker = "const WASM_BASE64 = \"";
        let body = &js[js.find(marker).expect("marker") + marker.len()..];
        let b64 = &body[..body.find('"').expect("quote")];
        assert_eq!(base64_decode(b64), module);
    }
}
