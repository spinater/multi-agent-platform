#!/usr/bin/env bash
# Installs MAP on this machine as a service for the current user: systemd --user on Linux, launchd on macOS.
# Builds the server and dashboard from this checkout, adds a pinned Reflex unless one is on PATH, writes a config
# once (never overwrites it) and starts the service. The agent CLIs (claude, codex, agy) are the ones on your PATH.
#
#   deploy/install.sh [--mode isolated|system] [--home DIR] [--config-dir DIR] [--projects-dir DIR] [--port 4400]
#                     [--bind 127.0.0.1] [--prefix DIR] [--config FILE] [--agent-login isolated|shared] [--no-service]
#   deploy/install.sh --uninstall      # stops and removes the service and the app; keeps config, data, projects
#
# Layout (--home DIR, default ~/MAP):  DIR/app        the binary, dashboard and Reflex (replaced on every install)
#                                      DIR/config     map.toml, templates/, agents/, data/ (database, runs, corpus
#                                                     repos, git key, the default workspace, logs)
#                                      DIR/workspace  the project folder: projects can only be its sub-folders
# --agent-login (new configs only, default isolated): isolated gives MAP's agents their own sign-ins and history in
# DIR/config/agent-home; shared uses your own (~/.claude, ~/.codex, ~/.gemini).
#
# --mode isolated (default): the layout above, MAP's own agent sign-ins and its own tmux server (tmux -L map).
# --mode system: for a machine (VM) provisioned for MAP, MAP becomes one with it: the usual places
#   (~/.config/map/map.toml, ~/.local/share/map for the app and data, `map` linked into ~/.local/bin), your own CLI
#   sign-ins (~/.claude, ~/.codex, ~/.gemini), the default tmux server (plain `tmux attach`), and projects anywhere
#   in your home folder (--projects-dir to narrow it). --home is not used.
set -euo pipefail

PORT=4400
BIND=127.0.0.1
HOME_DIR=""
PREFIX=""
CONFIG=""
CONFIG_DIR=""
PROJECTS_DIR=""
SERVICE=1
UNINSTALL=0
AGENT_LOGIN=""
MODE=isolated
LABEL=com.multi-agent-platform.map
REFLEX_VERSION=v0.2.4
LEGACY_CONFIG="${XDG_CONFIG_HOME:-$HOME/.config}/map/map.toml"
LEGACY_PREFIX="${XDG_DATA_HOME:-$HOME/.local/share}/map"

die_early() { printf 'error: %s\n' "$*" >&2; exit 2; }
abs() { case "$1" in /*) printf '%s' "$1" ;; "~"*) printf '%s' "$HOME${1#\~}" ;; *) printf '%s/%s' "$PWD" "$1" ;; esac; }

while [ $# -gt 0 ]; do
  case "$1" in
    --port) PORT="$2"; shift 2 ;;
    --bind) BIND="$2"; shift 2 ;;
    --home) HOME_DIR="$(abs "$2")"; shift 2 ;;
    --config-dir) CONFIG_DIR="$(abs "$2")"; shift 2 ;;
    --projects-dir) PROJECTS_DIR="$(abs "$2")"; shift 2 ;;
    --prefix) PREFIX="$(abs "$2")"; shift 2 ;;
    --config) CONFIG="$(abs "$2")"; shift 2 ;;
    --mode)
      case "$2" in isolated|system) MODE="$2" ;; *) echo "--mode takes isolated or system" >&2; exit 2 ;; esac
      shift 2 ;;
    --agent-login)
      case "$2" in isolated|shared) AGENT_LOGIN="$2" ;; *) echo "--agent-login takes isolated or shared" >&2; exit 2 ;; esac
      shift 2 ;;
    --no-service) SERVICE=0; shift ;;
    --uninstall) UNINSTALL=1; shift ;;
    -h|--help) sed -n '2,/^set -euo/p' "$0" | sed '$d'; exit 0 ;;
    *) echo "unknown option $1" >&2; exit 2 ;;
  esac
done

# Where everything goes. --home sets all three; --config-dir / --projects-dir / --prefix override one each. With no
# flags, an install made before this layout (~/.config/map/map.toml) keeps its config and prefix.
if [ -z "$HOME_DIR" ] && [ -z "$CONFIG_DIR" ] && [ -z "$CONFIG" ] && [ -f "$LEGACY_CONFIG" ]; then
  CONFIG="$LEGACY_CONFIG"
  PREFIX="${PREFIX:-$LEGACY_PREFIX}"
fi
if [ "$MODE" = system ]; then
  [ -z "$HOME_DIR" ] || die_early "--home is for --mode isolated; --mode system uses ~/.config/map and ~/.local/share/map"
  PREFIX="${PREFIX:-$LEGACY_PREFIX}"
  CONFIG="${CONFIG:-${CONFIG_DIR:+$CONFIG_DIR/map.toml}}"
  CONFIG="${CONFIG:-$LEGACY_CONFIG}"
  CONFIG_DIR="$(dirname "$CONFIG")"
  PROJECTS_DIR="${PROJECTS_DIR:-$HOME}"
  DATA_DIR="$PREFIX/data"
  AGENT_LOGIN="${AGENT_LOGIN:-shared}"
  TMUX_SOCKET=""
else
  HOME_DIR="${HOME_DIR:-$HOME/MAP}"
  PREFIX="${PREFIX:-$HOME_DIR/app}"
  if [ -n "$CONFIG" ]; then
    CONFIG_DIR="$(dirname "$CONFIG")"
  else
    CONFIG_DIR="${CONFIG_DIR:-$HOME_DIR/config}"
    CONFIG="$CONFIG_DIR/map.toml"
  fi
  PROJECTS_DIR="${PROJECTS_DIR:-$HOME_DIR/workspace}"
  DATA_DIR="$CONFIG_DIR/data"
  AGENT_LOGIN="${AGENT_LOGIN:-isolated}"
  TMUX_SOCKET=map
fi

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
  # Only the app: config, data and projects stay (an older prefix may hold data/ and workspace/, which are kept).
  for b in map map-tell map-ask; do
    [ "$(readlink "$HOME/.local/bin/$b" 2>/dev/null)" = "$PREFIX/bin/$b" ] && rm -f "$HOME/.local/bin/$b"
  done
  rm -rf "$PREFIX/bin" "$PREFIX/dashboard"
  rmdir "$PREFIX" 2>/dev/null || true
  say "service and app removed; config $CONFIG_DIR, data $DATA_DIR and projects $PROJECTS_DIR are kept"
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

mkdir -p "$PREFIX/bin" "$CONFIG_DIR/templates" "$CONFIG_DIR/agents" "$PROJECTS_DIR"
install -m 755 "$REPO/backend/target/release/map" "$PREFIX/bin/map"
# The lane helpers: map-tell queues a tell, map-ask asks the project's Reflex or knowledge from a lane MAP did not start.
install -m 755 "$REPO/deploy/map-tell" "$REPO/deploy/map-ask" "$PREFIX/bin/"
if [ "$MODE" = system ]; then
  # One with the machine: `map` and the lane helpers on PATH like any other tool.
  mkdir -p "$HOME/.local/bin"
  for b in map map-tell map-ask; do ln -sf "$PREFIX/bin/$b" "$HOME/.local/bin/$b"; done
fi
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

if [ -f "$CONFIG" ]; then
  say "keeping the existing config $CONFIG"
  if ! grep -Eq '^[[:space:]]*(path|root|roots)[[:space:]]*=' "$CONFIG"; then
    echo "    it sets no project folder, so projects can only be made under ~/project. To use $PROJECTS_DIR, add:"
    printf '      [projects]\n      path = "%s"\n' "$PROJECTS_DIR"
    echo "    to $CONFIG (one [projects] table only) and restart MAP."
  fi
  if [ "$MODE" = isolated ] && ! grep -Eq '^[[:space:]]*\[agents_home\]' "$CONFIG"; then
    echo "    its agents share your own CLI sign-ins (no [agents_home]). To give them their own, add:"
    printf '      [agents_home]\n      mode = "isolated"\n'
    echo "    restart MAP and sign each provider in under Settings -> Agent CLIs."
  fi
else
  mkdir -p "$DATA_DIR/workspace"
  # The workspace for tasks without a project must be a git repo. It lives in data/, so it is never a project.
  [ -d "$DATA_DIR/workspace/.git" ] || git init -q "$DATA_DIR/workspace"
  gemini_binary=gemini
  command -v gemini >/dev/null || ! command -v agy >/dev/null || gemini_binary=agy
  init_agent="# init_agent = \"$CONFIG_DIR/agents/init-agent.md\""
  [ -f "$CONFIG_DIR/agents/init-agent.md" ] && init_agent="init_agent = \"$CONFIG_DIR/agents/init-agent.md\""
  planner_agent="# planner_agent = \"$CONFIG_DIR/agents/planner-agent.md\""
  tmux_socket="# tmux_socket = \"map\"   # MAP's sessions on their own tmux server (tmux -L map attach -t <session>)"
  [ -n "$TMUX_SOCKET" ] && tmux_socket="# MAP's sessions live on their own tmux server: tmux -L $TMUX_SOCKET attach -t <session>
tmux_socket = \"$TMUX_SOCKET\""
  [ -f "$CONFIG_DIR/agents/planner-agent.md" ] && planner_agent="planner_agent = \"$CONFIG_DIR/agents/planner-agent.md\""
  (umask 077 && cat >"$CONFIG" <<EOF
# Written by deploy/install.sh. Every setting is documented in map.example.toml.
[server]
# isolated: MAP keeps to its own folder, sign-ins and tmux server. system: one with this machine (your sign-ins,
# your tmux server, projects anywhere in your home folder). Informational; the settings below are what count.
mode = "$MODE"
bind = "$BIND:$PORT"
# The dashboard asks for this once per browser. Keep it secret.
api_token = "$(openssl rand -hex 32)"
static_dir = "$PREFIX/dashboard"
data_dir = "$DATA_DIR"
# Public host names that reach this server (e.g. a Cloudflare tunnel), so their Host header is accepted.
# allowed_hosts = ["agents.example.com"]

[workspace]
# Where tasks and sessions without a project run. Projects use their own folders.
dir = "$DATA_DIR/workspace"
session_prefix = "map-"
$tmux_socket

[workspace.env]
# The PATH the installer ran with, so the service finds the same agent CLIs as your shell.
PATH = "$PATH"

[agents_home]
# isolated: MAP's agents sign in with their own accounts and keep their own history, in $CONFIG_DIR/agent-home
# (claude/, codex/, gemini-home/); sign each one in once under Settings → Agent CLIs. Your own claude, codex and
# agy in a terminal are unaffected. shared: they use your own sign-ins and history (~/.claude, ~/.codex, ~/.gemini).
mode = "$AGENT_LOGIN"

[orchestrator]
autostart = false
max_parallel = 1
provider_order = ["claude", "codex", "gemini"]

[providers.claude]
# Unattended workers stop at every permission prompt without this; only enable it if you accept that risk.
# extra_args = ["--dangerously-skip-permissions"]

[providers.gemini]
binary = "$gemini_binary"

# Also built in and opt-in (add them to provider_order above; see map.example.toml for every field):
#   cursor      the Cursor CLI (cursor-agent); sign in under Settings -> Agent CLIs, or set CURSOR_API_KEY
#   copilot     the GitHub Copilot CLI (copilot); sign in there, or set COPILOT_GITHUB_TOKEN
#   openrouter  Claude Code through OpenRouter: set OPENROUTER_API_KEY in this service's environment (never here)
# [providers.copilot]
# extra_args = ["--allow-all-tools"]

[reflex]
binary = "$REFLEX"

[projects]
# The project folder: projects can only be created in its sub-folders, and the dashboard's folder picker browses
# nothing else. Change it here (or reinstall with --projects-dir) and restart MAP.
path = "$PROJECTS_DIR"

[templates]
# Your own project templates (one folder with a template.toml each), next to the built-in ones.
dirs = ["$CONFIG_DIR/templates"]
# Instructions that replace the built-in init agent's (written when $CONFIG_DIR/agents/init-agent.md exists).
$init_agent
# Instructions that replace the built-in planner agent's (written when $CONFIG_DIR/agents/planner-agent.md exists).
$planner_agent
EOF
  )
  say "wrote $CONFIG"
fi

"$PREFIX/bin/map" check -c "$CONFIG" || true

LOG_DIR="$DATA_DIR"
[ -d "$LOG_DIR" ] || LOG_DIR="$PREFIX/data"
layout() {
  say "layout"
  echo "    app       $PREFIX            (map, dashboard, reflex: replaced on every install)"
  if [ "$MODE" = system ]; then
    echo "    mode      system: your own CLI sign-ins, the default tmux server (tmux attach -t map-…), \`map\` in ~/.local/bin"
    echo "    config    $CONFIG_DIR        (map.toml, templates/, agents/)"
  else
    echo "    mode      isolated: MAP's own sign-ins (Settings -> Agent CLIs) and tmux server (tmux -L map attach -t …)"
    echo "    config    $CONFIG_DIR        (map.toml, templates/, agents/, agent-home/ with isolated agent logins)"
  fi
  echo "    data      $DATA_DIR          (database, runs, corpus repos, git key, default workspace)"
  echo "    projects  $PROJECTS_DIR      (the project folder: projects are its sub-folders)"
}

if [ "$SERVICE" = 0 ]; then
  layout
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
    mkdir -p "$(dirname "$PLIST")" "$LOG_DIR"
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
  <key>StandardOutPath</key><string>$LOG_DIR/map.log</string>
  <key>StandardErrorPath</key><string>$LOG_DIR/map.log</string>
</dict>
</plist>
EOF
    launchctl bootstrap "gui/$(id -u)" "$PLIST"
    say "service $LABEL started (log: $LOG_DIR/map.log)" ;;
esac

for _ in $(seq 1 30); do
  curl -s -o /dev/null "http://127.0.0.1:$PORT/api/health" && break
  sleep 1
done
layout
say "open http://127.0.0.1:$PORT and paste the api_token from $CONFIG"
