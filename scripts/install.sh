#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
SERVICE_SRC="$PROJECT_DIR/systemd/qq-x11-guard-rs.service"
BIN_DST="$HOME/.local/bin/qq-x11-guard-rs"
SERVICE_DST="$HOME/.config/systemd/user/qq-x11-guard-rs.service"

if [[ ! -d "$PROJECT_DIR" ]]; then
  echo "找不到專案目錄：$PROJECT_DIR"
  exit 1
fi

if [[ ! -f "$SERVICE_SRC" ]]; then
  echo "找不到服務檔：$SERVICE_SRC"
  exit 1
fi

mkdir -p "$HOME/.local/bin" "$HOME/.config/systemd/user"

(
  cd "$PROJECT_DIR"
  cargo build --release
)

install -m 755 "$PROJECT_DIR/target/release/qq-x11-guard-rs" "$BIN_DST"
install -m 644 "$SERVICE_SRC" "$SERVICE_DST"

systemctl --user daemon-reload
systemctl --user enable --now qq-x11-guard-rs.service
systemctl --user status --no-pager qq-x11-guard-rs.service || true

echo
echo "已啟用 qq-x11-guard-rs.service"
echo "查看日誌：journalctl --user -u qq-x11-guard-rs.service -f"
