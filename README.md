# qq-x11-patch

一個用 Rust 寫的 `QQ` X11 連線守護程式。  
當 `QQ` 對 X11 的連線數超過門檻（預設 `10`）時，會自動：

1. 嘗試優雅關閉 `QQ`（`SIGTERM`）
2. 若逾時仍存在則強制關閉（`SIGKILL`）
3. 重新啟動 `QQ`

並透過 `systemd --user` 讓守護程式在登入後自動啟動、異常自動拉起。

---

## 原理

這個專案是「**Rust 事件判斷** + **systemd 服務管理**」：

- Rust 程式監看 `/proc/<qq_pid>/fd`（`inotify` 事件）
- 每次事件觸發時，重新計算 `QQ` 實際佔用的 X11 連線數
- 若超過門檻，執行重啟流程
- `systemd --user` 負責自啟動與程序存活

> 為避免漏事件，程式另外有低頻備援輪詢（預設 15 秒）。

---

## 倉庫結構

- `src/main.rs`：Rust 守護程式主體
- `systemd/qq-x11-guard-rs.service`：`systemd --user` 服務檔
- `scripts/install.sh`：安裝腳本（編譯、安裝 binary、啟用服務）
- `Cargo.toml`：Rust 專案設定

---

## 安裝與啟用

### 1) 需求

- Linux（X11 session）
- `rustup` / `cargo`
- `systemd --user`
- `ss`（通常由 `iproute2` 提供）

### 2) 安裝

在 repo 根目錄執行：

```bash
bash scripts/install.sh
```

腳本會做這些事：

1. `cargo build --release`
2. 安裝 binary 到 `~/.local/bin/qq-x11-guard-rs`
3. 安裝服務到 `~/.config/systemd/user/qq-x11-guard-rs.service`
4. `systemctl --user enable --now qq-x11-guard-rs.service`

### 3) 驗證

```bash
systemctl --user status qq-x11-guard-rs.service
journalctl --user -u qq-x11-guard-rs.service -f
```

---

## 開機/登入後的行為

1. 你登入桌面後，`systemd --user` 啟動 `qq-x11-guard-rs.service`
2. 服務執行 `~/.local/bin/qq-x11-guard-rs`
3. 程式持續監控 `QQ` X11 連線數
4. 超過門檻（預設 10）就重啟 `QQ`
5. 若守護程式退出，`Restart=always` 會自動重啟守護程式

---

## 調整門檻與參數

編輯服務檔中的 `ExecStart`：

```ini
ExecStart=%h/.local/bin/qq-x11-guard-rs --app-name qq --threshold 10 --restart-cmd qq --cooldown 120 --fallback-poll 15 --scan-interval 2
```

修改後套用：

```bash
systemctl --user daemon-reload
systemctl --user restart qq-x11-guard-rs.service
```

---

## 參數說明

- `--threshold`：X11 連線門檻（預設 `10`）
- `--cooldown`：重啟冷卻秒數（預設 `120`）
- `--fallback-poll`：備援輪詢秒數（預設 `15`）
- `--scan-interval`：PID 同步秒數（預設 `2`）
- `--dry-run`：只記錄動作，不真的重啟

---

## 常見問題

### Q: 這是「開機自啟」還是「登入自啟」？

目前是 `systemd --user`，屬於**登入自啟**。  
若你要在未登入圖形桌面前就啟動，需額外配置 `linger` 與顯示環境，通常不建議對這類 GUI 監控這麼做。

### Q: 為什麼不用純 systemd path？

`systemd path` 擅長檔案變化觸發，但不擅長直接判斷「某程序 X11 連線數 > N」這種條件。  
所以把條件判斷放在 Rust，讓 `systemd` 專心做服務生命週期管理。
