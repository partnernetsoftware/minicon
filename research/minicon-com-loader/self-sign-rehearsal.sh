#!/bin/bash
# Prove that a signable unsigned APE survives an Authenticode transform.
# This uses an ephemeral untrusted certificate and is NEVER G6 evidence.
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/../.." && pwd)
INPUT=${1:?usage: self-sign-rehearsal.sh UNSIGNED_MINICON_COM [OUTPUT_DIR]}
OUTPUT_DIR=${2:-"$ROOT/research/minicon-com-loader/dist/self-sign-rehearsal"}
CEILING=9437184

command -v openssl >/dev/null
command -v osslsigncode >/dev/null
test -f "$INPUT"
mkdir -p "$OUTPUT_DIR"

LAB=$(mktemp -d "${TMPDIR:-/tmp}/minicon-self-sign.XXXXXX")
cleanup() {
  rm -rf -- "$LAB"
}
trap cleanup EXIT HUP INT TERM

before=$(shasum -a 256 "$INPUT" | awk '{print $1}')
python3 - "$INPUT" <<'PY'
import struct, sys
from pathlib import Path

raw = Path(sys.argv[1]).read_bytes()
pe = struct.unpack_from("<I", raw, 0x3C)[0]
if raw[pe:pe + 4] != b"PE\0\0":
    raise SystemExit("unsigned input lacks PE signature")
optional = pe + 24
size = struct.unpack_from("<H", raw, pe + 20)[0]
count = struct.unpack_from("<I", raw, optional + 108)[0]
security_offset, security_bytes = struct.unpack_from("<II", raw, optional + 112 + 4 * 8)
if (size, count, security_offset, security_bytes) != (240, 16, 0, 0):
    raise SystemExit(
        f"unsigned input is not signable: optional={size} directories={count} "
        f"security=({security_offset},{security_bytes})"
    )
PY

openssl req -x509 -newkey rsa:2048 -nodes -days 1 -sha256 \
  -subj '/C=AU/O=MINICON REHEARSAL ONLY/CN=UNTRUSTED SELF-SIGN TEST' \
  -addext 'extendedKeyUsage=codeSigning' \
  -keyout "$LAB/key.pem" -out "$LAB/cert.pem" >/dev/null 2>&1

SIGNED="$OUTPUT_DIR/minicon-self-signed.com"
SIGNED_TMP="$LAB/minicon-self-signed.com"
osslsigncode sign -h sha256 -certs "$LAB/cert.pem" -key "$LAB/key.pem" \
  -n 'MiniCon untrusted self-sign rehearsal' -in "$INPUT" -out "$SIGNED_TMP" >/dev/null
mv -f -- "$SIGNED_TMP" "$SIGNED"
osslsigncode verify -CAfile "$LAB/cert.pem" -in "$SIGNED" >"$LAB/verify.log"
grep -q 'Signature verification: ok' "$LAB/verify.log"

cp "$SIGNED" "$LAB/tampered.com"
python3 - "$LAB/tampered.com" <<'PY'
import sys
from pathlib import Path

path = Path(sys.argv[1])
raw = bytearray(path.read_bytes())
raw[4096] ^= 1
path.write_bytes(raw)
PY
set +e
osslsigncode verify -CAfile "$LAB/cert.pem" -in "$LAB/tampered.com" >"$LAB/tampered.log" 2>&1
tampered_rc=$?
set -e
test "$tampered_rc" -ne 0

after=$(shasum -a 256 "$SIGNED" | awk '{print $1}')
test "$after" != "$before"
bytes=$(wc -c <"$SIGNED" | tr -d ' ')
test "$bytes" -le "$CEILING"
unzip -tq "$SIGNED" >/dev/null
chmod +x "$SIGNED"
version=$($SIGNED --version | tr -d '\r')
status=$($SIGNED --status)
test "$version" = 'minicon 0.1.3'
printf '%s\n' "$status" | grep -q 'pty backend'

python3 - "$SIGNED" <<'PY'
import struct, sys
from pathlib import Path

raw = Path(sys.argv[1]).read_bytes()
pe = struct.unpack_from("<I", raw, 0x3C)[0]
optional = pe + 24
security_offset, security_bytes = struct.unpack_from("<II", raw, optional + 112 + 4 * 8)
if security_offset <= 0 or security_bytes <= 0:
    raise SystemExit("signed APE did not acquire an Authenticode certificate table")
if security_offset + security_bytes > len(raw):
    raise SystemExit("signed APE certificate table exceeds file bounds")
print(f"security_file_offset={security_offset}")
print(f"security_bytes={security_bytes}")
PY

python3 - "$OUTPUT_DIR/rehearsal-receipt.json" "$before" "$after" "$bytes" "$version" <<'PY'
import json, sys
from pathlib import Path

output, before, after, size, version = sys.argv[1:]
receipt = {
    "schema": 1,
    "kind": "minicon-self-sign-rehearsal",
    "evidence_scope": "mechanism-only-not-g6",
    "trusted": False,
    "timestamped": False,
    "organization": "MINICON REHEARSAL ONLY",
    "before_sha256": before,
    "after_sha256": after,
    "after_bytes": int(size),
    "candidate_ceiling_bytes": 9437184,
    "product_version": version,
    "checks": [
        "empty-security-directory-before",
        "authenticode-table-after",
        "signature-verifies-against-ephemeral-test-ca",
        "signed-byte-tamper-rejected",
        "zip-central-directory-readable",
        "darwin-version-and-status-execute",
    ],
}
Path(output).write_text(json.dumps(receipt, indent=2) + "\n", encoding="utf-8")
PY

printf 'PASS self-sign rehearsal (not G6)\n'
printf 'before_sha256=%s\nafter_sha256=%s\nafter_bytes=%s\n' "$before" "$after" "$bytes"
