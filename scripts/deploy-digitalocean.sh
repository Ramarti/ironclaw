#!/usr/bin/env bash
# Deploy IronClaw to a DigitalOcean Droplet (Ubuntu 22.04/24.04).
#
# Builds from source with the libSQL backend (no PostgreSQL dependency).
# Sets up Caddy for automatic HTTPS and systemd for process management.
#
# Usage:
#   DOMAIN=ironclaw.example.com ANTHROPIC_API_KEY=sk-ant-... ./scripts/deploy-digitalocean.sh
#
# Or run interactively — the script prompts for required values.

set -euo pipefail

# ── Helpers ──────────────────────────────────────────────────────────

log() { printf '\033[1;32m>>>\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33mWARN:\033[0m %s\n' "$*" >&2; }
die() {
	printf '\033[1;31mERROR:\033[0m %s\n' "$*" >&2
	exit 1
}

prompt_required() {
	local var_name="$1" prompt_text="$2"
	if [ -z "${!var_name:-}" ]; then
		printf '%s: ' "$prompt_text" >&2
		read -r "${var_name?}"
		[ -n "${!var_name:-}" ] || die "$var_name is required"
	fi
}

prompt_secret() {
	local var_name="$1" prompt_text="$2"
	if [ -z "${!var_name:-}" ]; then
		printf '%s: ' "$prompt_text" >&2
		read -rs "${var_name?}"
		echo >&2
		[ -n "${!var_name:-}" ] || die "$var_name is required"
	fi
}

generate_token() { openssl rand -hex 32; }

# ── Pre-flight checks ───────────────────────────────────────────────

[ "$(id -u)" -eq 0 ] || die "Run this script as root (or via sudo)"

log "IronClaw DigitalOcean deployment"

# Collect required configuration
prompt_required DOMAIN "Domain name pointing to this server (e.g. ironclaw.example.com)"
LLM_BACKEND="${LLM_BACKEND:-anthropic}"
prompt_secret ANTHROPIC_API_KEY "Anthropic API key (from console.anthropic.com)"
GATEWAY_AUTH_TOKEN="${GATEWAY_AUTH_TOKEN:-$(generate_token)}"

INSTALL_DIR="/opt/ironclaw"
REPO_URL="https://github.com/nearai/ironclaw.git"
BRANCH="${BRANCH:-main}"

# ── System dependencies ─────────────────────────────────────────────

log "Installing system dependencies..."
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq \
	build-essential pkg-config libssl-dev git curl ufw \
	debian-keyring debian-archive-keyring apt-transport-https

# ── Caddy ────────────────────────────────────────────────────────────

if ! command -v caddy &>/dev/null; then
	log "Installing Caddy..."
	curl -1sLf 'https://dl.cloudsmith.io/public/caddy/stable/gpg.key' |
		gpg --dearmor -o /usr/share/keyrings/caddy-stable-archive-keyring.gpg
	curl -1sLf 'https://dl.cloudsmith.io/public/caddy/stable/debian.deb.txt' |
		tee /etc/apt/sources.list.d/caddy-stable.list
	apt-get update -qq
	apt-get install -y -qq caddy
else
	log "Caddy already installed, skipping"
fi

# ── Firewall ─────────────────────────────────────────────────────────

log "Configuring firewall..."
ufw --force reset >/dev/null
ufw default deny incoming
ufw default allow outgoing
ufw allow 22/tcp  # SSH
ufw allow 80/tcp  # HTTP (ACME challenges)
ufw allow 443/tcp # HTTPS
ufw --force enable

# ── Rust toolchain ───────────────────────────────────────────────────

if ! command -v rustup &>/dev/null; then
	log "Installing Rust toolchain..."
	curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs |
		sh -s -- -y --default-toolchain stable
fi
# shellcheck source=/dev/null
source "$HOME/.cargo/env"
rustup update stable --no-self-update
log "Rust $(rustc --version)"

# ── Create ironclaw user ────────────────────────────────────────────

if ! id ironclaw &>/dev/null; then
	log "Creating ironclaw system user..."
	useradd --system --create-home --home-dir "$INSTALL_DIR" \
		--shell /usr/sbin/nologin ironclaw
else
	log "User ironclaw already exists"
fi

# ── Directory structure ──────────────────────────────────────────────

log "Setting up directory structure..."
mkdir -p "$INSTALL_DIR"/{bin,data,personas,skills,tools,channels,src}
chown -R ironclaw:ironclaw "$INSTALL_DIR"

# ── Clone / update source ───────────────────────────────────────────

if [ -d "$INSTALL_DIR/src/.git" ]; then
	log "Updating source..."
	git -C "$INSTALL_DIR/src" fetch origin
	git -C "$INSTALL_DIR/src" checkout "$BRANCH"
	git -C "$INSTALL_DIR/src" reset --hard "origin/$BRANCH"
else
	log "Cloning IronClaw ($BRANCH)..."
	git clone --branch "$BRANCH" "$REPO_URL" "$INSTALL_DIR/src"
fi

# ── Swap file (for builds on small droplets) ─────────────────────────

TOTAL_MEM_KB=$(awk '/MemTotal/ {print $2}' /proc/meminfo)
if [ "$TOTAL_MEM_KB" -lt 4000000 ] && [ ! -f /swapfile ]; then
	log "Adding 2GB swap file for build (low memory detected)..."
	fallocate -l 2G /swapfile
	chmod 600 /swapfile
	mkswap /swapfile
	swapon /swapfile
	echo '/swapfile none swap sw 0 0' >>/etc/fstab
fi

# ── Build ────────────────────────────────────────────────────────────

log "Building IronClaw (release, libsql backend)..."
cd "$INSTALL_DIR/src"
cargo build --release --no-default-features --features "libsql,html-to-markdown"

cp target/release/ironclaw "$INSTALL_DIR/bin/ironclaw"
chown ironclaw:ironclaw "$INSTALL_DIR/bin/ironclaw"
log "Binary installed at $INSTALL_DIR/bin/ironclaw"

# ── Environment file ─────────────────────────────────────────────────

log "Writing .env..."
cat >"$INSTALL_DIR/.env" <<EOF
# IronClaw configuration — generated by deploy-digitalocean.sh

# Database
DATABASE_BACKEND=libsql
LIBSQL_PATH=$INSTALL_DIR/data/ironclaw.db

# LLM provider
LLM_BACKEND=$LLM_BACKEND
ANTHROPIC_API_KEY=$ANTHROPIC_API_KEY

# Agent
AGENT_NAME=ironclaw

# Channels — headless server, web gateway only
CLI_ENABLED=false
GATEWAY_ENABLED=true
GATEWAY_HOST=127.0.0.1
GATEWAY_PORT=3000
GATEWAY_AUTH_TOKEN=$GATEWAY_AUTH_TOKEN
GATEWAY_USER_ID=default

# Workspace
SKILLS_ENABLED=true
ROUTINES_ENABLED=true

# Logging
RUST_LOG=ironclaw=info
EOF

chmod 600 "$INSTALL_DIR/.env"
chown ironclaw:ironclaw "$INSTALL_DIR/.env"

# ── systemd unit ─────────────────────────────────────────────────────

log "Installing systemd service..."
cat >/etc/systemd/system/ironclaw.service <<EOF
[Unit]
Description=IronClaw AI Assistant
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=ironclaw
Group=ironclaw
WorkingDirectory=$INSTALL_DIR
EnvironmentFile=$INSTALL_DIR/.env
ExecStart=$INSTALL_DIR/bin/ironclaw
Restart=on-failure
RestartSec=5
LimitNOFILE=65536

[Install]
WantedBy=multi-user.target
EOF

systemctl daemon-reload
systemctl enable ironclaw

# ── Caddy configuration ─────────────────────────────────────────────

log "Configuring Caddy for $DOMAIN..."
cat >/etc/caddy/Caddyfile <<EOF
$DOMAIN {
    reverse_proxy localhost:3000
}
EOF

# ── Start services ───────────────────────────────────────────────────

log "Starting services..."
systemctl restart caddy
systemctl restart ironclaw

# Wait briefly for startup
sleep 2

# ── Summary ──────────────────────────────────────────────────────────

echo
echo "============================================"
echo "  IronClaw deployed successfully"
echo "============================================"
echo
echo "  URL:        https://$DOMAIN"
echo "  Auth token: $GATEWAY_AUTH_TOKEN"
echo
echo "  Useful commands:"
echo "    systemctl status ironclaw    # Service status"
echo "    journalctl -fu ironclaw      # Live logs"
echo "    systemctl restart ironclaw   # Restart"
echo
echo "  Verify:"
echo "    curl -H 'Authorization: Bearer $GATEWAY_AUTH_TOKEN' https://$DOMAIN/api/health"
echo
echo "  Config:  $INSTALL_DIR/.env"
echo "  Data:    $INSTALL_DIR/data/"
echo "  Binary:  $INSTALL_DIR/bin/ironclaw"
echo "============================================"
