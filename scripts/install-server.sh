#!/bin/sh
# Installs (or updates) kvistcode-server for the current user, sets it up to start on boot,
# and prints a pairing link. Safe to run again: that updates it and restarts the service.
#
#   curl -fsSL https://github.com/pl0xuee/kvistcode/releases/latest/download/kvistcode-server-install.sh | sh
#
# Options (after `sh -s --` when piping):
#   --tailscale     reach it from your devices over Tailscale (default when Tailscale is running)
#   --lan           reach it from your home network (plain HTTP; trusted networks only)
#   --local         this machine only (use an SSH tunnel from elsewhere)
#   --no-service    don't set up start-on-boot
#   --uninstall     stop and remove it (your data folder is kept)
#
# Environment: KVISTCODE_BIN_DIR (default ~/.local/bin); KVISTCODE_DOWNLOAD_BASE (a mirror of
# the release downloads, for testing).
set -eu

VERSION="@VERSION@" # filled in by the release; a copy from the repository asks GitHub
REPO="pl0xuee/kvistcode"
BIN_DIR="${KVISTCODE_BIN_DIR:-$HOME/.local/bin}"
BIN="$BIN_DIR/kvistcode-server"
UNIT_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"
UNIT="$UNIT_DIR/kvistcode-server.service"
MODE=auto
SERVICE=1
UNINSTALL=0

say() { printf '%s\n' "$*"; }
step() { printf '\n\033[1m%s\033[0m\n' "$*"; }
die() { printf 'kvistcode-server install: %s\n' "$*" >&2; exit 1; }

for arg in "$@"; do
  case "$arg" in
    --tailscale) MODE=tailscale ;;
    --lan) MODE=lan ;;
    --local) MODE=local ;;
    --no-service) SERVICE=0 ;;
    --uninstall) UNINSTALL=1 ;;
    -h | --help) sed -n '2,16p' "$0" 2>/dev/null | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) die "unknown option '$arg' (see --help)" ;;
  esac
done

has() { command -v "$1" >/dev/null 2>&1; }
have_user_systemd() { has systemctl && systemctl --user show-environment >/dev/null 2>&1; }

if [ "$UNINSTALL" = 1 ]; then
  step "Removing kvistcode-server"
  if have_user_systemd && [ -f "$UNIT" ]; then
    systemctl --user disable --now kvistcode-server >/dev/null 2>&1 || true
    rm -f "$UNIT"
    systemctl --user daemon-reload || true
  fi
  rm -f "$BIN"
  say "Removed. Your agents and settings are still in ~/.local/share/kvistcode-server; delete that folder to remove them too."
  exit 0
fi

# ---- checks -------------------------------------------------------------------------
[ "$(uname -s)" = Linux ] || die "the ready-made server is for Linux. On other systems, build it from source (docs/HEADLESS.md)."
case "$(uname -m)" in
  x86_64 | amd64) ARCH=x86_64 ;;
  aarch64 | arm64) ARCH=aarch64 ;;
  *) die "there's no ready-made server for $(uname -m). Build it from source (docs/HEADLESS.md)." ;;
esac
if [ "$(id -u)" = 0 ] && [ "${KVISTCODE_ALLOW_ROOT:-}" != 1 ]; then
  die "don't install as root: agents run commands as the user that runs the server. Log in as your normal user and run this again (or set KVISTCODE_ALLOW_ROOT=1 if you really mean it)."
fi
for tool in tar sha256sum; do has "$tool" || die "'$tool' is missing; install it and run this again."; done
if has curl; then
  fetch() { curl -fsSL --retry 3 -o "$2" "$1"; }
  fetch_text() { curl -fsSL --retry 3 "$1"; }
elif has wget; then
  fetch() { wget -q -O "$2" "$1"; }
  fetch_text() { wget -q -O - "$1"; }
else
  die "this needs curl or wget; install one and run this again."
fi

if [ "$VERSION" = "@""VERSION@" ]; then
  VERSION=$(fetch_text "https://api.github.com/repos/$REPO/releases/latest" | sed -n 's/.*"tag_name": *"v\{0,1\}\([^"]*\)".*/\1/p' | head -n1)
  [ -n "$VERSION" ] || die "couldn't find the latest version on GitHub; try again in a minute."
fi

# ---- download -----------------------------------------------------------------------
NAME="kvistcode-server-$VERSION-linux-$ARCH"
URL="${KVISTCODE_DOWNLOAD_BASE:-https://github.com/$REPO/releases/download}/v$VERSION/$NAME.tar.gz"
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

step "Downloading Kvistcode server $VERSION ($ARCH)"
fetch "$URL" "$TMP/$NAME.tar.gz" || die "couldn't download $URL"
fetch "$URL.sha256" "$TMP/$NAME.tar.gz.sha256" || die "couldn't download the checksum"
(cd "$TMP" && sha256sum -c --status "$NAME.tar.gz.sha256") || die "the download is damaged (checksum mismatch); run this again."
tar -xzf "$TMP/$NAME.tar.gz" -C "$TMP"

UPDATING=0
[ -x "$BIN" ] && UPDATING=1
mkdir -p "$BIN_DIR"
# Rename into place, so a running server keeps its old file until it restarts.
cp "$TMP/$NAME/kvistcode-server" "$BIN_DIR/.kvistcode-server.new"
chmod 755 "$BIN_DIR/.kvistcode-server.new"
mv -f "$BIN_DIR/.kvistcode-server.new" "$BIN"
"$BIN" --version >/dev/null || die "the server doesn't start on this machine (it needs glibc 2.35 or newer: Ubuntu 22.04, Debian 12 or later)."
say "Installed $BIN"

case ":$PATH:" in
  *":$BIN_DIR:"*) ;;
  *)
    if [ "$BIN_DIR" = "$HOME/.local/bin" ] && ! grep -qs '.local/bin' "$HOME/.profile"; then
      printf '\n# Added by the Kvistcode server installer\nexport PATH="$HOME/.local/bin:$PATH"\n' >>"$HOME/.profile"
      say "Added ~/.local/bin to your PATH in ~/.profile (open a new terminal to use the 'kvistcode-server' command)."
    else
      say "Note: $BIN_DIR isn't on your PATH; run it as $BIN."
    fi
    ;;
esac

# ---- how it is reached --------------------------------------------------------------
if [ "$MODE" = auto ]; then
  if [ -f "$UNIT" ]; then
    # Keep what the last install chose.
    if grep -q -- '--tailscale' "$UNIT"; then MODE=tailscale
    elif grep -q -- '--lan' "$UNIT"; then MODE=lan
    else MODE=local; fi
  elif has tailscale && tailscale status >/dev/null 2>&1; then
    MODE=tailscale
  else
    MODE=local
  fi
fi
FLAGS=""
[ "$MODE" = tailscale ] && FLAGS=" --tailscale"
[ "$MODE" = lan ] && FLAGS=" --lan"

# ---- start on boot ------------------------------------------------------------------
STARTED=0
if [ "$SERVICE" = 1 ] && have_user_systemd; then
  step "Starting it (and on every boot)"
  mkdir -p "$UNIT_DIR"
  cat >"$UNIT" <<EOF
[Unit]
Description=Kvistcode server
After=network-online.target
Wants=network-online.target

[Service]
ExecStart=$BIN$FLAGS
Restart=on-failure
TimeoutStopSec=30

[Install]
WantedBy=default.target
EOF
  systemctl --user daemon-reload
  systemctl --user enable kvistcode-server >/dev/null 2>&1
  systemctl --user restart kvistcode-server
  # Keep it running when you log out (some systems ask for your password here).
  if ! loginctl show-user "$(id -un)" -p Linger 2>/dev/null | grep -q yes; then
    loginctl enable-linger "$(id -un)" 2>/dev/null ||
      say "Note: run 'sudo loginctl enable-linger $(id -un)' so it keeps running after you log out."
  fi
  STARTED=1
elif [ "$SERVICE" = 1 ]; then
  say "No systemd user session here, so it won't start on boot by itself."
fi

# ---- pairing ------------------------------------------------------------------------
if [ "$STARTED" = 1 ]; then
  step "Pair your browser"
  link=""
  i=0
  while [ $i -lt 60 ]; do
    if link=$("$BIN" --pair 2>/dev/null); then break; fi
    link=""
    i=$((i + 1))
    sleep 1
  done
  if [ -n "$link" ]; then
    printf '%s\n' "$link"
  else
    say "The server is starting; get a pairing link in a moment with: kvistcode-server --pair"
    say "If it doesn't come up, see: journalctl --user -u kvistcode-server -e"
  fi
else
  step "Start it"
  say "Run: $BIN$FLAGS"
  say "It prints a pairing link to open in your browser."
fi

step "Next steps"
case "$MODE" in
  tailscale) say "Open the link on any device signed in to your tailnet." ;;
  lan) say "Open the link on a device on the same network. Plain HTTP: only use this on a network you trust." ;;
  local)
    say "It only listens on this machine. From your computer, open a tunnel first:"
    say "    ssh -L 47821:127.0.0.1:47821 $(id -un)@$(hostname)"
    say "then open the link. (Install Tailscale and run this again to skip the tunnel.)"
    ;;
esac
say "Install and sign in to the agent tools you use on this machine once (Claude Code: 'claude' then /login; Codex: 'codex login'); see the guide."
if [ "$UPDATING" = 1 ]; then
  say "Updated to $VERSION. Your agents and settings are kept."
fi
say "Updates install by themselves when no agent is working. Update now: kvistcode-server --update"
say "Guide: https://github.com/$REPO/blob/main/docs/SERVER-SETUP.md"
