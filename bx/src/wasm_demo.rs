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
    },
    print_f64: (value) => { emit(formatFloat(value)); },
    fmod: (a, b) => a % b,
    int_to_str: (value) => writeStringBlock(value.toString()),
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
    env_get: () => writeStringBlock(typeof process !== "undefined" ? (process.env.PATH || "") : ""),
    mkdir: () => -1,
    file_exists: () => 0,
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
    run: () => -1,
    capture: () => 0,
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
