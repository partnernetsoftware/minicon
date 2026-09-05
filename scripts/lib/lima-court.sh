# Locate lima-court in the utm-court operator repo. Source this file.

_MINICON_LIMA_COURT_LIB_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

minicon_lima_court_cli() {
  _minicon_lima_court_is_trampoline() {
    [ -f "$1" ] && grep -q 'Trampoline: MiniCon does not own the Lima court' "$1" 2>/dev/null
  }
  _minicon_lima_court_is_real() {
    [ -n "$1" ] && [ -x "$1" ] && [ -f "$1" ] || return 1
    _minicon_lima_court_is_trampoline "$1" && return 1
    grep -q 'Container-like lifecycle facade for Lima' "$1" 2>/dev/null
  }

  if [ -n "${MINICON_LIMA_COURT_CLI:-}" ] && ! _minicon_lima_court_is_trampoline "$MINICON_LIMA_COURT_CLI"; then
    if _minicon_lima_court_is_real "$MINICON_LIMA_COURT_CLI" || [ -x "$MINICON_LIMA_COURT_CLI" ]; then
      printf '%s\n' "$MINICON_LIMA_COURT_CLI"
      return 0
    fi
  fi
  if _minicon_lima_court_is_real "${UTM_COURT_HOME:-}/bin/lima-court"; then
    printf '%s\n' "$UTM_COURT_HOME/bin/lima-court"
    return 0
  fi
  _here="$(cd "$_MINICON_LIMA_COURT_LIB_DIR/../.." && pwd)"
  for _candidate in \
    "$_here/../utm-court/bin/lima-court" \
    "${HOME}/repos/utm-court/bin/lima-court"
  do
    if _minicon_lima_court_is_real "$_candidate"; then
      printf '%s\n' "$_candidate"
      return 0
    fi
  done
  echo "lima-court CLI not found; clone partnernetsoftware/utm-court or set UTM_COURT_HOME" >&2
  return 2
}
