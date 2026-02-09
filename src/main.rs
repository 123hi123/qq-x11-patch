use std::collections::{HashMap, HashSet};
use std::env;
use std::ffi::CString;
use std::fs;
use std::io;
use std::mem;
use std::os::fd::RawFd;
use std::path::Path;
use std::process::{Command, Stdio};
use std::ptr;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const WATCH_MASK: u32 = libc::IN_CREATE
    | libc::IN_DELETE
    | libc::IN_ATTRIB
    | libc::IN_MOVED_FROM
    | libc::IN_MOVED_TO
    | libc::IN_MOVE_SELF
    | libc::IN_DELETE_SELF;
const EVENT_BUF_SIZE: usize = 8192;

// ===== 區塊 1：設定與參數 =====
#[derive(Debug, Clone)]
struct Config {
    app_name: String,
    threshold: usize,
    display: String,
    restart_cmd: String,
    cooldown_seconds: u64,
    fallback_poll_seconds: u64,
    scan_interval_seconds: u64,
    dry_run: bool,
    log_prefix: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            app_name: "qq".to_string(),
            threshold: 10,
            display: env::var("DISPLAY").unwrap_or_else(|_| ":0".to_string()),
            restart_cmd: "qq".to_string(),
            cooldown_seconds: 120,
            fallback_poll_seconds: 15,
            scan_interval_seconds: 2,
            dry_run: false,
            log_prefix: "[qq-x11-guard-rs]".to_string(),
        }
    }
}

fn parse_args() -> Result<Config, String> {
    let mut config = Config::default();
    let args: Vec<String> = env::args().collect();
    let mut index = 1;

    while index < args.len() {
        let key = args[index].as_str();
        match key {
            "--app-name" => {
                index += 1;
                config.app_name = args.get(index).ok_or("--app-name 需要值")?.clone();
            }
            "--threshold" => {
                index += 1;
                let value = args.get(index).ok_or("--threshold 需要值")?;
                config.threshold = value
                    .parse::<usize>()
                    .map_err(|_| "--threshold 必須是正整數".to_string())?;
                if config.threshold == 0 {
                    return Err("--threshold 必須 >= 1".to_string());
                }
            }
            "--display" => {
                index += 1;
                config.display = args.get(index).ok_or("--display 需要值")?.clone();
            }
            "--restart-cmd" => {
                index += 1;
                config.restart_cmd = args.get(index).ok_or("--restart-cmd 需要值")?.clone();
            }
            "--cooldown" => {
                index += 1;
                let value = args.get(index).ok_or("--cooldown 需要值")?;
                config.cooldown_seconds = value
                    .parse::<u64>()
                    .map_err(|_| "--cooldown 必須是整數".to_string())?;
            }
            "--fallback-poll" => {
                index += 1;
                let value = args.get(index).ok_or("--fallback-poll 需要值")?;
                config.fallback_poll_seconds = value
                    .parse::<u64>()
                    .map_err(|_| "--fallback-poll 必須是正整數".to_string())?;
                if config.fallback_poll_seconds == 0 {
                    return Err("--fallback-poll 必須 >= 1".to_string());
                }
            }
            "--scan-interval" => {
                index += 1;
                let value = args.get(index).ok_or("--scan-interval 需要值")?;
                config.scan_interval_seconds = value
                    .parse::<u64>()
                    .map_err(|_| "--scan-interval 必須是正整數".to_string())?;
                if config.scan_interval_seconds == 0 {
                    return Err("--scan-interval 必須 >= 1".to_string());
                }
            }
            "--dry-run" => {
                config.dry_run = true;
            }
            "--help" | "-h" => {
                print_help(&args[0]);
                std::process::exit(0);
            }
            _ => {
                return Err(format!("不支援的參數: {key}"));
            }
        }
        index += 1;
    }
    Ok(config)
}

fn print_help(program: &str) {
    println!(
        "用法: {program} [選項]\n\
         \n\
         --app-name <name>        監控程序名，預設 qq\n\
         --threshold <n>          X11 連線門檻，預設 10\n\
         --display <display>      X11 DISPLAY，預設 $DISPLAY 或 :0\n\
         --restart-cmd <cmd>      超標後重啟命令，預設 qq\n\
         --cooldown <sec>         重啟冷卻秒數，預設 120\n\
         --fallback-poll <sec>    備援輪詢秒數，預設 15\n\
         --scan-interval <sec>    PID 同步秒數，預設 2\n\
         --dry-run                只輸出行為，不真的重啟\n\
         -h, --help               顯示說明"
    );
}

fn timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn log(config: &Config, message: &str) {
    println!("{} {} {}", timestamp(), config.log_prefix, message);
}

fn display_to_socket(display: &str) -> Result<String, String> {
    if !display.starts_with(':') {
        return Err(format!("無效 DISPLAY: {display}"));
    }
    let display_num = display[1..].split('.').next().unwrap_or("");
    if display_num.is_empty() || !display_num.chars().all(|char| char.is_ascii_digit()) {
        return Err(format!("無效 DISPLAY: {display}"));
    }
    Ok(format!("/tmp/.X11-unix/X{display_num}"))
}

// ===== 區塊 2：程序與 socket 狀態收集 =====
fn find_pids_by_name(process_name: &str) -> Vec<i32> {
    let mut pids = Vec::new();
    let entries = match fs::read_dir("/proc") {
        Ok(value) => value,
        Err(_) => return pids,
    };

    for entry in entries.flatten() {
        let file_name = entry.file_name();
        let pid_text = file_name.to_string_lossy();
        if !pid_text.chars().all(|char| char.is_ascii_digit()) {
            continue;
        }
        let pid = match pid_text.parse::<i32>() {
            Ok(value) => value,
            Err(_) => continue,
        };

        let comm_path = format!("/proc/{pid}/comm");
        let comm = match fs::read_to_string(&comm_path) {
            Ok(value) => value,
            Err(_) => continue,
        };
        if comm.trim() == process_name {
            pids.push(pid);
        }
    }

    pids.sort_unstable();
    pids
}

fn socket_inodes_for_pid(pid: i32) -> HashSet<String> {
    let mut result = HashSet::new();
    let fd_path = format!("/proc/{pid}/fd");
    let entries = match fs::read_dir(fd_path) {
        Ok(value) => value,
        Err(_) => return result,
    };

    for entry in entries.flatten() {
        let link = match fs::read_link(entry.path()) {
            Ok(value) => value,
            Err(_) => continue,
        };
        let link_text = link.to_string_lossy();
        if let Some(inode) = parse_socket_inode(&link_text) {
            result.insert(inode.to_string());
        }
    }
    result
}

fn parse_socket_inode(text: &str) -> Option<&str> {
    if !text.starts_with("socket:[") || !text.ends_with(']') {
        return None;
    }
    Some(&text[8..text.len() - 1])
}

fn peer_inodes_on_x11_socket(socket_path: &str) -> HashSet<String> {
    let mut inodes = HashSet::new();
    let sources = [format!("@{socket_path}"), socket_path.to_string()];

    for source in sources {
        let output = Command::new("ss")
            .args(["-xnpH", "src", source.as_str()])
            .output();
        let output = match output {
            Ok(value) => value,
            Err(_) => continue,
        };
        if !output.status.success() {
            continue;
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        for line in stdout.lines() {
            let tokens: Vec<&str> = line.split_whitespace().collect();
            if let Some(peer) = extract_peer_inode(&tokens, socket_path) {
                inodes.insert(peer.to_string());
            }
        }
    }
    inodes
}

fn extract_peer_inode<'a>(tokens: &'a [&'a str], socket_path: &str) -> Option<&'a str> {
    let with_at = format!("@{socket_path}");
    for (index, token) in tokens.iter().enumerate() {
        if *token != socket_path && *token != with_at {
            continue;
        }
        if index + 3 >= tokens.len() {
            return None;
        }
        if tokens[index + 2] != "*" {
            return None;
        }
        let peer = tokens[index + 3];
        if peer.chars().all(|char| char.is_ascii_digit()) {
            return Some(peer);
        }
    }
    None
}

fn count_app_x11_connections(app_pids: &[i32], socket_path: &str) -> usize {
    if app_pids.is_empty() {
        return 0;
    }
    let x11_peer_inodes = peer_inodes_on_x11_socket(socket_path);
    if x11_peer_inodes.is_empty() {
        return 0;
    }
    let mut app_socket_inodes = HashSet::new();
    for pid in app_pids {
        app_socket_inodes.extend(socket_inodes_for_pid(*pid));
    }
    app_socket_inodes.intersection(&x11_peer_inodes).count()
}

// ===== 區塊 3：事件來源（inotify） =====
struct InotifyWatch {
    fd: RawFd,
    wd_to_pid: HashMap<i32, i32>,
    pid_to_wd: HashMap<i32, i32>,
}

impl InotifyWatch {
    fn new() -> io::Result<Self> {
        let fd = unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            fd,
            wd_to_pid: HashMap::new(),
            pid_to_wd: HashMap::new(),
        })
    }

    fn add_pid(&mut self, pid: i32) {
        if self.pid_to_wd.contains_key(&pid) {
            return;
        }
        let fd_path = format!("/proc/{pid}/fd");
        if !Path::new(&fd_path).is_dir() {
            return;
        }
        let c_path = match CString::new(fd_path) {
            Ok(value) => value,
            Err(_) => return,
        };
        let wd = unsafe { libc::inotify_add_watch(self.fd, c_path.as_ptr(), WATCH_MASK) };
        if wd < 0 {
            return;
        }
        self.wd_to_pid.insert(wd, pid);
        self.pid_to_wd.insert(pid, wd);
    }

    fn remove_pid(&mut self, pid: i32) {
        let wd = match self.pid_to_wd.remove(&pid) {
            Some(value) => value,
            None => return,
        };
        self.wd_to_pid.remove(&wd);
        unsafe {
            libc::inotify_rm_watch(self.fd, wd);
        }
    }

    fn sync_pids(&mut self, current_pids: &[i32]) {
        let current: HashSet<i32> = current_pids.iter().copied().collect();
        let existing: HashSet<i32> = self.pid_to_wd.keys().copied().collect();

        for pid in existing.difference(&current) {
            self.remove_pid(*pid);
        }
        for pid in current.difference(&existing) {
            self.add_pid(*pid);
        }
    }

    fn wait_for_events(&mut self, timeout: Duration) -> io::Result<Vec<i32>> {
        let timeout_ms = timeout.as_millis().min(i32::MAX as u128) as i32;
        let mut poll_fd = libc::pollfd {
            fd: self.fd,
            events: libc::POLLIN,
            revents: 0,
        };
        let poll_result = unsafe { libc::poll(&mut poll_fd as *mut libc::pollfd, 1, timeout_ms) };
        if poll_result < 0 {
            return Err(io::Error::last_os_error());
        }
        if poll_result == 0 {
            return Ok(Vec::new());
        }

        let mut events = Vec::new();
        let mut buffer = [0u8; EVENT_BUF_SIZE];

        loop {
            let read_size =
                unsafe { libc::read(self.fd, buffer.as_mut_ptr().cast(), buffer.len()) as isize };
            if read_size < 0 {
                let error = io::Error::last_os_error();
                if matches!(error.raw_os_error(), Some(code) if code == libc::EAGAIN || code == libc::EINTR)
                {
                    break;
                }
                return Err(error);
            }
            if read_size == 0 {
                break;
            }

            let mut offset = 0usize;
            let total = read_size as usize;
            while offset + mem::size_of::<libc::inotify_event>() <= total {
                let event_ptr =
                    unsafe { buffer.as_ptr().add(offset).cast::<libc::inotify_event>() };
                let event = unsafe { ptr::read_unaligned(event_ptr) };
                offset += mem::size_of::<libc::inotify_event>();

                let name_len = event.len as usize;
                if offset + name_len > total {
                    break;
                }
                offset += name_len;

                let pid_opt = self.wd_to_pid.get(&event.wd).copied();
                if event.mask & (libc::IN_DELETE_SELF | libc::IN_MOVE_SELF) != 0 {
                    if let Some(pid) = pid_opt {
                        self.remove_pid(pid);
                        events.push(pid);
                    }
                    continue;
                }
                if let Some(pid) = pid_opt {
                    events.push(pid);
                }
            }
        }
        Ok(events)
    }
}

impl Drop for InotifyWatch {
    fn drop(&mut self) {
        if self.fd >= 0 {
            unsafe {
                libc::close(self.fd);
            }
        }
    }
}

// ===== 區塊 4：超標後的重啟動作 =====
fn terminate_processes(pids: &[i32], sig: i32) {
    for pid in pids {
        unsafe {
            libc::kill(*pid, sig);
        }
    }
}

fn wait_until_gone(process_name: &str, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if find_pids_by_name(process_name).is_empty() {
            return true;
        }
        if Instant::now() >= deadline {
            return find_pids_by_name(process_name).is_empty();
        }
        thread::sleep(Duration::from_millis(200));
    }
}

fn start_process(command: &str) {
    let _ = Command::new("sh")
        .args(["-lc", command])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

// ===== 區塊 5：主事件迴圈 =====
struct Guard {
    config: Config,
    socket_path: String,
    inotify: InotifyWatch,
    last_restart: Option<Instant>,
}

impl Guard {
    fn new(config: Config) -> Result<Self, String> {
        let socket_path = display_to_socket(&config.display)?;
        let inotify = InotifyWatch::new().map_err(|err| format!("inotify 初始化失敗: {err}"))?;
        Ok(Self {
            config,
            socket_path,
            inotify,
            last_restart: None,
        })
    }

    fn sync_watches(&mut self) -> Vec<i32> {
        let pids = find_pids_by_name(&self.config.app_name);
        self.inotify.sync_pids(&pids);
        pids
    }

    fn restart_app(&mut self, x11_count: usize) {
        if let Some(last) = self.last_restart {
            let elapsed = last.elapsed().as_secs();
            if elapsed < self.config.cooldown_seconds {
                let remain = self.config.cooldown_seconds - elapsed;
                log(
                    &self.config,
                    &format!("超標但在冷卻期中，剩餘約 {remain} 秒"),
                );
                return;
            }
        }

        let pids = find_pids_by_name(&self.config.app_name);
        if pids.is_empty() {
            log(&self.config, "偵測超標時找不到目標程序，略過重啟");
            return;
        }

        log(
            &self.config,
            &format!(
                "{} X11 連線 {} 條，超過門檻 {}，準備重啟",
                self.config.app_name, x11_count, self.config.threshold
            ),
        );

        if self.config.dry_run {
            log(&self.config, "dry-run 模式：不會實際重啟程序");
            self.last_restart = Some(Instant::now());
            return;
        }

        terminate_processes(&pids, libc::SIGTERM);
        if !wait_until_gone(&self.config.app_name, Duration::from_secs(8)) {
            let remaining = find_pids_by_name(&self.config.app_name);
            if !remaining.is_empty() {
                terminate_processes(&remaining, libc::SIGKILL);
                let _ = wait_until_gone(&self.config.app_name, Duration::from_secs(3));
            }
        }
        start_process(&self.config.restart_cmd);
        self.last_restart = Some(Instant::now());
        log(
            &self.config,
            &format!("已執行重啟命令: {}", self.config.restart_cmd),
        );
    }

    fn check_threshold(&mut self, trigger: &str, pids: Option<Vec<i32>>) {
        let pids = if let Some(value) = pids {
            self.inotify.sync_pids(&value);
            value
        } else {
            self.sync_watches()
        };
        if pids.is_empty() {
            return;
        }

        let x11_count = count_app_x11_connections(&pids, &self.socket_path);
        if x11_count > self.config.threshold {
            self.restart_app(x11_count);
        } else if trigger == "fallback" {
            log(
                &self.config,
                &format!(
                    "目前 {} X11 連線 {} 條（門檻 {}）",
                    self.config.app_name, x11_count, self.config.threshold
                ),
            );
        }
    }

    fn run(&mut self) -> io::Result<()> {
        log(
            &self.config,
            &format!(
                "啟動監控，DISPLAY={}，門檻={}",
                self.config.display, self.config.threshold
            ),
        );

        let pids = self.sync_watches();
        self.check_threshold("startup", Some(pids));

        let mut next_sync = Instant::now() + Duration::from_secs(self.config.scan_interval_seconds);
        let mut next_fallback =
            Instant::now() + Duration::from_secs(self.config.fallback_poll_seconds);

        loop {
            let now = Instant::now();
            if now >= next_sync {
                self.sync_watches();
                next_sync = now + Duration::from_secs(self.config.scan_interval_seconds);
            }

            let timeout_to_sync = next_sync.saturating_duration_since(now);
            let timeout_to_fallback = next_fallback.saturating_duration_since(now);
            let timeout = timeout_to_sync
                .min(timeout_to_fallback)
                .max(Duration::from_millis(100));

            let events = self.inotify.wait_for_events(timeout)?;
            if !events.is_empty() {
                self.check_threshold("event", None);
            }

            let now = Instant::now();
            if now >= next_fallback {
                self.check_threshold("fallback", None);
                next_fallback = now + Duration::from_secs(self.config.fallback_poll_seconds);
            }
        }
    }
}

fn main() {
    let config = match parse_args() {
        Ok(value) => value,
        Err(error) => {
            eprintln!("參數錯誤: {error}");
            eprintln!("使用 --help 查看用法");
            std::process::exit(2);
        }
    };

    let mut guard = match Guard::new(config.clone()) {
        Ok(value) => value,
        Err(error) => {
            eprintln!("初始化失敗: {error}");
            std::process::exit(1);
        }
    };

    if let Err(error) = guard.run() {
        eprintln!("{} 執行錯誤: {}", timestamp(), error);
        std::process::exit(1);
    }
}
