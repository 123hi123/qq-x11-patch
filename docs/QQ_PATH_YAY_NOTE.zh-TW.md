# QQ 路徑與安裝方式說明（README 以外補充）

本專案預設的 QQ 可執行檔是：

`/opt/QQ/qq`

`systemd` 服務檔中的參數也使用這個路徑：

`--restart-cmd /opt/QQ/qq`

---

## 我們的安裝前提

這個專案是以 **`yay` 安裝 `linuxqq`** 的環境驗證與調整。  
在這種安裝方式下，QQ 主程式通常位於 `/opt/QQ/qq`。

安裝範例：

```bash
yay -S linuxqq
```

---

## 如何確認你自己的 QQ 啟動路徑

### 1) 先看系統是否有 `qq` 指令

```bash
command -v qq
```

若有輸出路徑，就可先用那個值。

### 2) 確認 `/opt/QQ/qq` 是否存在、由誰提供

```bash
ls -l /opt/QQ/qq
pacman -Qo /opt/QQ/qq
```

若看到類似 `owned by linuxqq`，代表可直接使用 `/opt/QQ/qq`。

### 3) 查套件安裝了哪些檔案（以 `linuxqq` 為例）

```bash
pacman -Ql linuxqq | rg -i '/qq$|/QQ/qq$'
```

---

## 套用到服務檔

編輯：

`~/.config/systemd/user/qq-x11-guard-rs.service`

確認 `ExecStart` 內有：

```ini
--restart-cmd /opt/QQ/qq
```

修改後執行：

```bash
systemctl --user daemon-reload
systemctl --user restart qq-x11-guard-rs.service
```

---

## 非 yay 安裝的情況

如果你不是用 `yay`/`linuxqq` 安裝，路徑與啟動方式可能不同。  
這類情況請直接詢問你常用的 AI 助手，並附上以下資訊再請它幫你改：

- 你的發行版與桌面環境
- `which qq` / `command -v qq` 輸出
- 套件管理器安裝來源（例如 flatpak、snap、手動安裝）
- 目前的 `qq-x11-guard-rs.service` 內容
