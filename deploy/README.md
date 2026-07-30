# Anima — Deployment & Release Infrastructure

## Overview

The CI/CD pipeline automatically builds and releases Anima on every push to `master`.

```
Push to master → GitHub Actions builds all targets → GitHub Release created → Server auto-deployed
```

## Release Pipeline (`release.yml`)

**Trigger:** Push to `master` branch or any `v*` tag.

### Build Matrix

| Target | Runner | Artifact | Notes |
|---|---|---|---|
| macOS arm64 client | `macos-latest` | `anima-macos-arm64.zip` | Native Apple Silicon build |
| Linux x64 client | `ubuntu-latest` | `anima-linux-x64.zip` | Native build + Wayland/Vulkan deps |
| Windows x64 client | `ubuntu-latest` | `anima-windows-x64.zip` | Cross-compiled via mingw-w64 |
| Linux x64 server | `ubuntu-latest` | `anima-linux-server.zip` | Headless server binary |

### Release Tagging

- **Tag push (`v*`):** Uses the tag as-is (e.g., `v0.3.0`)
- **Master push:** Auto-generates tag from `Cargo.toml` version + short SHA (e.g., `v0.3.0-a1b2c3d`)
- All releases are marked as `latest` so the website download URLs always resolve

### Artifact Structure

Client zips contain:
```
anima/
  anima          (or anima.exe on Windows)
  assets/        (game assets: models, fonts, images, audio)
```

Server zip contains:
```
anima-server     (single binary, no assets needed)
```

## CI Checks (`ci.yml`)

Runs on every PR to `master`:
- `cargo check --all-targets`
- `cargo clippy -- -D warnings`
- Release builds of both client and server

## Server Deployment

### Architecture

```
GitHub Actions (deploy-server job — depends only on the server build,
so server hotfixes ship without waiting for client builds)
    ↓ SCP binary
    ↓ SSH run deploy script (health-gated: service active + UDP :5000 bound)
AWS Lightsail: anima-game-1 (us-west-2, static IP 44.235.228.56)
    /opt/anima/anima-server (systemd-managed, crash-alerting via
    ExecStopPost log hook + OnFailure unit → /opt/anima/alerts.log)
```

### Initial Server Setup

Run the setup script on the Salt Lake City box:

```bash
ssh user@slc-server 'bash -s' < deploy/setup-server.sh
```

This creates:
- `anima` system user (no login shell)
- `/opt/anima/` install directory
- `anima-server` systemd service (auto-restart, security-hardened)
- Opens UDP port 5000 in the firewall

### GitHub Secrets Required

Set these in the `KaneLabs/fps` repo settings → Secrets → Actions:

| Secret | Value |
|---|---|
| `DEPLOY_HOST` | IP address or hostname of the Salt Lake City server |
| `DEPLOY_USER` | SSH username on the server (e.g., `deploy` or `ryan`) |
| `DEPLOY_SSH_KEY` | Private key contents (Ed25519 recommended) |
| `DEPLOY_SSH_PORT` | SSH port (default: 22) |

Create a deploy environment named `production` in the repo settings for deployment protection rules.

### Generate Deploy Key

```bash
# On your local machine
ssh-keygen -t ed25519 -C "anima-deploy-github-actions" -f ~/.ssh/anima-deploy

# Add public key to the server
ssh user@slc-server 'cat >> ~/.ssh/authorized_keys' < ~/.ssh/anima-deploy.pub

# Add private key as DEPLOY_SSH_KEY secret in GitHub
cat ~/.ssh/anima-deploy
```

### Manual Server Operations

```bash
# Check server status
ssh user@slc-server 'sudo systemctl status anima-server'

# View live logs
ssh user@slc-server 'journalctl -u anima-server -f'

# Restart server
ssh user@slc-server 'sudo systemctl restart anima-server'

# View last 50 log lines
ssh user@slc-server 'journalctl -u anima-server -n 50 --no-pager'
```

## Deploy Guards

The `deploy-server` job is serialized (`concurrency: production-deploy`) and the
on-box deploy script enforces two guards **before** touching the service:

1. **Playtest gate** — if `/opt/anima/playtest-live` exists and is less than 3h
   old (file mtime), the deploy aborts loudly. Source of truth is the shared
   note `543fbc05/playtest-live` (CEO sets it; ops mirrors it to the box).
   - Set:   `sudo touch /opt/anima/playtest-live` (re-touch to renew the 3h window)
   - Clear: `sudo rm -f /opt/anima/playtest-live`
   - A stale flag (>3h) is ignored and auto-cleared, so a forgotten flag can
     never brick deploys overnight.
2. **Epoch guard** — the deploy refuses to install a commit whose timestamp is
   older than the running build (`/opt/anima/DEPLOY_EPOCH`). This stops racing
   release runs from landing out of order.
   - **Known false positive:** the guard compares commit *timestamps*, so a
     rebased or cherry-picked hotfix can carry an older commit date than the
     running build and be refused even though it is genuinely newer. That is
     exactly what the force path below is for — don't debug the guard at 3am,
     just force the deploy (CEO sign-off) and move on.

**Emergency rollback (intentional deploy of an older commit):** set the repo
variable `DEPLOY_FORCE=1` (Settings → Secrets and variables → Variables) and
re-run the deploy job — or `sudo touch /opt/anima/FORCE_DEPLOY` on the box
(one-shot, consumed by the next deploy). Force bypasses **both** guards — CEO
sign-off required. Remember to unset `DEPLOY_FORCE` afterwards.

## Website Integration

The playanima.com download buttons point to:
- **macOS:** `https://github.com/KaneLabs/fps/releases/latest/download/anima-macos-arm64.zip`
- **Windows:** `https://github.com/KaneLabs/fps/releases/latest/download/anima-windows-x64.zip`

These auto-resolve to the latest release. No website changes needed when new versions are released.

## Triggering a Release

### Automatic (recommended)
Just merge a PR to `master`. The pipeline handles everything.

### Manual version bump
```bash
# Edit version in Cargo.toml, then:
git tag v0.4.0
git push origin master --tags
```
