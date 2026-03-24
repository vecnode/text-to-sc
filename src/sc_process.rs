//! SuperCollider **runtime discovery** for MCP tools: processes, install paths, and OSC reachability.
//!
//! - **Single snapshot** — an internal `collect_snapshot` pass feeds `probe`, `get_servers`, `discover_supercollider`, etc., so one
//!   scan gives consistent answers.
//! - **OSC `/status`** — complements name-based checks (e.g. sclang without scsynth).
//! - **Install detection** — Windows-oriented (`where.exe`, Program Files, running binary parents).

use std::collections::{BTreeSet, HashMap};
use std::env;
use std::fmt::Write as _;
use std::net::{SocketAddr, UdpSocket};
use std::panic;
use std::path::{Path, PathBuf};
use std::io::Read;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use sysinfo::{DiskUsage, Pid, ProcessRefreshKind, ProcessesToUpdate, System};

/// Embedded bootstrap: `sclang bootstrap.sc /path/to/snippet.sc` — compile-only, exit 0/1.
const SCLANG_SYNTAX_BOOTSTRAP: &str = include_str!("../assets/sclang_syntax_bootstrap.sc");

/// `sclang bootstrap.sc user.sc PORT` — `Server.remote` + interpret (live server).
const SCLANG_REMOTE_EXECUTE_BOOTSTRAP: &str = include_str!("../assets/sclang_remote_execute.sc");

/// Well-known default UDP ports scsynth may listen on (still combined with `-u` from argv when present).
const DEFAULT_SC_PORTS: [u16; 2] = [57110, 57120];

/// Headless `sclang` + class library + `Server.remote` can take tens of seconds; user code may loop — cap wait.
const SCLANG_EXECUTE_TIMEOUT: Duration = Duration::from_secs(120);

/// Poll interval while waiting for scsynth to exit or `/status` after respawn.
const SERVER_OSC_CONTROL_POLL: Duration = Duration::from_millis(200);
/// Max wait after OSC `/quit` for the old server PID to disappear.
const SERVER_QUIT_WAIT_MAX: Duration = Duration::from_secs(15);
/// Max wait after respawn for `/status.reply` on the UDP port.
const SERVER_BOOT_WAIT_MAX: Duration = Duration::from_secs(25);

/// Run a subprocess with captured stdout/stderr, draining pipes so Windows cannot deadlock on full buffers.
/// If `timeout` elapses before exit, the child is killed.
fn command_output_with_timeout(
    cmd: &mut Command,
    timeout: Duration,
) -> Result<std::process::Output, String> {
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("spawn failed: {e}"))?;
    let mut stdout_pipe = child.stdout.take().ok_or_else(|| "stdout pipe missing".to_string())?;
    let mut stderr_pipe = child.stderr.take().ok_or_else(|| "stderr pipe missing".to_string())?;

    let stdout_thread = thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout_pipe.read_to_end(&mut buf);
        buf
    });
    let stderr_thread = thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stderr_pipe.read_to_end(&mut buf);
        buf
    });

    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st,
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = stdout_thread.join();
                    let _ = stderr_thread.join();
                    return Err(format!(
                        "subprocess timed out after {}s (process killed)",
                        timeout.as_secs()
                    ));
                }
                thread::sleep(Duration::from_millis(50));
            }
            Err(e) => {
                let _ = child.kill();
                let _ = stdout_thread.join();
                let _ = stderr_thread.join();
                return Err(format!("try_wait failed: {e}"));
            }
        }
    };

    let stdout = stdout_thread
        .join()
        .map_err(|_| "stdout reader thread panicked".to_string())?;
    let stderr = stderr_thread
        .join()
        .map_err(|_| "stderr reader thread panicked".to_string())?;

    Ok(std::process::Output {
        status,
        stdout,
        stderr,
    })
}

// --- Snapshot DTOs (`discover_supercollider` embeds these via `serde_json`) ---

/// Best-effort install root plus resolved EXE paths.
#[derive(Clone, Serialize)]
struct ScInstallInfo {
    base_dir: String,
    sclang_path: Option<String>,
    scsynth_path: Option<String>,
    supernova_path: Option<String>,
}

/// Any tracked SuperCollider-related process (`sclang`, `scsynth`, `scide`, …).
#[derive(Clone, Serialize)]
struct ScProcessCandidate {
    pid: u32,
    name: String,
    exe_path: Option<String>,
    cmdline: String,
    role: String,
}

/// A live audio server binary (`scsynth` or `supernova`) plus stats and OSC probe result.
#[derive(Clone, Serialize)]
struct ScServerInstance {
    pid: u32,
    name: String,
    exe_path: Option<String>,
    cmdline: String,
    rss_bytes: u64,
    vsize_bytes: u64,
    cpu_raw_pct: f32,
    cpu_norm_pct: f32,
    uptime_s: u64,
    disk_read_bytes: u64,
    disk_written_bytes: u64,
    candidate_ports: Vec<u16>,
    responding_port: Option<u16>,
    osc_reachable: bool,
    install_match: String,
}

/// Full picture returned by one `collect_snapshot_impl` pass.
#[derive(Clone, Serialize)]
struct ScSnapshot {
    logical_cpus: usize,
    install: Option<ScInstallInfo>,
    servers: Vec<ScServerInstance>,
    candidates: Vec<ScProcessCandidate>,
    osc_port_results: HashMap<u16, bool>,
}

// --- Process image name → logical role (substring match on lowercase name) ---

fn is_scsynth(name_lower: &str) -> bool {
    name_lower.contains("scsynth")
}

fn is_supernova(name_lower: &str) -> bool {
    name_lower.contains("supernova")
}

fn is_sclang(name_lower: &str) -> bool {
    name_lower.contains("sclang")
}

fn is_scide(name_lower: &str) -> bool {
    name_lower.contains("scide")
}

fn path_to_string(p: &Path) -> String {
    p.to_string_lossy().to_string()
}

// --- OSC `/status` → `/status.reply` on UDP (localhost) ---

fn osc_padded_string(s: &str, out: &mut Vec<u8>) {
    out.extend_from_slice(s.as_bytes());
    out.push(0);
    while !out.len().is_multiple_of(4) {
        out.push(0);
    }
}

fn osc_status_packet() -> Vec<u8> {
    let mut buf = Vec::with_capacity(32);
    osc_padded_string("/status", &mut buf);
    osc_padded_string(",", &mut buf);
    buf
}

fn osc_status_alive(addr: SocketAddr) -> bool {
    let socket = match UdpSocket::bind("127.0.0.1:0") {
        Ok(s) => s,
        Err(_) => return false,
    };
    let _ = socket.set_read_timeout(Some(Duration::from_millis(250)));
    let _ = socket.set_write_timeout(Some(Duration::from_millis(250)));

    let packet = osc_status_packet();
    if socket.send_to(&packet, addr).is_err() {
        return false;
    }

    let mut buf = [0_u8; 2048];
    match socket.recv_from(&mut buf) {
        Ok((n, _src)) => buf[..n].windows(b"/status.reply".len()).any(|w| w == b"/status.reply"),
        Err(_) => false,
    }
}

fn osc_quit_packet() -> Vec<u8> {
    let mut buf = Vec::with_capacity(32);
    osc_padded_string("/quit", &mut buf);
    osc_padded_string(",", &mut buf);
    buf
}

/// Raw OSC `/quit` on UDP — works when `Server.quit` / `Server.reboot` refuse `Server.remote` instances.
fn send_osc_quit(port: u16) -> bool {
    let socket = match UdpSocket::bind("127.0.0.1:0") {
        Ok(s) => s,
        Err(_) => return false,
    };
    let _ = socket.set_write_timeout(Some(Duration::from_millis(500)));
    let packet = osc_quit_packet();
    socket
        .send_to(&packet, SocketAddr::from(([127, 0, 0, 1], port)))
        .is_ok()
}

fn process_alive(pid_u: u32) -> bool {
    let mut sys = System::new();
    sys.refresh_processes(ProcessesToUpdate::All, true);
    sys.process(Pid::from_u32(pid_u)).is_some()
}

fn wait_process_gone(pid: u32) -> bool {
    let deadline = Instant::now() + SERVER_QUIT_WAIT_MAX;
    while Instant::now() < deadline {
        if !process_alive(pid) {
            return true;
        }
        thread::sleep(SERVER_OSC_CONTROL_POLL);
    }
    false
}

fn wait_server_responding(port: u16) -> bool {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let deadline = Instant::now() + SERVER_BOOT_WAIT_MAX;
    while Instant::now() < deadline {
        if osc_status_alive(addr) {
            return true;
        }
        thread::sleep(SERVER_OSC_CONTROL_POLL);
    }
    false
}

fn pick_server_exe(srv: &ScServerInstance, install: Option<&ScInstallInfo>) -> Option<PathBuf> {
    if let Some(ref p) = srv.exe_path {
        let pb = PathBuf::from(p);
        if pb.is_file() {
            return Some(pb);
        }
    }
    let inst = install?;
    let name_lower = srv.name.to_lowercase();
    if is_supernova(&name_lower) {
        inst.supernova_path.as_ref().map(PathBuf::from)
    } else {
        inst.scsynth_path.as_ref().map(PathBuf::from)
    }
}

// --- Argv parsing: `scsynth -u 57112`, `-u=57112`, `-u57112` ---

fn parse_port_candidate(token: &str) -> Option<u16> {
    token.parse::<u16>().ok().filter(|p| *p > 0)
}

fn parse_sc_ports(cmdline: &str) -> Vec<u16> {
    let tokens: Vec<&str> = cmdline.split_whitespace().collect();
    let mut out = Vec::new();

    for (idx, tok) in tokens.iter().enumerate() {
        if *tok == "-u" || *tok == "--udp-port" {
            if let Some(next) = tokens.get(idx + 1).and_then(|v| parse_port_candidate(v)) {
                out.push(next);
            }
            continue;
        }
        if let Some(raw) = tok.strip_prefix("-u=") {
            if let Some(p) = parse_port_candidate(raw) {
                out.push(p);
            }
            continue;
        }
        if let Some(raw) = tok.strip_prefix("-u") {
            if let Some(p) = parse_port_candidate(raw) {
                out.push(p);
            }
        }
    }

    out.sort_unstable();
    out.dedup();
    out
}

// --- Install directory candidates (registry-style layout not required) ---

fn detect_install_candidates() -> Vec<PathBuf> {
    let mut dirs = BTreeSet::new();

    let roots = vec![
        env::var("ProgramFiles").ok().map(PathBuf::from),
        env::var("ProgramFiles(x86)").ok().map(PathBuf::from),
        Some(PathBuf::from(r"C:\Program Files")),
        Some(PathBuf::from(r"C:\Program Files (x86)")),
    ];

    for root in roots.into_iter().flatten() {
        dirs.insert(root.join("SuperCollider"));
        dirs.insert(root.join("SuperCollider").join("bin"));
        if let Ok(entries) = std::fs::read_dir(&root) {
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }
                let name = entry.file_name().to_string_lossy().to_lowercase();
                if name.contains("supercollider") {
                    dirs.insert(path.clone());
                    dirs.insert(path.join("bin"));
                }
            }
        }
    }

    dirs.into_iter().collect()
}

/// Resolve `binary` on `%PATH%` via the Windows `where` command.
fn paths_from_where(binary: &str) -> Vec<PathBuf> {
    let output = match Command::new("where").arg(binary).output() {
        Ok(o) if o.status.success() => o,
        _ => return Vec::new(),
    };
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(PathBuf::from)
        .filter(|p| p.exists())
        .collect()
}

/// Derive likely install roots from live `sclang` / `scsynth` / `scide` EXE paths.
fn process_hint_dirs() -> Vec<PathBuf> {
    let mut sys = System::new();
    sys.refresh_processes(ProcessesToUpdate::All, true);
    let mut dirs = BTreeSet::new();

    for (_pid, proc_) in sys.processes() {
        let name = proc_.name().to_string_lossy().to_lowercase();
        if !(is_scsynth(&name) || is_supernova(&name) || is_sclang(&name) || is_scide(&name)) {
            continue;
        }
        if let Some(exe) = proc_.exe() {
            if let Some(parent) = exe.parent() {
                dirs.insert(parent.to_path_buf());
                if parent.ends_with("bin") {
                    if let Some(up) = parent.parent() {
                        dirs.insert(up.to_path_buf());
                    }
                } else {
                    dirs.insert(parent.join("bin"));
                }
            }
        }
    }
    dirs.into_iter().collect()
}

fn detect_install_impl() -> Option<ScInstallInfo> {
    let mut candidates = BTreeSet::new();
    for base in detect_install_candidates() {
        candidates.insert(base);
    }
    for p in paths_from_where("sclang.exe")
        .into_iter()
        .chain(paths_from_where("scsynth.exe"))
        .chain(paths_from_where("supernova.exe"))
    {
        if let Some(parent) = p.parent() {
            candidates.insert(parent.to_path_buf());
            if parent.ends_with("bin") {
                if let Some(up) = parent.parent() {
                    candidates.insert(up.to_path_buf());
                }
            } else {
                candidates.insert(parent.join("bin"));
            }
        }
    }
    for hint in process_hint_dirs() {
        candidates.insert(hint);
    }

    for base in candidates {
        let sclang = base.join("sclang.exe");
        let scsynth = base.join("scsynth.exe");
        let supernova = base.join("supernova.exe");
        if sclang.exists() || scsynth.exists() || supernova.exists() {
            return Some(ScInstallInfo {
                base_dir: path_to_string(&base),
                sclang_path: sclang.exists().then(|| path_to_string(&sclang)),
                scsynth_path: scsynth.exists().then(|| path_to_string(&scsynth)),
                supernova_path: supernova.exists().then(|| path_to_string(&supernova)),
            });
        }
    }
    None
}

/// Exposes the first resolved install root for sibling modules (e.g. docs indexer).
pub fn detect_install_base_dir() -> Option<PathBuf> {
    detect_install_impl().map(|i| PathBuf::from(i.base_dir))
}

fn parse_version_from_text(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    for i in 0..bytes.len() {
        if !bytes[i].is_ascii_digit() {
            continue;
        }
        let mut j = i;
        let mut dots = 0_u8;
        while j < bytes.len() && (bytes[j].is_ascii_digit() || bytes[j] == b'.') {
            if bytes[j] == b'.' {
                dots += 1;
            }
            j += 1;
        }
        if dots >= 1 && j > i {
            return Some(s[i..j].to_string());
        }
    }
    None
}

fn version_from_sclang_binary(path: &str) -> Option<String> {
    let output = Command::new(path).arg("-v").output().ok()?;
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    parse_version_from_text(&text)
}

/// Compile `code` with `sclang` without executing it (class library still loads once).
pub fn check_sclang_syntax(code: &str) -> String {
    let Some(inst) = detect_install_impl() else {
        return "check_sclang_syntax failed: SuperCollider install not detected — run detect_supercollider_install.".to_string();
    };
    let Some(ref sclang) = inst.sclang_path else {
        return "check_sclang_syntax failed: sclang executable not found next to detected install.".to_string();
    };

    let nanos = match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_nanos(),
        Err(_) => 0,
    };
    let dir = std::env::temp_dir().join(format!("supercollider-mcp-syntax-{nanos}"));
    if let Err(e) = std::fs::create_dir_all(&dir) {
        return format!("check_sclang_syntax failed: temp dir: {e}");
    }
    let boot = dir.join("bootstrap.sc");
    let snippet = dir.join("snippet.sc");
    if let Err(e) = std::fs::write(&boot, SCLANG_SYNTAX_BOOTSTRAP) {
        let _ = std::fs::remove_dir_all(&dir);
        return format!("check_sclang_syntax failed: write bootstrap: {e}");
    }
    if let Err(e) = std::fs::write(&snippet, code) {
        let _ = std::fs::remove_dir_all(&dir);
        return format!("check_sclang_syntax failed: write snippet: {e}");
    }

    let output = Command::new(sclang).arg(&boot).arg(&snippet).output();
    let _ = std::fs::remove_dir_all(&dir);

    let output = match output {
        Ok(o) => o,
        Err(e) => return format!("check_sclang_syntax failed: spawn sclang ({sclang}): {e}"),
    };

    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let ok = output.status.success();

    fn tail_lines(s: &str, max: usize) -> String {
        let lines: Vec<&str> = s.lines().collect();
        let n = lines.len();
        let start = n.saturating_sub(max);
        lines[start..].join("\n")
    }

    if ok {
        format!(
            "sclang syntax: OK\n- sclang={sclang}\n- note: snippet was compile-checked only (not executed). Loading SC class library adds ~0.5–2s overhead per run."
        )
    } else {
        format!(
            "sclang syntax: ERROR\n- sclang={sclang}\n- exit_code={:?}\n--- stderr (last lines) ---\n{}\n--- stdout (last lines) ---\n{}",
            output.status.code(),
            tail_lines(&stderr, 40),
            tail_lines(&stdout, 20)
        )
    }
}

fn resolve_control_target(
    server_pid: Option<u32>,
    osc_port: Option<u16>,
) -> Result<(ScServerInstance, u16), String> {
    if let Some(port) = osc_port.filter(|&p| p > 0) {
        let snap = collect_snapshot()?;
        let srv = snap
            .servers
            .iter()
            .find(|s| s.responding_port == Some(port))
            .or_else(|| snap.servers.first())
            .ok_or_else(|| {
                "No SuperCollider audio server found; boot scsynth/supernova first.".to_string()
            })?;
        return Ok((srv.clone(), port));
    }
    let snap = collect_snapshot()?;
    let srv = match server_pid {
        Some(pid) => snap
            .servers
            .iter()
            .find(|s| s.pid == pid)
            .ok_or_else(|| format!("No server process with pid={pid} (scsynth/supernova)."))?,
        None => snap
            .servers
            .iter()
            .find(|s| s.osc_reachable)
            .or_else(|| snap.servers.first())
            .ok_or_else(|| {
                "No SuperCollider audio server found; boot scsynth/supernova first.".to_string()
            })?,
    };
    let port = srv.responding_port.ok_or_else(|| {
        format!(
            "Could not resolve OSC UDP port for pid {} (no /status.reply on probed ports).",
            srv.pid
        )
    })?;
    Ok((srv.clone(), port))
}

fn resolve_execute_target(
    server_pid: Option<u32>,
    osc_port: Option<u16>,
) -> Result<(u16, u32), String> {
    let (srv, port) = resolve_control_target(server_pid, osc_port)?;
    Ok((port, srv.pid))
}

/// Stop the **scsynth/supernova process** via OSC `/quit` (not the same as freeing synth nodes).  
/// `Server.quit` / `Server.reboot` do not apply to `Server.remote` in sclang; this uses raw UDP.  
/// Does not restart the binary — use `reboot_supercollider_server` for quit+respawn.
pub fn quit_supercollider_server(server_pid: Option<u32>, osc_port: Option<u16>) -> String {
    let (srv, port) = match resolve_control_target(server_pid, osc_port) {
        Ok(x) => x,
        Err(e) => return format!("quit_supercollider_server failed: {e}"),
    };
    if !send_osc_quit(port) {
        return format!("quit_supercollider_server failed: UDP send /quit to 127.0.0.1:{port} failed");
    }
    let gone = wait_process_gone(srv.pid);
    format!(
        "quit_supercollider_server: {}\n- sent OSC /quit to 127.0.0.1:{port}\n- previous_pid={}\n- process_exited={gone}\n- note: Restart scsynth from the SuperCollider IDE or `reboot_supercollider_server` if you need it running again.",
        if gone { "OK" } else { "PARTIAL (PID still alive or still exiting; check get_servers)" },
        srv.pid
    )
}

/// OSC `/quit` then spawn the same class of server (`scsynth` or `supernova`) with **`-u <port>` only**.  
/// Use when `s.reboot` is unavailable (remote server). If audio fails, boot from the IDE with your usual flags.
pub fn reboot_supercollider_server(server_pid: Option<u32>, osc_port: Option<u16>) -> String {
    let install = detect_install_impl();
    let (srv, port) = match resolve_control_target(server_pid, osc_port) {
        Ok(x) => x,
        Err(e) => return format!("reboot_supercollider_server failed: {e}"),
    };
    let Some(exe) = pick_server_exe(&srv, install.as_ref()) else {
        return "reboot_supercollider_server failed: could not resolve scsynth.exe / supernova.exe path (exe_path missing and install not detected).".to_string();
    };
    if !send_osc_quit(port) {
        return format!("reboot_supercollider_server failed: UDP send /quit to 127.0.0.1:{port} failed");
    }
    if !wait_process_gone(srv.pid) {
        return format!(
            "reboot_supercollider_server failed: pid {} still alive after /quit (waited {:?}). Try quit_supercollider_server again or end the process manually.",
            srv.pid, SERVER_QUIT_WAIT_MAX
        );
    }

    let child = match Command::new(&exe).arg("-u").arg(port.to_string()).spawn() {
        Ok(c) => c,
        Err(e) => {
            return format!(
                "reboot_supercollider_server failed: spawn {:?} -u {}: {e}",
                exe, port
            );
        }
    };
    let new_id = child.id();

    if wait_server_responding(port) {
        format!(
            "reboot_supercollider_server: OK\n- exe={}\n- osc_udp_port={port}\n- previous_pid={}\n- spawned_pid={new_id}\n- /status.reply: yes\n- note: minimal args (-u only). Prefer IDE boot if your setup needs extra flags.",
            path_to_string(&exe),
            srv.pid
        )
    } else {
        format!(
            "reboot_supercollider_server: PARTIAL\n- exe={}\n- osc_udp_port={port}\n- previous_pid={}\n- spawned_pid={new_id}\n- /status.reply: no within {:?}\n- process may still be starting; check get_servers / ping_supercollider.",
            path_to_string(&exe),
            srv.pid,
            SERVER_BOOT_WAIT_MAX
        )
    }
}

/// After `Server.remote`, `Server.default` is the target; this frees all nodes in the default group (same as IDE “stop” scope for that client).
const STOP_ALL_SYNTHS_SCLANG: &str = "Server.default.freeAll;";

/// Free every synth/node under this client’s default group on the live server (via `execute_supercollider_code`).
///
/// Note: `Synth.freeAll` is not valid SuperCollider; use `Server.default.freeAll`.
pub fn stop_supercollider_synths(server_pid: Option<u32>, osc_port: Option<u16>) -> String {
    let inner = execute_supercollider_code(STOP_ALL_SYNTHS_SCLANG, server_pid, osc_port);
    if inner.starts_with("execute_supercollider_code: OK") {
        inner.replacen(
            "execute_supercollider_code: OK",
            "stop_supercollider_synths: OK (interpreted Server.default.freeAll)",
            1,
        )
    } else if inner.starts_with("execute_supercollider_code: ERROR") {
        inner.replacen(
            "execute_supercollider_code: ERROR",
            "stop_supercollider_synths: ERROR",
            1,
        )
    } else if inner.starts_with("execute_supercollider_code failed:") {
        inner.replacen(
            "execute_supercollider_code failed:",
            "stop_supercollider_synths failed:",
            1,
        )
    } else {
        format!("stop_supercollider_synths:\n{inner}")
    }
}

/// Run `code` on the **live** scsynth/supernova using headless `sclang` and `Server.remote` to `127.0.0.1:port`.
///
/// **Security:** this is arbitrary code execution with audio side effects — only enable this MCP on trusted hosts.
pub fn execute_supercollider_code(
    code: &str,
    server_pid: Option<u32>,
    osc_port: Option<u16>,
) -> String {
    if code.trim().is_empty() {
        return "execute_supercollider_code: code is empty.".to_string();
    }

    let (port, srv_pid) = match resolve_execute_target(server_pid, osc_port) {
        Ok(x) => x,
        Err(e) => return format!("execute_supercollider_code failed: {e}"),
    };

    let Some(inst) = detect_install_impl() else {
        return "execute_supercollider_code failed: SuperCollider install not detected.".to_string();
    };
    let Some(ref sclang) = inst.sclang_path else {
        return "execute_supercollider_code failed: sclang not found in install.".to_string();
    };

    let nanos = match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_nanos(),
        Err(_) => 0,
    };
    let dir = std::env::temp_dir().join(format!("supercollider-mcp-exec-{nanos}"));
    if let Err(e) = std::fs::create_dir_all(&dir) {
        return format!("execute_supercollider_code failed: temp dir: {e}");
    }
    let boot = dir.join("bootstrap.sc");
    let snippet = dir.join("user.sc");
    if let Err(e) = std::fs::write(&boot, SCLANG_REMOTE_EXECUTE_BOOTSTRAP) {
        let _ = std::fs::remove_dir_all(&dir);
        return format!("execute_supercollider_code failed: write bootstrap: {e}");
    }
    if let Err(e) = std::fs::write(&snippet, code) {
        let _ = std::fs::remove_dir_all(&dir);
        return format!("execute_supercollider_code failed: write user.sc: {e}");
    }

    let port_str = port.to_string();
    let mut cmd = Command::new(sclang);
    cmd.arg(&boot).arg(&snippet).arg(&port_str);
    let output = command_output_with_timeout(&mut cmd, SCLANG_EXECUTE_TIMEOUT);
    let _ = std::fs::remove_dir_all(&dir);

    let output = match output {
        Ok(o) => o,
        Err(e) => {
            return format!(
                "execute_supercollider_code failed: {e}\n- sclang={sclang}\n- target_server_pid={srv_pid}\n- osc_udp_port={port}\n- hint: if this timed out, user code may be stuck in an infinite loop, or the server may be unreachable / refusing extra clients (see scsynth maxLogins)."
            );
        }
    };

    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let ok = output.status.success();

    fn tail_lines(s: &str, max: usize) -> String {
        let lines: Vec<&str> = s.lines().collect();
        let n = lines.len();
        let start = n.saturating_sub(max);
        lines[start..].join("\n")
    }

    if ok {
        format!(
            "execute_supercollider_code: OK\n- sclang={sclang}\n- target_server_pid={srv_pid}\n- osc_udp_port={port}\n- note: interpreted via Server.remote(127.0.0.1:{port}). Schedules may outlive this short sclang process.\n--- stdout (tail) ---\n{}",
            tail_lines(&stdout, 25)
        )
    } else {
        format!(
            "execute_supercollider_code: ERROR\n- sclang={sclang}\n- target_server_pid={srv_pid}\n- osc_udp_port={port}\n- exit_code={:?}\n--- stderr (tail) ---\n{}\n--- stdout (tail) ---\n{}",
            output.status.code(),
            tail_lines(&stderr, 45),
            tail_lines(&stdout, 25)
        )
    }
}

pub fn get_supercollider_version() -> String {
    let mut out = String::new();
    let install = detect_install_impl();
    if let Some(i) = &install {
        let _ = writeln!(out, "SuperCollider install detected at {}", i.base_dir);
        if let Some(v) = parse_version_from_text(&i.base_dir) {
            let _ = writeln!(out, "- version_from_path={v}");
        }
        if let Some(sclang) = &i.sclang_path {
            let _ = writeln!(out, "- sclang_path={sclang}");
            if let Some(v) = version_from_sclang_binary(sclang) {
                let _ = writeln!(out, "- version_from_sclang={v}");
            } else {
                out.push_str("- version_from_sclang=<unavailable>\n");
            }
        }
        if let Some(scsynth) = &i.scsynth_path {
            let _ = writeln!(out, "- scsynth_path={scsynth}");
        }
    } else {
        out.push_str("SuperCollider install not detected.\n");
    }

    let snapshot = match collect_snapshot() {
        Ok(s) => s,
        Err(_) => return out,
    };
    let mut seen = false;
    for c in snapshot.candidates {
        if c.role == "scsynth" || c.role == "sclang" || c.role == "supernova" {
            if let Some(p) = c.exe_path {
                if !seen {
                    out.push_str("Running binaries:\n");
                    seen = true;
                }
                let _ = writeln!(out, "- pid={} role={} exe={}", c.pid, c.role, p);
                if let Some(v) = parse_version_from_text(&p) {
                    let _ = writeln!(out, "  version_hint={v}");
                }
            }
        }
    }
    out
}

fn install_match_for(
    process_name_lower: &str,
    exe_path: Option<&str>,
    install: Option<&ScInstallInfo>,
) -> String {
    let Some(inst) = install else {
        return "install_not_detected".to_string();
    };
    let Some(exe) = exe_path else {
        return "exe_path_unavailable".to_string();
    };
    let exe_lower = exe.to_lowercase();

    let expected = if is_scsynth(process_name_lower) {
        inst.scsynth_path.as_ref()
    } else if is_supernova(process_name_lower) {
        inst.supernova_path.as_ref()
    } else if is_sclang(process_name_lower) {
        inst.sclang_path.as_ref()
    } else {
        None
    };

    match expected {
        Some(exp) if exe_lower == exp.to_lowercase() => "exact_match".to_string(),
        Some(exp) => format!("mismatch (expected={exp})"),
        None => "install_binary_missing".to_string(),
    }
}

// --- One sysinfo pass: candidates, servers, merged OSC probe set ---

fn collect_snapshot_impl() -> ScSnapshot {
    let mut sys = System::new();
    sys.refresh_cpu_all();
    sys.refresh_processes(ProcessesToUpdate::All, true);
    thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
    sys.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing().with_cpu().with_memory(),
    );

    let n_cpu = sys.cpus().len().max(1);
    let install = detect_install_impl();
    let mut candidates = Vec::new();
    let mut servers = Vec::new();

    let mut probe_ports: BTreeSet<u16> = DEFAULT_SC_PORTS.into_iter().collect();
    let mut per_pid_ports: HashMap<u32, Vec<u16>> = HashMap::new();

    for (pid, proc_) in sys.processes() {
        let name = proc_.name().to_string_lossy().to_string();
        let name_lower = name.to_lowercase();
        let exe_path = proc_.exe().map(path_to_string);
        let cmdline = proc_
            .cmd()
            .iter()
            .map(|s| s.to_string_lossy().into_owned())
            .collect::<Vec<String>>()
            .join(" ");

        let role = if is_scsynth(&name_lower) {
            "scsynth"
        } else if is_supernova(&name_lower) {
            "supernova"
        } else if is_sclang(&name_lower) {
            "sclang"
        } else if is_scide(&name_lower) {
            "scide"
        } else {
            continue;
        };

        let pid_u = pid.as_u32();
        let parsed_ports = parse_sc_ports(&cmdline);
        for port in &parsed_ports {
            probe_ports.insert(*port);
        }
        per_pid_ports.insert(pid_u, parsed_ports.clone());

        candidates.push(ScProcessCandidate {
            pid: pid_u,
            name: name.clone(),
            exe_path: exe_path.clone(),
            cmdline: cmdline.clone(),
            role: role.to_string(),
        });

        if role == "scsynth" || role == "supernova" {
            let disk: DiskUsage = proc_.disk_usage();
            servers.push(ScServerInstance {
                pid: pid_u,
                name: name.clone(),
                exe_path: exe_path.clone(),
                cmdline,
                rss_bytes: proc_.memory(),
                vsize_bytes: proc_.virtual_memory(),
                cpu_raw_pct: proc_.cpu_usage(),
                cpu_norm_pct: proc_.cpu_usage() / n_cpu as f32,
                uptime_s: proc_.run_time(),
                disk_read_bytes: disk.total_read_bytes,
                disk_written_bytes: disk.total_written_bytes,
                candidate_ports: Vec::new(),
                responding_port: None,
                osc_reachable: false,
                install_match: install_match_for(&name_lower, exe_path.as_deref(), install.as_ref()),
            });
        }
    }

    let mut osc_port_results = HashMap::new();
    for port in probe_ports {
        let alive = osc_status_alive(SocketAddr::from(([127, 0, 0, 1], port)));
        osc_port_results.insert(port, alive);
    }

    for srv in &mut servers {
        let mut ports = per_pid_ports.remove(&srv.pid).unwrap_or_default();
        for p in DEFAULT_SC_PORTS {
            if !ports.contains(&p) {
                ports.push(p);
            }
        }
        ports.sort_unstable();
        ports.dedup();
        srv.responding_port = ports.iter().copied().find(|p| *osc_port_results.get(p).unwrap_or(&false));
        srv.osc_reachable = srv.responding_port.is_some();
        srv.candidate_ports = ports;
    }

    candidates.sort_by_key(|c| c.pid);
    servers.sort_by_key(|s| s.pid);

    ScSnapshot {
        logical_cpus: n_cpu,
        install,
        servers,
        candidates,
        osc_port_results,
    }
}

/// Wraps `collect_snapshot_impl` so a `sysinfo` panic becomes an `Err` instead of killing the MCP server.
fn collect_snapshot() -> Result<ScSnapshot, String> {
    panic::catch_unwind(collect_snapshot_impl).map_err(|_| {
        "internal panic while collecting SuperCollider snapshot (sysinfo/OS)".to_string()
    })
}

/// Mebibytes for human-readable tool output (1024-based, matches typical task managers).
pub(crate) fn mib(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

fn fmt_disk(read: u64, write: u64) -> String {
    format!("disk_read={read} disk_written={write} (bytes total since measure)")
}

/// Quick health check with process and OSC summary.
pub fn probe(message: Option<&str>) -> String {
    let snapshot = match collect_snapshot() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[supercollider-mcp] probe: {e}");
            return format!("Process scan failed unexpectedly. Check supercollider-mcp stderr.\nReason: {e}");
        }
    };

    let mut out = String::new();
    out.push_str("SuperCollider health check (process + OSC + install):\n");

    let scsynth: Vec<String> = snapshot
        .candidates
        .iter()
        .filter(|c| c.role == "scsynth")
        .map(|c| format!("pid {} ({})", c.pid, c.name))
        .collect();
    let supernova: Vec<String> = snapshot
        .candidates
        .iter()
        .filter(|c| c.role == "supernova")
        .map(|c| format!("pid {} ({})", c.pid, c.name))
        .collect();
    let sclang: Vec<String> = snapshot
        .candidates
        .iter()
        .filter(|c| c.role == "sclang")
        .map(|c| format!("pid {} ({})", c.pid, c.name))
        .collect();

    if scsynth.is_empty() {
        out.push_str("- scsynth: not found\n");
    } else {
        let _ = writeln!(out, "- scsynth: running — {}", scsynth.join(", "));
    }
    if supernova.is_empty() {
        out.push_str("- supernova: not found\n");
    } else {
        let _ = writeln!(out, "- supernova: running — {}", supernova.join(", "));
    }
    if sclang.is_empty() {
        out.push_str("- sclang: not found\n");
    } else {
        let _ = writeln!(out, "- sclang: running — {}", sclang.join(", "));
    }

    let mut alive_ports: Vec<u16> = snapshot
        .osc_port_results
        .iter()
        .filter_map(|(p, alive)| (*alive).then_some(*p))
        .collect();
    alive_ports.sort_unstable();
    if alive_ports.is_empty() {
        let mut checked: Vec<u16> = snapshot.osc_port_results.keys().copied().collect();
        checked.sort_unstable();
        let _ = writeln!(
            out,
            "- osc localhost: no /status.reply on checked ports {:?}",
            checked
        );
    } else {
        let _ = writeln!(out, "- osc localhost: /status.reply on ports {:?}", alive_ports);
    }

    let alive = snapshot.servers.iter().any(|s| s.osc_reachable);
    let _ = writeln!(out, "- server_alive={alive}");

    match &snapshot.install {
        Some(i) => {
            let _ = writeln!(out, "- install: detected at {}", i.base_dir);
        }
        None => out.push_str("- install: not detected in standard paths\n"),
    }
    out.push_str("- supercollider-mcp: running\n");

    if let Some(m) = message.filter(|m| !m.trim().is_empty()) {
        let _ = writeln!(out, "Note: {m}");
    }
    out
}

/// Detailed inventory of running server processes.
pub fn get_servers() -> String {
    let snapshot = match collect_snapshot() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[supercollider-mcp] get_servers: {e}");
            return format!("get_servers failed unexpectedly. Check supercollider-mcp stderr.\nReason: {e}");
        }
    };

    if snapshot.servers.is_empty() {
        return "No SuperCollider server process found (scsynth/supernova).\nBoot the server from sclang/IDE and call this tool again.".to_string();
    }

    let mut out = String::new();
    out.push_str("SuperCollider server process(es) on this machine:\n\n");

    for row in &snapshot.servers {
        let line = format!(
            "pid={} name={} rss={:.1} MiB vsize={:.1} MiB cpu~={:.1}% cpu_raw={:.1}% uptime={}s ports={:?} responding_port={:?} osc_reachable={} install_match={} {}",
            row.pid,
            row.name,
            mib(row.rss_bytes),
            mib(row.vsize_bytes),
            row.cpu_norm_pct,
            row.cpu_raw_pct,
            row.uptime_s,
            row.candidate_ports,
            row.responding_port,
            row.osc_reachable,
            row.install_match,
            fmt_disk(row.disk_read_bytes, row.disk_written_bytes)
        );
        eprintln!("[supercollider-mcp] get_servers: {line}");
        let _ = writeln!(out, "- {line}");
    }
    let _ = writeln!(
        out,
        "\nCPU % is estimated from OS counters ({} logical CPUs); not SuperCollider DSP load.",
        snapshot.logical_cpus
    );
    out
}

/// Unified high-fidelity discovery with machine-friendly JSON block.
pub fn discover_supercollider() -> String {
    let snapshot = match collect_snapshot() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[supercollider-mcp] discover_supercollider: {e}");
            return format!("discover_supercollider failed unexpectedly. Reason: {e}");
        }
    };

    let alive = snapshot.servers.iter().any(|s| s.osc_reachable);
    let mut out = String::new();
    let _ = writeln!(out, "SuperCollider discovery: server_alive={alive}");
    let _ = writeln!(
        out,
        "servers={} candidates={} checked_ports={}",
        snapshot.servers.len(),
        snapshot.candidates.len(),
        snapshot.osc_port_results.len()
    );
    out.push_str("\nJSON:\n");
    match serde_json::to_string_pretty(&snapshot) {
        Ok(json) => out.push_str(&json),
        Err(e) => {
            let _ = write!(out, "{{\"error\":\"failed to serialize snapshot: {e}\"}}");
        }
    }
    out
}

/// Status for one server PID (or first running server).
pub fn get_server_status(pid: Option<u32>) -> String {
    let snapshot = match collect_snapshot() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[supercollider-mcp] get_server_status: {e}");
            return format!("get_server_status failed unexpectedly. Reason: {e}");
        }
    };

    let target = match pid {
        Some(p) => snapshot.servers.iter().find(|s| s.pid == p),
        None => snapshot
            .servers
            .iter()
            .find(|s| s.osc_reachable)
            .or_else(|| snapshot.servers.first()),
    };

    let Some(srv) = target else {
        return "No SuperCollider server process currently running (scsynth/supernova).".to_string();
    };

    let mut out = String::new();
    let _ = writeln!(out, "Server status for pid={}", srv.pid);
    let _ = writeln!(out, "- name={}", srv.name);
    let _ = writeln!(out, "- exe_path={}", srv.exe_path.as_deref().unwrap_or("<unknown>"));
    let _ = writeln!(out, "- osc_reachable={}", srv.osc_reachable);
    let _ = writeln!(out, "- responding_port={:?}", srv.responding_port);
    let _ = writeln!(out, "- candidate_ports={:?}", srv.candidate_ports);
    let _ = writeln!(out, "- install_match={}", srv.install_match);
    let _ = writeln!(out, "- rss_mib={:.1}", mib(srv.rss_bytes));
    let _ = writeln!(out, "- cpu_norm_pct={:.1}", srv.cpu_norm_pct);
    let _ = writeln!(out, "- uptime_s={}", srv.uptime_s);
    out
}

/// Detect SuperCollider installation and known executable locations.
pub fn detect_supercollider_install() -> String {
    let install = detect_install_impl();
    let mut out = String::new();
    match install {
        Some(i) => {
            out.push_str("SuperCollider install detected.\n");
            let _ = writeln!(out, "- base_dir={}", i.base_dir);
            let _ = writeln!(out, "- sclang_path={}", i.sclang_path.unwrap_or_else(|| "<missing>".to_string()));
            let _ = writeln!(out, "- scsynth_path={}", i.scsynth_path.unwrap_or_else(|| "<missing>".to_string()));
            let _ = writeln!(out, "- supernova_path={}", i.supernova_path.unwrap_or_else(|| "<missing>".to_string()));
        }
        None => out.push_str("SuperCollider install not detected in standard Windows paths.\n"),
    }
    out
}

fn push_path_if_exists(label: &str, path: &Path, out: &mut String) {
    if path.exists() {
        let _ = writeln!(out, "- {}={}", label, path_to_string(path));
    }
}

/// Local `Help` / `HelpSource` paths plus optional online mirrors of the same material.
pub fn get_server_docs() -> String {
    let install = detect_install_impl();
    let mut out = String::new();
    out.push_str("SuperCollider docs pointers:\n");
    out.push_str("- note: Prefer local paths and search_supercollider_docs / answer_supercollider_docs (offline). Online entries are optional mirrors of doc.sccode.org.\n");

    if let Some(i) = install {
        let base = PathBuf::from(&i.base_dir);
        let help_dir = base.join("Help");
        let help_source = base.join("HelpSource");
        let _ = writeln!(out, "- local_install_dir={}", i.base_dir);
        push_path_if_exists("local_help_dir", &help_dir, &mut out);
        push_path_if_exists("local_help_source_dir", &help_source, &mut out);

        push_path_if_exists(
            "local_server_command_reference_schelp",
            &help_source.join("Reference").join("Server-Command-Reference.schelp"),
            &mut out,
        );
        push_path_if_exists(
            "local_server_guide_schelp",
            &help_source.join("Guides").join("Server-Guide.schelp"),
            &mut out,
        );
        push_path_if_exists(
            "local_server_class_schelp",
            &help_source.join("Classes").join("Server.schelp"),
            &mut out,
        );
        push_path_if_exists(
            "local_server_command_reference_html",
            &help_dir.join("Reference").join("Server-Command-Reference.html"),
            &mut out,
        );
    } else {
        out.push_str("- local_install_dir=<not detected>\n");
    }

    let _ = writeln!(out, "- online_docs_mirror=https://doc.sccode.org/");
    let _ = writeln!(out, "- online_docs=https://doc.sccode.org/ (alias of online_docs_mirror)");
    let _ = writeln!(
        out,
        "- online_server_command_reference_mirror=https://doc.sccode.org/Reference/Server-Command-Reference.html"
    );
    let _ = writeln!(
        out,
        "- server_command_reference=https://doc.sccode.org/Reference/Server-Command-Reference.html (alias: same URL as online_server_command_reference_mirror)"
    );
    out
}

/// List all detected SuperCollider-related processes (debugging helper).
pub fn list_server_candidates() -> String {
    let snapshot = match collect_snapshot() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[supercollider-mcp] list_server_candidates: {e}");
            return format!("list_server_candidates failed unexpectedly. Reason: {e}");
        }
    };

    if snapshot.candidates.is_empty() {
        return "No SuperCollider-related processes found (scsynth/supernova/sclang/scide).".to_string();
    }

    let mut out = String::new();
    out.push_str("SuperCollider process candidates:\n");
    for c in snapshot.candidates {
        let _ = writeln!(
            out,
            "- pid={} role={} name={} exe={} cmdline={}",
            c.pid,
            c.role,
            c.name,
            c.exe_path.unwrap_or_else(|| "<unknown>".to_string()),
            if c.cmdline.is_empty() { "<empty>" } else { &c.cmdline }
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mib_roundtrip_small() {
        assert!((mib(1024 * 1024) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn name_filters() {
        assert!(is_scsynth("scsynth.exe"));
        assert!(is_sclang("sclang"));
        assert!(!is_scsynth("notepad"));
    }

    #[test]
    fn parse_ports_from_cmdline() {
        let a = parse_sc_ports("scsynth.exe -u 57112 -z 64");
        assert_eq!(a, vec![57112]);
        let b = parse_sc_ports("scsynth.exe -u=57113");
        assert_eq!(b, vec![57113]);
        let c = parse_sc_ports("supernova.exe -u57114");
        assert_eq!(c, vec![57114]);
    }
}
