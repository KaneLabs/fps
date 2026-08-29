#!/usr/bin/env bash
# ==============================================================================
# Anima Game Server — Initial Setup Script
# Run this ONCE on the Salt Lake City bare metal box to configure the server.
#
# Usage: ssh user@host 'bash -s' < deploy/setup-server.sh
# ==============================================================================
set -euo pipefail

INSTALL_DIR="/opt/anima"
SERVICE_NAME="anima-server"
GAME_USER="anima"
GAME_PORT=5000

echo "==> Installing dependencies..."
sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -qq unzip

echo "==> Creating game user..."
if ! id "${GAME_USER}" &>/dev/null; then
  sudo useradd --system --no-create-home --shell /usr/sbin/nologin "${GAME_USER}"
  echo "    Created user: ${GAME_USER}"
else
  echo "    User ${GAME_USER} already exists"
fi

echo "==> Creating install directory..."
sudo mkdir -p "${INSTALL_DIR}"
sudo chown "${GAME_USER}:${GAME_USER}" "${INSTALL_DIR}"

echo "==> Opening firewall port ${GAME_PORT}/udp..."
if command -v ufw &>/dev/null; then
  sudo ufw allow "${GAME_PORT}/udp" comment "Anima game server"
elif command -v firewall-cmd &>/dev/null; then
  sudo firewall-cmd --permanent --add-port="${GAME_PORT}/udp"
  sudo firewall-cmd --reload
else
  echo "    No firewall manager found — manually open UDP port ${GAME_PORT}"
fi

echo "==> Creating systemd service..."
sudo tee /etc/systemd/system/${SERVICE_NAME}.service > /dev/null << EOF
[Unit]
Description=Anima Game Server
After=network-online.target
Wants=network-online.target
# Crash alerting: default start-limit (10s/5) with RestartSec=5 means a crash
# loop NEVER reaches failed state. 5 failures in 5 min -> failed -> OnFailure.
StartLimitIntervalSec=300
StartLimitBurst=5
OnFailure=anima-alert@%N.service

[Service]
Type=simple
User=${GAME_USER}
Group=${GAME_USER}
WorkingDirectory=${INSTALL_DIR}
ExecStart=${INSTALL_DIR}/anima-server
Restart=always
RestartSec=5
# Log every abnormal exit, even ones auto-restart recovers from.
# The '+' prefix runs this as root rather than as ${GAME_USER}: it must read
# the root-only webhook config. Keeping the webhook unreadable by the game
# server matters because a compromised server could otherwise flood the
# alert channel and drown a real alert.
ExecStopPost=+/usr/local/bin/anima-crash-log.sh

# Resource limits
LimitNOFILE=65535
LimitNPROC=4096

# Security hardening
ProtectSystem=strict
ProtectHome=true
ReadWritePaths=${INSTALL_DIR}
PrivateTmp=true
NoNewPrivileges=true

# Logging
StandardOutput=journal
StandardError=journal
SyslogIdentifier=${SERVICE_NAME}

[Install]
WantedBy=multi-user.target
EOF

echo "==> Installing alert delivery (webhook push)..."
# Delivery layer. Reads the webhook URL from config on the box rather than
# baking it in, so alerting can ship before anyone has supplied a URL, and
# the URL is never in git or in CI.
#
# WHY THIS EXISTS: crash detection without delivery is the least useful
# place to be — it looks like coverage on a diagram while time-to-detection
# is actually undefined (it equals "until a human opens a file").
sudo mkdir -p /etc/anima
sudo tee /usr/local/bin/anima-alert-notify.sh > /dev/null << 'SCRIPT'
#!/usr/bin/env bash
# anima-alert-notify.sh <severity> <message>
# Pushes an alert to the configured webhook. Silently no-ops when no webhook
# is configured, so this is safe to install before a URL exists.
#
# Invariants:
#   - NEVER prints the webhook URL (it would land in journald).
#   - NEVER fails its caller: always exits 0, so a webhook outage cannot
#     break a systemd stop/failure path and mask the very crash it reports.
#   - Bounded runtime: curl is capped, so a hung endpoint cannot wedge
#     service shutdown.
set -uo pipefail

CONF=/etc/anima/alert-webhook.conf
[ -r "$CONF" ] || exit 0
# shellcheck source=/dev/null
. "$CONF" 2>/dev/null || exit 0
URL="${ANIMA_ALERT_WEBHOOK:-}"
[ -n "$URL" ] || exit 0

SEVERITY="${1:-info}"
MESSAGE="${2:-(no message)}"
TEXT="[${SEVERITY}] anima $(hostname -s) $(date -u +%FT%TZ) — ${MESSAGE}"

# JSON-escape without depending on python/jq being present.
esc="${TEXT//\\/\\\\}"
esc="${esc//\"/\\\"}"
esc="${esc//$'\n'/ }"
J="\"${esc}\""

case "$URL" in
  *hooks.slack.com*)                 BODY="{\"text\":${J}}" ;;
  *discord.com/api/webhooks*|*discordapp.com/api/webhooks*)
                                     BODY="{\"content\":${J}}" ;;
  *)                                 BODY="{\"text\":${J},\"content\":${J}}" ;;
esac

curl -sS -m 10 -X POST -H 'Content-Type: application/json' \
     -d "$BODY" "$URL" >/dev/null 2>&1 || true
exit 0
SCRIPT
sudo chmod 700 /usr/local/bin/anima-alert-notify.sh
sudo chown root:root /usr/local/bin/anima-alert-notify.sh

# Webhook config: root-only. The game server runs as ${GAME_USER}; if it
# could read this, a compromised server could flood the alert channel and
# drown a real alert — which matters now that alerting is a security
# control rather than convenience.
if [ ! -f /etc/anima/alert-webhook.conf ]; then
  sudo tee /etc/anima/alert-webhook.conf > /dev/null << 'CONF'
# Alert delivery webhook. Uncomment and set to enable push alerting.
# Slack:   https://hooks.slack.com/services/...
# Discord: https://discord.com/api/webhooks/...
#ANIMA_ALERT_WEBHOOK="https://..."
CONF
fi
sudo chmod 600 /etc/anima/alert-webhook.conf
sudo chown root:root /etc/anima/alert-webhook.conf

echo "==> Installing crash alerting hooks..."
sudo tee /usr/local/bin/anima-crash-log.sh > /dev/null << 'SCRIPT'
#!/usr/bin/env bash
# Invoked by systemd ExecStopPost with SERVICE_RESULT / EXIT_CODE / EXIT_STATUS set.
if [ "${SERVICE_RESULT:-success}" != "success" ]; then
  MSG="anima-server abnormal exit: result=${SERVICE_RESULT} code=${EXIT_CODE:-?} status=${EXIT_STATUS:-?}"
  echo "$(date -u +%FT%TZ) ${MSG}" >> /opt/anima/alerts.log
  logger -t anima-alert -p daemon.err "${MSG}"
  /usr/local/bin/anima-alert-notify.sh warning "${MSG}" || true
fi
SCRIPT
sudo chmod 755 /usr/local/bin/anima-crash-log.sh

sudo tee /etc/systemd/system/anima-alert@.service > /dev/null << 'UNIT'
[Unit]
Description=Anima failure alert for %i

[Service]
Type=oneshot
ExecStart=/bin/sh -c 'MSG="ANIMA ALERT: %i entered FAILED state (crash loop, start limit hit)"; echo "$(date -u +%%FT%%TZ) $MSG" >> /opt/anima/alerts.log; logger -t anima-alert -p daemon.crit "$MSG"; /usr/local/bin/anima-alert-notify.sh critical "$MSG" || true'
UNIT

sudo touch "${INSTALL_DIR}/alerts.log"
sudo chown "${GAME_USER}:${GAME_USER}" "${INSTALL_DIR}/alerts.log"

echo "==> Enabling service..."
sudo systemctl daemon-reload
sudo systemctl enable "${SERVICE_NAME}"

echo "==> Creating deploy user SSH directory..."
mkdir -p ~/.ssh
chmod 700 ~/.ssh
touch ~/.ssh/authorized_keys
chmod 600 ~/.ssh/authorized_keys

echo ""
echo "============================================"
echo "  Server setup complete!"
echo "============================================"
echo ""
echo "Next steps:"
echo "  1. Copy the server binary to ${INSTALL_DIR}/anima-server"
echo "  2. sudo systemctl start ${SERVICE_NAME}"
echo "  3. Check logs: journalctl -u ${SERVICE_NAME} -f"
echo ""
echo "GitHub Actions needs these secrets:"
echo "  DEPLOY_HOST     = <server IP or hostname>"
echo "  DEPLOY_USER     = $(whoami)"
echo "  DEPLOY_SSH_KEY  = <contents of a new SSH private key>"
echo "  DEPLOY_SSH_PORT = 22 (or custom port)"
echo ""
echo "Add the matching public key to ~/.ssh/authorized_keys on this box."
