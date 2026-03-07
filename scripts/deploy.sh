#!/usr/bin/env bash
# Deploy IronClaw to a DigitalOcean Droplet (or any Ubuntu 22.04/24.04 server).
#
# Interactive script — prompts for everything it needs.
# Can also be driven non-interactively via environment variables.
#
# What it does:
#   1. Provisions a droplet (optional, requires doctl)
#   2. Cross-compiles the binary (Docker on macOS, native on Linux x86_64)
#   3. Uploads binary + install payload via SSH
#   4. Installs Caddy (HTTPS), UFW firewall, systemd service
#   5. Uses libSQL (embedded) — no PostgreSQL dependency
#
# Usage:
#   ./scripts/deploy.sh                    # fully interactive
#   DROPLET_IP=1.2.3.4 ./scripts/deploy.sh # skip droplet creation
#
# Environment overrides (all optional, prompted if missing):
#   DROPLET_IP, DOMAIN, SSH_KEY, SSH_USER, LLM_BACKEND,
#   ANTHROPIC_API_KEY, OPENAI_API_KEY, NEARAI_API_KEY,
#   GATEWAY_AUTH_TOKEN, BRANCH

set -euo pipefail

# ── Helpers ──────────────────────────────────────────────────────────

RED='\033[1;31m'
GRN='\033[1;32m'
YLW='\033[1;33m'
CYN='\033[1;36m'
BLD='\033[1m'
RST='\033[0m'

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

prompt_secret() {
	local var_name="$1" prompt_text="$2"
	if [ -z "${!var_name:-}" ]; then
		printf "${CYN}?${RST} %s: " "$prompt_text" >&2
		read -rs "${var_name?}"
		echo >&2
		[ -n "${!var_name:-}" ] || die "$var_name is required"
	fi
}

prompt_optional() {
	local var_name="$1" prompt_text="$2" default="${3:-}"
	if [ -z "${!var_name:-}" ]; then
		if [ -n "$default" ]; then
			printf "${CYN}?${RST} %s [${BLD}%s${RST}]: " "$prompt_text" "$default" >&2
		else
			printf "${CYN}?${RST} %s (optional): " "$prompt_text" >&2
		fi
		read -r "${var_name?}"
		if [ -z "${!var_name:-}" ] && [ -n "$default" ]; then
			printf -v "$var_name" '%s' "$default"
		fi
	fi
}

choose() {
	local var_name="$1" prompt_text="$2"
	shift 2
	local options=("$@")
	printf "\n${BLD}%s${RST}\n" "$prompt_text" >&2
	local i=1
	for opt in "${options[@]}"; do
		printf "  ${CYN}%d)${RST} %s\n" "$i" "$opt" >&2
		((i++))
	done
	printf "${CYN}?${RST} Choose [1-%d]: " "${#options[@]}" >&2
	local choice
	read -r choice
	if ! [[ "$choice" =~ ^[0-9]+$ ]] || [ "$choice" -lt 1 ] || [ "$choice" -gt "${#options[@]}" ]; then
		die "Invalid choice: $choice"
	fi
	printf -v "$var_name" '%s' "$choice"
}

confirm() {
	local prompt_text="$1"
	printf "${CYN}?${RST} %s [y/N]: " "$prompt_text" >&2
	local ans
	read -r ans
	[[ "$ans" =~ ^[Yy] ]]
}

generate_token() { openssl rand -hex 32; }

ssh_cmd() { ssh -i "$SSH_KEY" -o ConnectTimeout=10 -o StrictHostKeyChecking=accept-new "$SSH_USER@$DROPLET_IP" "$@"; }
scp_cmd() { scp -i "$SSH_KEY" -q "$@"; }

# ── Banner ───────────────────────────────────────────────────────────

echo
printf '%s' "$BLD"
cat <<'BANNER'
  ╦╦═╗╔═╗╔╗╔╔═╗╦  ╔═╗╦ ╦
  ║╠╦╝║ ║║║║║  ║  ╠═╣║║║
  ╩╩╚═╚═╝╝╚╝╚═╝╩═╝╩ ╩╚╩╝
BANNER
printf '%s' "$RST"
echo "  Deploy to DigitalOcean"
echo

# ── Pre-flight ───────────────────────────────────────────────────────

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

command -v ssh >/dev/null || die "ssh not found"

# ── Step 1: Server ───────────────────────────────────────────────────

log "Step 1/5: Server"

if [ -n "${DROPLET_IP:-}" ]; then
	log "Using provided server: $DROPLET_IP"
else
	choose SERVER_CHOICE "How do you want to connect?" \
		"Create a new DigitalOcean droplet (requires doctl)" \
		"Use an existing server (any Ubuntu 22.04/24.04)"

	if [ "$SERVER_CHOICE" = "1" ]; then
		command -v doctl >/dev/null || die "doctl not found. Install: brew install doctl && doctl auth init"

		log "Fetching regions..."
		echo
		doctl compute region list --format Slug,Name,Available --no-header | while IFS= read -r line; do
			echo "  $line"
		done
		echo
		prompt DO_REGION "Region slug (e.g. nyc1, sfo3, ams3)"

		choose DROPLET_SIZE "Droplet size" \
			"s-1vcpu-2gb  — \$12/mo (minimum, tight builds)" \
			"s-2vcpu-4gb  — \$24/mo (recommended)" \
			"s-4vcpu-8gb  — \$48/mo (fast builds, room to grow)"

		case "$DROPLET_SIZE" in
			1) SIZE_SLUG="s-1vcpu-2gb" ;;
			2) SIZE_SLUG="s-2vcpu-4gb" ;;
			3) SIZE_SLUG="s-4vcpu-8gb" ;;
		esac

		log "Fetching SSH keys from your DO account..."
		echo
		doctl compute ssh-key list --format ID,Name,FingerPrint --no-header | while IFS= read -r line; do
			echo "  $line"
		done
		echo
		prompt DO_SSH_KEY_ID "SSH key ID from the list above"

		DROPLET_NAME="${DROPLET_NAME:-ironclaw}"

		log "Creating droplet: $DROPLET_NAME ($SIZE_SLUG in $DO_REGION)..."
		DROPLET_ID=$(doctl compute droplet create "$DROPLET_NAME" \
			--size "$SIZE_SLUG" \
			--image ubuntu-24-04-x64 \
			--region "$DO_REGION" \
			--ssh-keys "$DO_SSH_KEY_ID" \
			--enable-monitoring \
			--wait \
			--format ID \
			--no-header)

		DROPLET_IP=$(doctl compute droplet get "$DROPLET_ID" --format PublicIPv4 --no-header)
		log "Droplet created: $DROPLET_IP (ID: $DROPLET_ID)"

		log "Waiting for SSH to become available..."
		for _ in $(seq 1 30); do
			if ssh_cmd true 2>/dev/null; then break; fi
			sleep 5
		done
	else
		if command -v doctl >/dev/null 2>&1; then
			log "Your existing droplets:"
			echo
			doctl compute droplet list --format ID,Name,PublicIPv4,Region,Status --no-header | while IFS= read -r line; do
				echo "  $line"
			done
			echo
		fi
		prompt DROPLET_IP "Server IP address"
	fi
fi

SSH_USER="${SSH_USER:-root}"
prompt_optional SSH_USER "SSH user" "$SSH_USER"

# SSH key selection
if [ -z "${SSH_KEY:-}" ]; then
	log "Available SSH keys:"
	for key in ~/.ssh/id_*.pub; do
		[ -f "$key" ] && echo "  ${key%.pub}"
	done
	prompt SSH_KEY "SSH private key path (e.g. ~/.ssh/id_ed25519)"
fi
SSH_KEY="${SSH_KEY/#\~/$HOME}"
[ -f "$SSH_KEY" ] || die "SSH key not found: $SSH_KEY"

log "Verifying SSH connectivity to $SSH_USER@$DROPLET_IP..."
ssh_cmd true || die "Cannot SSH into $SSH_USER@$DROPLET_IP"

# ── Step 2: LLM Provider ────────────────────────────────────────────

log "Step 2/5: LLM Provider"

if [ -z "${LLM_BACKEND:-}" ]; then
	choose LLM_CHOICE "Which LLM provider?" \
		"Anthropic (Claude)" \
		"OpenAI (GPT)" \
		"NEAR AI" \
		"OpenAI-compatible (OpenRouter, Together AI, vLLM, etc.)" \
		"Ollama (local, requires Ollama on server)" \
		"Tinfoil (private TEE inference)"

	case "$LLM_CHOICE" in
		1) LLM_BACKEND="anthropic" ;;
		2) LLM_BACKEND="openai" ;;
		3) LLM_BACKEND="nearai" ;;
		4) LLM_BACKEND="openai_compatible" ;;
		5) LLM_BACKEND="ollama" ;;
		6) LLM_BACKEND="tinfoil" ;;
	esac
fi

LLM_ENV_LINES=""

case "$LLM_BACKEND" in
	anthropic)
		prompt_secret ANTHROPIC_API_KEY "Anthropic API key (from console.anthropic.com)"
		LLM_ENV_LINES="ANTHROPIC_API_KEY=$ANTHROPIC_API_KEY"
		;;
	openai)
		prompt_secret OPENAI_API_KEY "OpenAI API key (from platform.openai.com)"
		LLM_ENV_LINES="OPENAI_API_KEY=$OPENAI_API_KEY"
		;;
	nearai)
		prompt_secret NEARAI_API_KEY "NEAR AI API key (from cloud.near.ai)"
		prompt_optional NEARAI_MODEL "Model name" "claude-3-5-sonnet-20241022"
		LLM_ENV_LINES=$(cat <<-ENVEOF
			NEARAI_API_KEY=$NEARAI_API_KEY
			NEARAI_MODEL=$NEARAI_MODEL
		ENVEOF
		)
		;;
	openai_compatible)
		prompt LLM_BASE_URL "API base URL (e.g. https://openrouter.ai/api/v1)"
		prompt_secret LLM_API_KEY "API key"
		prompt LLM_MODEL "Model name (e.g. anthropic/claude-sonnet-4)"
		prompt_optional LLM_EXTRA_HEADERS "Extra headers (Key:Value,Key2:Value2)" ""
		LLM_ENV_LINES=$(cat <<-ENVEOF
			LLM_BASE_URL=$LLM_BASE_URL
			LLM_API_KEY=$LLM_API_KEY
			LLM_MODEL=$LLM_MODEL
		ENVEOF
		)
		[ -n "${LLM_EXTRA_HEADERS:-}" ] && LLM_ENV_LINES="${LLM_ENV_LINES}
LLM_EXTRA_HEADERS=$LLM_EXTRA_HEADERS"
		;;
	ollama)
		prompt_optional OLLAMA_HOST "Ollama host" "http://127.0.0.1:11434"
		prompt LLM_MODEL "Model name (e.g. llama3, mistral)"
		LLM_ENV_LINES=$(cat <<-ENVEOF
			OLLAMA_HOST=$OLLAMA_HOST
			LLM_MODEL=$LLM_MODEL
		ENVEOF
		)
		;;
	tinfoil)
		prompt_secret TINFOIL_API_KEY "Tinfoil API key"
		prompt_optional TINFOIL_MODEL "Model name" "kimi-k2-5"
		LLM_ENV_LINES=$(cat <<-ENVEOF
			TINFOIL_API_KEY=$TINFOIL_API_KEY
			TINFOIL_MODEL=$TINFOIL_MODEL
		ENVEOF
		)
		;;
	*)
		die "Unknown LLM backend: $LLM_BACKEND"
		;;
esac

# ── Step 3: Domain & Auth ────────────────────────────────────────────

log "Step 3/5: Domain & Auth"

prompt_optional DOMAIN "Domain name for HTTPS (leave empty for HTTP-only)" ""
GATEWAY_AUTH_TOKEN="${GATEWAY_AUTH_TOKEN:-$(generate_token)}"
BRANCH="${BRANCH:-$(git -C "$REPO_DIR" rev-parse --abbrev-ref HEAD)}"

# ── Step 4: Build ────────────────────────────────────────────────────

log "Step 4/5: Building binary"

BUILD_FEATURES="libsql,html-to-markdown"
BINARY_PATH=""

if [ "$(uname -s)" = "Linux" ] && [ "$(uname -m)" = "x86_64" ]; then
	log "Building natively (linux/amd64, branch: $BRANCH)..."
	cd "$REPO_DIR"
	cargo build --release --no-default-features --features "$BUILD_FEATURES"
	BINARY_PATH="$REPO_DIR/target/release/ironclaw"
else
	command -v docker >/dev/null || die "Docker is required for cross-compilation (you're not on Linux x86_64)"
	log "Cross-compiling via Docker (linux/amd64, branch: $BRANCH)..."
	cd "$REPO_DIR"

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
	BINARY_PATH="$REPO_DIR/target/x86_64-unknown-linux-gnu/release/ironclaw"
	# Fallback: Docker on amd64 host writes to target/release/
	[ -f "$BINARY_PATH" ] || BINARY_PATH="$REPO_DIR/target/release/ironclaw"
fi

[ -f "$BINARY_PATH" ] || die "Build failed: binary not found at $BINARY_PATH"
BINARY_SIZE=$(du -h "$BINARY_PATH" | cut -f1)
log "Binary built: $BINARY_SIZE"

# ── Step 5: Deploy ───────────────────────────────────────────────────

log "Step 5/5: Deploying to $DROPLET_IP"

log "Uploading binary..."
scp_cmd "$BINARY_PATH" "$SSH_USER@$DROPLET_IP:/tmp/ironclaw-bin"

# Build the .env content
ENV_CONTENT=$(cat <<ENVEOF
# IronClaw — generated by deploy.sh on $(date -u +%Y-%m-%dT%H:%M:%SZ)

# Database (embedded, zero-dependency)
DATABASE_BACKEND=libsql
LIBSQL_PATH=/opt/ironclaw/data/ironclaw.db

# LLM
LLM_BACKEND=$LLM_BACKEND
$LLM_ENV_LINES

# Agent
AGENT_NAME=ironclaw

# Web gateway (Caddy reverse-proxies to this)
CLI_ENABLED=false
GATEWAY_ENABLED=true
GATEWAY_HOST=127.0.0.1
GATEWAY_PORT=3000
GATEWAY_AUTH_TOKEN=$GATEWAY_AUTH_TOKEN
GATEWAY_USER_ID=default

# Features
SKILLS_ENABLED=true
ROUTINES_ENABLED=true

# Logging
RUST_LOG=ironclaw=info
ENVEOF
)

# Build the install script that runs on the server
INSTALL_SCRIPT=$(cat <<'INSTALLEOF'
#!/usr/bin/env bash
set -euo pipefail

log() { printf '\033[1;32m>>>\033[0m %s\n' "$*"; }
die() { printf '\033[1;31mERROR:\033[0m %s\n' "$*" >&2; exit 1; }

[ "$(id -u)" -eq 0 ] || die "Must run as root"

INSTALL_DIR="/opt/ironclaw"

# ── System packages ─────────────────────────────────────────────────

log "Installing system packages..."
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq \
    ca-certificates libssl3 curl ufw \
    debian-keyring debian-archive-keyring apt-transport-https >/dev/null

# ── Caddy ────────────────────────────────────────────────────────────

if ! command -v caddy &>/dev/null; then
    log "Installing Caddy..."
    curl -1sLf 'https://dl.cloudsmith.io/public/caddy/stable/gpg.key' |
        gpg --dearmor -o /usr/share/keyrings/caddy-stable-archive-keyring.gpg
    curl -1sLf 'https://dl.cloudsmith.io/public/caddy/stable/debian.deb.txt' |
        tee /etc/apt/sources.list.d/caddy-stable.list >/dev/null
    apt-get update -qq
    apt-get install -y -qq caddy >/dev/null
fi

# ── Firewall ─────────────────────────────────────────────────────────

log "Configuring firewall..."
ufw --force reset >/dev/null 2>&1
ufw default deny incoming >/dev/null
ufw default allow outgoing >/dev/null
ufw allow 22/tcp >/dev/null
ufw allow 80/tcp >/dev/null
ufw allow 443/tcp >/dev/null
ufw --force enable >/dev/null

# ── User & directories ──────────────────────────────────────────────

if ! id ironclaw &>/dev/null; then
    log "Creating ironclaw system user..."
    useradd --system --create-home --home-dir "$INSTALL_DIR" \
        --shell /usr/sbin/nologin ironclaw
fi

mkdir -p "$INSTALL_DIR"/{bin,data,skills,channels}

# ── Install binary ──────────────────────────────────────────────────

log "Installing binary..."
install -m 755 -o ironclaw -g ironclaw /tmp/ironclaw-bin "$INSTALL_DIR/bin/ironclaw"
rm -f /tmp/ironclaw-bin

# ── Write .env ──────────────────────────────────────────────────────

log "Writing configuration..."
install -m 600 -o ironclaw -g ironclaw /dev/null "$INSTALL_DIR/.env"
cat /tmp/ironclaw-env > "$INSTALL_DIR/.env"
chown ironclaw:ironclaw "$INSTALL_DIR/.env"
chmod 600 "$INSTALL_DIR/.env"
rm -f /tmp/ironclaw-env

# ── systemd ─────────────────────────────────────────────────────────

log "Setting up systemd service..."
cat >/etc/systemd/system/ironclaw.service <<UNIT
[Unit]
Description=IronClaw AI Assistant
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=ironclaw
Group=ironclaw
WorkingDirectory=/opt/ironclaw
EnvironmentFile=/opt/ironclaw/.env
ExecStart=/opt/ironclaw/bin/ironclaw
Restart=on-failure
RestartSec=5
LimitNOFILE=65536

[Install]
WantedBy=multi-user.target
UNIT

systemctl daemon-reload
systemctl enable ironclaw >/dev/null 2>&1

# ── Caddy config ────────────────────────────────────────────────────

DOMAIN_FILE="/tmp/ironclaw-domain"
if [ -s "$DOMAIN_FILE" ]; then
    DOMAIN=$(cat "$DOMAIN_FILE")
    log "Configuring Caddy for $DOMAIN..."
    cat >/etc/caddy/Caddyfile <<CADDY
$DOMAIN {
    reverse_proxy localhost:3000
}
CADDY
    rm -f "$DOMAIN_FILE"
else
    log "No domain — Caddy will serve on port 80 (HTTP)..."
    cat >/etc/caddy/Caddyfile <<CADDY
:80 {
    reverse_proxy localhost:3000
}
CADDY
fi

# ── Start ────────────────────────────────────────────────────────────

log "Starting services..."
systemctl restart caddy
systemctl restart ironclaw

sleep 2
log "Install complete."
INSTALLEOF
)

# Upload config and install script
log "Uploading configuration..."
ssh_cmd "install -m 600 /dev/null /tmp/ironclaw-env"
echo "$ENV_CONTENT" | ssh_cmd "cat > /tmp/ironclaw-env"

if [ -n "$DOMAIN" ]; then
	echo "$DOMAIN" | ssh_cmd "cat > /tmp/ironclaw-domain"
else
	ssh_cmd "rm -f /tmp/ironclaw-domain"
fi

log "Uploading install script..."
echo "$INSTALL_SCRIPT" | ssh_cmd "cat > /tmp/ironclaw-install.sh && chmod +x /tmp/ironclaw-install.sh"

log "Running install on $DROPLET_IP (this takes ~30 seconds)..."
ssh_cmd -t "bash /tmp/ironclaw-install.sh && rm -f /tmp/ironclaw-install.sh"

# ── Summary ──────────────────────────────────────────────────────────

if [ -n "$DOMAIN" ]; then
	ACCESS_URL="https://$DOMAIN"
else
	ACCESS_URL="http://$DROPLET_IP"
fi

echo
printf '%s' "$BLD"
cat <<'DONE'
  ╔══════════════════════════════════════╗
  ║   IronClaw deployed successfully!   ║
  ╚══════════════════════════════════════╝
DONE
printf '%s' "$RST"
echo
echo "  ${BLD}URL:${RST}        $ACCESS_URL"
echo "  ${BLD}Auth token:${RST} $GATEWAY_AUTH_TOKEN"
echo "  ${BLD}Droplet IP:${RST} $DROPLET_IP"
echo "  ${BLD}LLM:${RST}        $LLM_BACKEND"
echo
echo "  ${BLD}Useful commands:${RST}"
echo "    ssh -i $SSH_KEY $SSH_USER@$DROPLET_IP"
echo "    ssh -i $SSH_KEY $SSH_USER@$DROPLET_IP journalctl -fu ironclaw"
echo "    ssh -i $SSH_KEY $SSH_USER@$DROPLET_IP systemctl restart ironclaw"
echo
echo "  ${BLD}Verify:${RST}"
echo "    curl -H 'Authorization: Bearer $GATEWAY_AUTH_TOKEN' $ACCESS_URL/api/health"
echo
echo "  ${BLD}Update later:${RST}"
echo "    DROPLET_IP=$DROPLET_IP SSH_KEY=$SSH_KEY ./scripts/update.sh"
echo
