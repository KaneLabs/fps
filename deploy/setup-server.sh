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
# Log every abnormal exit, even ones auto-restart recovers from
ExecStopPost=/usr/local/bin/anima-crash-log.sh

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

echo "==> Installing crash alerting hooks..."
sudo tee /usr/local/bin/anima-crash-log.sh > /dev/null << 'SCRIPT'
#!/usr/bin/env bash
# Invoked by systemd ExecStopPost with SERVICE_RESULT / EXIT_CODE / EXIT_STATUS set.
if [ "${SERVICE_RESULT:-success}" != "success" ]; then
  echo "$(date -u +%FT%TZ) anima-server abnormal exit: result=${SERVICE_RESULT} code=${EXIT_CODE:-?} status=${EXIT_STATUS:-?}" >> /opt/anima/alerts.log
  logger -t anima-alert -p daemon.err "anima-server crashed: result=${SERVICE_RESULT} code=${EXIT_CODE:-?} status=${EXIT_STATUS:-?}"
fi
SCRIPT
sudo chmod 755 /usr/local/bin/anima-crash-log.sh

sudo tee /etc/systemd/system/anima-alert@.service > /dev/null << 'UNIT'
[Unit]
Description=Anima failure alert for %i

[Service]
Type=oneshot
ExecStart=/bin/sh -c 'echo "$(date -u +%%FT%%TZ) ANIMA ALERT: %i entered FAILED state (crash loop, start limit hit)" >> /opt/anima/alerts.log; logger -t anima-alert -p daemon.crit "ANIMA ALERT: %i FAILED — crash loop"'
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
