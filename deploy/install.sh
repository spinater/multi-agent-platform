#!/usr/bin/env bash
# Installs MAP on this machine as a service for the current user: systemd --user on Linux, launchd on macOS.
# Builds the server and dashboard from this checkout, adds a pinned Reflex unless one is on PATH, writes a config
# once (never overwrites it) and starts the service. The agent CLIs (claude, codex, agy) are the ones on your PATH.
#
#   deploy/install.sh [--port 4400] [--bind 127.0.0.1] [--prefix DIR] [--config FILE] [--no-service]
#   deploy/install.sh --uninstall      # stops and removes the service; keeps the config and data
set -euo pipefail

PORT=4400
BIND=127.0.0.1
PREFIX="${XDG_DATA_HOME:-$HOME/.local/share}/map"
CONFIG="${XDG_CONFIG_HOME:-$HOME/.config}/map/map.toml"
SERVICE=1
UNINSTALL=0
LABEL=com.multi-agent-platform.map
REFLEX_VERSION=v0.2.4

while [ $# -gt 0 ]; do
  case "$1" in
    --port) PORT="$2"; shift 2 ;;
    --bind) BIND="$2"; shift 2 ;;
    --prefix) PREFIX="$2"; shift 2 ;;
    --config) CONFIG="$2"; shift 2 ;;
    --no-service) SERVICE=0; shift ;;
    --uninstall) UNINSTALL=1; shift ;;
    -h|--help) sed -n '2,8p' "$0"; exit 0 ;;
    *) echo "unknown option $1" >&2; exit 2 ;;
  esac
done

OS="$(uname -s)"
ARCH="$(uname -m)"
REPO="$(cd "$(dirname "$0")/.." && pwd)"
UNIT="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user/map.service"
PLIST="$HOME/Library/LaunchAgents/$LABEL.plist"
say() { printf '\033[1m==>\033[0m %s\n' "$*"; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }

stop_service() {
  case "$OS" in
    Linux)
      if [ -f "$UNIT" ]; then
        systemctl --user disable --now map.service 2>/dev/null || true
        rm -f "$UNIT"
        systemctl --user daemon-reload
      fi ;;
    Darwin)
      if [ -f "$PLIST" ]; then
        launchctl bootout "gui/$(id -u)" "$PLIST" 2>/dev/null || true
        rm -f "$PLIST"
      fi ;;
  esac
}

if [ "$UNINSTALL" = 1 ]; then
  stop_service
  rm -rf "$PREFIX/bin" "$PREFIX/dashboard"
  say "service removed; config $CONFIG and data $PREFIX/data are kept"
  exit 0
fi

case "$OS" in Linux|Darwin) ;; *) die "only Linux and macOS are supported" ;; esac
for tool in cargo bun tmux git curl openssl; do
  command -v "$tool" >/dev/null || die "$tool is required (and must be on PATH)"
done

say "building the server"
(cd "$REPO/backend" && cargo build --release --locked)
say "building the dashboard"
(cd "$REPO/frontend" && bun install --frozen-lockfile && bun run build)

mkdir -p "$PREFIX/bin" "$PREFIX/data" "$PREFIX/workspace"
install -m 755 "$REPO/backend/target/release/map" "$PREFIX/bin/map"
rm -rf "$PREFIX/dashboard" && cp -R "$REPO/frontend/out" "$PREFIX/dashboard"

# Reflex: the one on PATH, else a pinned release checked against its SHA256.
REFLEX="$(command -v reflex || true)"
if [ -z "$REFLEX" ]; then
  case "$OS-$ARCH" in
    Linux-x86_64) target=x86_64-unknown-linux-musl; sum=30ad90eeaf6b476a84b4d9e959fd9e0f9cf9464708fd9355f30abf792169c322 ;;
    Linux-aarch64|Linux-arm64) target=aarch64-unknown-linux-musl; sum=638f99652ae8cc014d18f568acd2d6ddd15bd6611ec22c5ae5e2aecfcec65357 ;;
    Darwin-arm64) target=aarch64-apple-darwin; sum=65fecfdd401fc77e15d1745ee95a694a6fbc603c7121c0e265a4808f8751df2f ;;
    Darwin-x86_64) target=x86_64-apple-darwin; sum=e9365aa3bdfefb771ac0e000ddd145bcc0bf90456a3329c1f47d9c1344b39a0a ;;
    *) die "no Reflex build for $OS $ARCH; install it yourself and rerun" ;;
  esac
  asset="reflex-$REFLEX_VERSION-$target.tar.gz"
  tmp="$(mktemp -d)"
  say "downloading Reflex $REFLEX_VERSION"
  curl -fsSL -o "$tmp/$asset" "https://github.com/gist-rs/reflex/releases/download/$REFLEX_VERSION/$asset"
  if command -v sha256sum >/dev/null; then got="$(sha256sum "$tmp/$asset")"; else got="$(shasum -a 256 "$tmp/$asset")"; fi
  [ "${got%% *}" = "$sum" ] || die "Reflex checksum mismatch"
  tar -xzf "$tmp/$asset" -C "$PREFIX/bin" reflex
  rm -rf "$tmp"
  REFLEX="$PREFIX/bin/reflex"
fi

# The workspace for tasks without a project must be a git repo.
[ -d "$PREFIX/workspace/.git" ] || git init -q "$PREFIX/workspace"

if [ -f "$CONFIG" ]; then
  say "keeping the existing config $CONFIG"
else
  mkdir -p "$(dirname "$CONFIG")"
  gemini_binary=gemini
  command -v gemini >/dev/null || ! command -v agy >/dev/null || gemini_binary=agy
  (umask 077 && cat >"$CONFIG" <<EOF
# Written by deploy/install.sh. Every setting is documented in map.example.toml.
[server]
bind = "$BIND:$PORT"
# The dashboard asks for this once per browser. Keep it secret.
api_token = "$(openssl rand -hex 32)"
static_dir = "$PREFIX/dashboard"
data_dir = "$PREFIX/data"
# Public host names that reach this server (e.g. a Cloudflare tunnel), so their Host header is accepted.
# allowed_hosts = ["agents.example.com"]

[workspace]
# Where tasks and sessions without a project run. Projects use their own folders.
dir = "$PREFIX/workspace"
session_prefix = "map-"

[workspace.env]
# The PATH the installer ran with, so the service finds the same agent CLIs as your shell.
PATH = "$PATH"

[orchestrator]
autostart = false
max_parallel = 1
provider_order = ["claude", "codex", "gemini"]

[providers.claude]
# Unattended workers stop at every permission prompt without this; only enable it if you accept that risk.
# extra_args = ["--dangerously-skip-permissions"]

[providers.gemini]
binary = "$gemini_binary"

[reflex]
binary = "$REFLEX"
EOF
  )
  say "wrote $CONFIG"
fi

"$PREFIX/bin/map" check -c "$CONFIG" || true

if [ "$SERVICE" = 0 ]; then
  say "installed; run it with: $PREFIX/bin/map serve -c $CONFIG"
  exit 0
fi

stop_service
case "$OS" in
  Linux)
    mkdir -p "$(dirname "$UNIT")"
    cat >"$UNIT" <<EOF
[Unit]
Description=multi-agent-platform (MAP)
After=network-online.target

[Service]
ExecStart=$PREFIX/bin/map serve -c $CONFIG
Environment=PATH=$PATH
WorkingDirectory=$HOME
Restart=on-failure
RestartSec=5
# Only the server stops on restart: agent sessions in tmux keep running and are picked up again.
KillMode=process

[Install]
WantedBy=default.target
EOF
    systemctl --user daemon-reload
    systemctl --user enable --now map.service
    say "service map.service started (systemctl --user status map, journalctl --user -u map -f)"
    if [ "$(loginctl show-user "$USER" -p Linger --value 2>/dev/null || echo no)" != yes ]; then
      echo "    to keep it running after you log out: sudo loginctl enable-linger $USER"
    fi ;;
  Darwin)
    mkdir -p "$(dirname "$PLIST")" "$PREFIX/data"
    cat >"$PLIST" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>$LABEL</string>
  <key>ProgramArguments</key>
  <array><string>$PREFIX/bin/map</string><string>serve</string><string>-c</string><string>$CONFIG</string></array>
  <key>EnvironmentVariables</key><dict><key>PATH</key><string>$PATH</string></dict>
  <key>WorkingDirectory</key><string>$HOME</string>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><dict><key>SuccessfulExit</key><false/></dict>
  <key>AbandonProcessGroup</key><true/>
  <key>StandardOutPath</key><string>$PREFIX/data/map.log</string>
  <key>StandardErrorPath</key><string>$PREFIX/data/map.log</string>
</dict>
</plist>
EOF
    launchctl bootstrap "gui/$(id -u)" "$PLIST"
    say "service $LABEL started (log: $PREFIX/data/map.log)" ;;
esac

for _ in $(seq 1 30); do
  curl -s -o /dev/null "http://127.0.0.1:$PORT/api/health" && break
  sleep 1
done
say "open http://127.0.0.1:$PORT and paste the api_token from $CONFIG"
