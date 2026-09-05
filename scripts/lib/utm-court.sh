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

  for _override in "${UTM_COURT_CLI:-}" "${MINICON_UTM_COURT_CLI:-}"; do
    if [ -n "$_override" ] && ! _minicon_utm_court_is_trampoline "$_override"; then
      if _minicon_utm_court_is_real "$_override" || [ -x "$_override" ]; then
        printf '%s\n' "$_override"
        return 0
      fi
    fi
  done
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
