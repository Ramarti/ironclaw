#!/usr/bin/env bash
# Push a new IronClaw binary to a running server and restart.
#
# Usage:
#   ./scripts/update.sh                           # interactive
#   DROPLET_IP=1.2.3.4 ./scripts/update.sh        # non-interactive
#   SSH_KEY=~/.ssh/id_rsa DROPLET_IP=1.2.3.4 ./scripts/update.sh

set -euo pipefail

GRN='\033[1;32m'
CYN='\033[1;36m'
RED='\033[1;31m'
BLD='\033[1m'
RST='\033[0m'

YLW='\033[1;33m'

log()  { printf "${GRN}>>>${RST} %s\n" "$*"; }
warn() { printf "${YLW}WARN:${RST} %s\n" "$*" >&2; }
die()  { printf "${RED}ERROR:${RST} %s\n" "$*" >&2; exit 1; }

prompt() {
	local var_name="$1" prompt_text="$2"
	if [ -z "${!var_name:-}" ]; then
		printf "${CYN}?${RST} %s: " "$prompt_text" >&2
		read -r "${var_name?}"
		[ -n "${!var_name:-}" ] || die "$var_name is required"
	fi
}

REPO_DIR="$(cd "$(dirname "$0")/.." && pwd)"
BUILD_FEATURES="libsql,html-to-markdown"
REMOTE_BIN="/opt/ironclaw/bin/ironclaw"

# ── Collect info ─────────────────────────────────────────────────────

prompt DROPLET_IP "Server IP address"

SSH_USER="${SSH_USER:-root}"
SSH_KEY="${SSH_KEY:-}"

if [ -z "$SSH_KEY" ]; then
	log "Available SSH keys:"
	for key in ~/.ssh/id_*.pub; do
		[ -f "$key" ] && echo "  ${key%.pub}"
	done
	prompt SSH_KEY "SSH private key path"
fi
SSH_KEY="${SSH_KEY/#\~/$HOME}"
[ -f "$SSH_KEY" ] || die "SSH key not found: $SSH_KEY"

ssh_cmd() { ssh -i "$SSH_KEY" -o ConnectTimeout=10 "$SSH_USER@$DROPLET_IP" "$@"; }

# ── Build ────────────────────────────────────────────────────────────

if [ "$(uname -s)" = "Linux" ] && [ "$(uname -m)" = "x86_64" ]; then
	log "Building (native linux/amd64)..."
	cargo build --release --no-default-features --features "$BUILD_FEATURES" \
		--manifest-path "$REPO_DIR/Cargo.toml"
	BINARY="$REPO_DIR/target/release/ironclaw"
else
	command -v docker >/dev/null || die "Docker required for cross-compilation"
	log "Cross-compiling via Docker (linux/amd64)..."
	docker run --rm \
		--platform linux/amd64 \
		-v "$REPO_DIR":/app \
		-w /app \
		rust:1.92-slim-bookworm \
		bash -c "
			apt-get update -qq && apt-get install -y -qq pkg-config libssl-dev cmake >/dev/null 2>&1
			cargo build --release \
				--no-default-features \
				--features '$BUILD_FEATURES'
		"
	BINARY="$REPO_DIR/target/x86_64-unknown-linux-gnu/release/ironclaw"
	# Fallback: Docker on amd64 host writes to target/release/
	[ -f "$BINARY" ] || BINARY="$REPO_DIR/target/release/ironclaw"
fi

[ -f "$BINARY" ] || die "Build failed: $BINARY not found"
log "Binary: $(du -h "$BINARY" | cut -f1)"

# ── Upload and restart ──────────────────────────────────────────────

log "Uploading to $DROPLET_IP..."
scp -i "$SSH_KEY" -q "$BINARY" "$SSH_USER@$DROPLET_IP:/tmp/ironclaw-bin"

log "Installing and restarting..."
ssh_cmd "install -m 755 -o ironclaw -g ironclaw /tmp/ironclaw-bin $REMOTE_BIN && rm -f /tmp/ironclaw-bin && systemctl restart ironclaw"

sleep 2
STATUS=$(ssh_cmd "systemctl is-active ironclaw" 2>/dev/null || echo "unknown")

if [ "$STATUS" = "active" ]; then
	log "Update complete — service is ${BLD}active${RST}"
else
	warn "Service status: $STATUS"
	echo "  Check logs: ssh -i $SSH_KEY $SSH_USER@$DROPLET_IP journalctl -fu ironclaw"
fi
