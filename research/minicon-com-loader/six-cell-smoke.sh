#!/bin/bash
# Execute minicon.com --status on every court this host can actually reach.
# Missing court = BLOCKED, never PASS. Do not publish on BLOCKED/FAIL.
set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
COM="${1:-$HERE/dist/minicon.com}"
PATH="$HOME/cosmocc/bin:$PATH"
export PATH
PASS=0
FAIL=0
BLOCK=0

record() {
  printf '%-16s %-8s %s\n' "$1" "$2" "$3"
  case "$2" in
    PASS) PASS=$((PASS + 1)) ;;
    FAIL) FAIL=$((FAIL + 1)) ;;
    BLOCKED) BLOCK=$((BLOCK + 1)) ;;
  esac
}

need_com() {
  if [[ ! -x "$COM" ]]; then
    echo "missing $COM" >&2
    exit 2
  fi
}

expect_status() {
  cell="$1"
  out="$2"
  if printf '%s' "$out" | grep -q 'pty backend'; then
    ver=$(printf '%s\n' "$out" | sed -n '1p')
    record "$cell" PASS "$ver"
  else
    record "$cell" FAIL "$(printf '%s' "$out" | tr '\n' ' ' | cut -c1-160)"
  fi
}

lima_status() {
  cell="$1"
  instance="$2"
  if ! command -v limactl >/dev/null; then
    record "$cell" BLOCKED "limactl missing"
    return
  fi
  if ! limactl start "$instance" >/dev/null 2>&1; then
    record "$cell" BLOCKED "limactl start $instance failed"
    return
  fi
  remote="/tmp/minicon.com"
  if ! limactl copy "$COM" "$instance:$remote" >/dev/null 2>&1; then
    record "$cell" FAIL "limactl copy failed"
    return
  fi
  out=$(limactl shell "$instance" -- sh -c "chmod +x $remote && $remote --status" 2>&1)
  rc=$?
  if [[ $rc -ne 0 ]]; then
    record "$cell" FAIL "exit=$rc ${out}" 
    return
  fi
  expect_status "$cell" "$out"
}

need_com
echo "com=$COM"
echo "size=$(wc -c <"$COM" | tr -d ' ') sha256=$(shasum -a 256 "$COM" | awk '{print $1}')"
echo

# 1. Darwin aarch64: this host, cosmocc arm64 slice
out=$( "$COM" --status 2>&1 )
rc=$?
if [[ $rc -eq 0 ]]; then expect_status osx-aarch64 "$out"
else record osx-aarch64 FAIL "exit=$rc $out"
fi

# 2. Darwin x86_64 via Rosetta: cosmocc amd64 slice
if arch -x86_64 /usr/bin/true >/dev/null 2>&1; then
  out=$( arch -x86_64 "$COM" --status 2>&1 )
  rc=$?
  if [[ $rc -eq 0 ]]; then expect_status osx-x86_64 "$out"
  else record osx-x86_64 FAIL "exit=$rc $out"
  fi
else
  record osx-x86_64 BLOCKED "Rosetta 2 missing"
fi

# 3–4. Linux Lima
lima_status lnx-aarch64 minicon-lnx-aarch64
lima_status lnx-x86_64 minicon-lnx-x86_64

# 5–6. Windows UTM — APE as-is; court must already exist
if [[ -x "$HERE/../../scripts/windows-utm-runner.sh" ]] &&
   [[ -x /Applications/UTM.app/Contents/MacOS/utmctl ]]; then
  for cell in win-aarch64 win-x86_64; do
    record "$cell" BLOCKED "APE-on-UTM not wired (runner expects cargo target dir, not minicon.com)"
  done
else
  record win-aarch64 BLOCKED "utmctl/windows-utm-runner missing"
  record win-x86_64 BLOCKED "utmctl/windows-utm-runner missing"
fi

echo
echo "pass=$PASS fail=$FAIL blocked=$BLOCK"
if [[ "$FAIL" -gt 0 ]]; then exit 1; fi
if [[ "$PASS" -lt 6 ]]; then exit 3; fi
exit 0
