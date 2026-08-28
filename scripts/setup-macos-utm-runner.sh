#!/bin/bash
# Provision the logged-in macOS UTM account as a runtime-only MiniCon target.
# Run from the shared bridge after mounting it with mount_virtiofs.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
INSTALL_DIR="$HOME/Library/Application Support/MiniCon"
LAUNCH_AGENTS="$HOME/Library/LaunchAgents"
PLIST="$LAUNCH_AGENTS/io.minicon.utm-agent.plist"
LABEL="io.minicon.utm-agent"

AGENT_SOURCE="$SCRIPT_DIR/macos-utm-agent-v2.sh"
[ -f "$AGENT_SOURCE" ] || AGENT_SOURCE="$SCRIPT_DIR/macos-utm-agent.sh"
[ -f "$AGENT_SOURCE" ] || {
  echo "macos-utm-agent.sh must be next to this provisioning script" >&2
  exit 2
}

mkdir -p "$INSTALL_DIR" "$LAUNCH_AGENTS"
install -m 700 "$AGENT_SOURCE" "$INSTALL_DIR/macos-utm-agent.sh"

escaped_program="$(printf '%s' "$INSTALL_DIR/macos-utm-agent.sh" |
  sed -e 's/&/\&amp;/g' -e 's/</\&lt;/g' -e 's/>/\&gt;/g')"
{
  printf '%s\n' '<?xml version="1.0" encoding="UTF-8"?>'
  printf '%s\n' '<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">'
  printf '%s\n' '<plist version="1.0"><dict>'
  printf '%s\n' '  <key>Label</key><string>io.minicon.utm-agent</string>'
  printf '  <key>ProgramArguments</key><array><string>%s</string></array>\n' "$escaped_program"
  printf '%s\n' '  <key>RunAtLoad</key><true/>'
  printf '%s\n' '  <key>KeepAlive</key><true/>'
  printf '%s\n' '  <key>ProcessType</key><string>Background</string>'
  printf '%s\n' '  <key>LowPriorityIO</key><true/>'
  printf '%s\n' '  <key>Nice</key><integer>10</integer>'
  printf '%s\n' '  <key>StandardOutPath</key><string>/tmp/minicon-utm-agent.log</string>'
  printf '%s\n' '  <key>StandardErrorPath</key><string>/tmp/minicon-utm-agent.err</string>'
  printf '%s\n' '</dict></plist>'
} >"$PLIST"
chmod 600 "$PLIST"
plutil -lint "$PLIST"

domain="gui/$(id -u)"
launchctl bootout "$domain/$LABEL" >/dev/null 2>&1 || true
for attempt in 1 2 3 4 5; do
  launchctl bootstrap "$domain" "$PLIST" && break
  [ "$attempt" -lt 5 ] || exit 1
  sleep 1
done
launchctl kickstart -k "$domain/$LABEL"
launchctl print "$domain/$LABEL" | sed -n '1,24p'
