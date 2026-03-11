#!/usr/bin/env bash
# IALY Game Server Provisioning Script
# Run on a fresh Ubuntu/Debian VPS as root:
#   ssh root@<server> 'bash -s' < provision.sh
set -euo pipefail

SERVER_USER="ialy"
INSTALL_DIR="/opt/ialy"
GAME_PORT=5000

echo "=== IALY Server Provisioning ==="

# 1. System updates
echo "[1/7] Updating system packages..."
apt-get update -qq && apt-get upgrade -y -qq

# 2. Create service account (no login shell, no home dir needed)
echo "[2/7] Creating service account: $SERVER_USER"
if id "$SERVER_USER" &>/dev/null; then
    echo "  User $SERVER_USER already exists, skipping"
else
    useradd --system --no-create-home --shell /usr/sbin/nologin "$SERVER_USER"
fi

# 3. Create install directory
echo "[3/7] Setting up $INSTALL_DIR"
mkdir -p "$INSTALL_DIR"
chown "$SERVER_USER":"$SERVER_USER" "$INSTALL_DIR"

# 4. Install systemd unit
echo "[4/7] Installing systemd service: ialy.service"
cat > /etc/systemd/system/ialy.service << 'UNIT'
[Unit]
Description=IALY Game Server
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=ialy
Group=ialy
WorkingDirectory=/opt/ialy
ExecStart=/opt/ialy/server
Restart=always
RestartSec=5

# Resource limits
LimitNOFILE=65535
MemoryMax=4G
Nice=-5

# Security hardening
ProtectSystem=strict
ReadWritePaths=/opt/ialy
ProtectHome=yes
NoNewPrivileges=yes
PrivateTmp=yes

# Logging
StandardOutput=journal
StandardError=journal
SyslogIdentifier=ialy

[Install]
WantedBy=multi-user.target
UNIT

systemctl daemon-reload
systemctl enable ialy.service

# 5. Firewall (UFW)
echo "[5/7] Configuring firewall..."
apt-get install -y -qq ufw
ufw --force reset
ufw default deny incoming
ufw default allow outgoing
ufw allow 22/tcp comment "SSH"
ufw allow ${GAME_PORT}/udp comment "IALY game server"
ufw --force enable
ufw status verbose

# 6. Harden SSH — disable password auth
echo "[6/7] Hardening SSH (key-only auth)..."
sed -i 's/^#\?PasswordAuthentication.*/PasswordAuthentication no/' /etc/ssh/sshd_config
sed -i 's/^#\?PermitRootLogin.*/PermitRootLogin prohibit-password/' /etc/ssh/sshd_config
sed -i 's/^#\?ChallengeResponseAuthentication.*/ChallengeResponseAuthentication no/' /etc/ssh/sshd_config
systemctl restart sshd

# 7. Allow ialy user to restart its own service (for CI deploy)
echo "[7/7] Setting up deploy permissions..."
cat > /etc/sudoers.d/ialy-deploy << 'SUDOERS'
# Allow root to restart ialy service (used by CI deploy via SSH as root)
# No additional sudoers needed — CI SSHes in as root
SUDOERS
chmod 440 /etc/sudoers.d/ialy-deploy

echo ""
echo "=== Provisioning complete ==="
echo ""
echo "Server directory:  $INSTALL_DIR"
echo "Service name:      ialy.service"
echo "Game port:         ${GAME_PORT}/udp"
echo "Service user:      $SERVER_USER"
echo ""
echo "Next steps:"
echo "  1. Upload the server binary to $INSTALL_DIR/server"
echo "  2. chmod +x $INSTALL_DIR/server"
echo "  3. systemctl start ialy"
echo "  4. journalctl -u ialy -f   (watch logs)"
echo ""
echo "To deploy updates:"
echo "  scp server root@<this-host>:$INSTALL_DIR/server.new"
echo "  ssh root@<this-host> 'mv $INSTALL_DIR/server.new $INSTALL_DIR/server && chmod +x $INSTALL_DIR/server && systemctl restart ialy'"
