//! SuperCollider discovery and health checks (process + OSC + install matching).

use std::collections::{BTreeSet, HashMap};
use std::env;
use std::fmt::Write as _;
use std::net::{SocketAddr, UdpSocket};
use std::panic;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::Duration;

use serde::Serialize;
use sysinfo::{DiskUsage, ProcessRefreshKind, ProcessesToUpdate, System};

const DEFAULT_SC_PORTS: [u16; 2] = [57110, 57120];

#[derive(Clone, Serialize)]
struct ScInstallInfo {
    base_dir: String,
    sclang_path: Option<String>,
    scsynth_path: Option<String>,
    supernova_path: Option<String>,
}

#[derive(Clone, Serialize)]
struct ScProcessCandidate {
    pid: u32,
    name: String,
    exe_path: Option<String>,
    cmdline: String,
    role: String,
}

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

#[derive(Clone, Serialize)]
struct ScSnapshot {
    logical_cpus: usize,
    install: Option<ScInstallInfo>,
    servers: Vec<ScServerInstance>,
    candidates: Vec<ScProcessCandidate>,
    osc_port_results: HashMap<u16, bool>,
}

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

fn collect_snapshot() -> Result<ScSnapshot, String> {
    panic::catch_unwind(collect_snapshot_impl).map_err(|_| {
        "internal panic while collecting SuperCollider snapshot (sysinfo/OS)".to_string()
    })
}

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

/// Return useful local docs paths and online docs links.
pub fn get_server_docs() -> String {
    let install = detect_install_impl();
    let mut out = String::new();
    out.push_str("SuperCollider docs pointers:\n");
    if let Some(i) = install {
        let help_dir = PathBuf::from(&i.base_dir).join("Help");
        let _ = writeln!(out, "- local_help_dir={}", path_to_string(&help_dir));
        let _ = writeln!(out, "- local_install_dir={}", i.base_dir);
    } else {
        out.push_str("- local_install_dir=<not detected>\n");
    }
    out.push_str("- online_docs=https://doc.sccode.org/\n");
    out.push_str("- server_command_reference=https://doc.sccode.org/Reference/Server-Command-Reference.html\n");
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
