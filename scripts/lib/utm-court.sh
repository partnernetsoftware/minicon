# Locate partnernetsoftware/utm-court. Source this file; do not execute it.
# MiniCon product runners call the court; they do not own utmctl.

_MINICON_UTM_COURT_LIB_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

minicon_utm_court_cli() {
  _minicon_utm_court_is_trampoline() {
    [ -f "$1" ] && grep -q 'Trampoline: MiniCon does not own the UTM court' "$1" 2>/dev/null
  }
  _minicon_utm_court_is_real() {
    [ -n "$1" ] && [ -x "$1" ] && [ -f "$1" ] || return 1
    _minicon_utm_court_is_trampoline "$1" && return 1
    grep -q 'Uniform, product-neutral lifecycle' "$1" 2>/dev/null
  }

  if [ -n "${MINICON_UTM_COURT_CLI:-}" ] && ! _minicon_utm_court_is_trampoline "$MINICON_UTM_COURT_CLI"; then
    if _minicon_utm_court_is_real "$MINICON_UTM_COURT_CLI" || [ -x "$MINICON_UTM_COURT_CLI" ]; then
      printf '%s\n' "$MINICON_UTM_COURT_CLI"
      return 0
    fi
  fi
  if _minicon_utm_court_is_real "${UTM_COURT_HOME:-}/bin/utm-court"; then
    printf '%s\n' "$UTM_COURT_HOME/bin/utm-court"
    return 0
  fi
  if command -v utm-court >/dev/null 2>&1; then
    _found="$(command -v utm-court)"
    if _minicon_utm_court_is_real "$_found"; then
      printf '%s\n' "$_found"
      return 0
    fi
  fi
  _here="$(cd "$_MINICON_UTM_COURT_LIB_DIR/../.." && pwd)"
  for _candidate in \
    "$_here/../utm-court/bin/utm-court" \
    "${HOME}/repos/utm-court/bin/utm-court"
  do
    if _minicon_utm_court_is_real "$_candidate"; then
      printf '%s\n' "$_candidate"
      return 0
    fi
  done
  echo "utm-court CLI not found; clone partnernetsoftware/utm-court or set UTM_COURT_HOME" >&2
  return 2
}

minicon_utm_court_home() {
  _cli="$(minicon_utm_court_cli)" || return $?
  cd "$(dirname "$_cli")/.." && pwd
}
