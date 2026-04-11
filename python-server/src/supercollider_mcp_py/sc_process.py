from __future__ import annotations

import json
import ctypes
import os
import platform
import random
import re
import shutil
import socket
import subprocess
import tempfile
import threading
import time
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import Iterable

try:
    import winreg  # type: ignore
except Exception:
    winreg = None  # type: ignore[assignment]

import psutil

DEFAULT_SC_PORTS = (57110, 57120)
SCLANG_EXECUTE_TIMEOUT_S = 120
SERVER_OSC_CONTROL_POLL_S = 0.2
SERVER_QUIT_WAIT_MAX_S = 15
SERVER_BOOT_WAIT_MAX_S = 25

# Tracks which SuperCollider server PID this MCP is actively controlling.
GLOBAL_SUPERCOLIDER_APP_PID: int | None = None
_GLOBAL_SUPERCOLIDER_APP_PID_LOCK = threading.Lock()
GLOBAL_SUPERCOLIDER_ACTIVE: bool = False
_GLOBAL_SUPERCOLIDER_ACTIVE_LOCK = threading.Lock()
SERVER_LOG_PATHS: dict[int, str] = {}
_SERVER_LOG_PATHS_LOCK = threading.Lock()
LOADED_MCP_TONE_PORTS: set[int] = set()
_LOADED_MCP_TONE_PORTS_LOCK = threading.Lock()

ASSETS_DIR = Path(__file__).resolve().parents[3] / "assets"
SCLANG_SYNTAX_BOOTSTRAP = (ASSETS_DIR / "sclang_syntax_bootstrap.sc").read_text(encoding="utf-8")
SCLANG_REMOTE_EXECUTE_BOOTSTRAP = (ASSETS_DIR / "sclang_remote_execute.sc").read_text(encoding="utf-8")


@dataclass(slots=True)
class ScInstallInfo:
    base_dir: str
    sclang_path: str | None
    scsynth_path: str | None
    supernova_path: str | None


@dataclass(slots=True)
class ScProcessCandidate:
    pid: int
    name: str
    exe_path: str | None
    cmdline: str
    role: str


@dataclass(slots=True)
class ScServerInstance:
    pid: int
    name: str
    exe_path: str | None
    cmdline: str
    rss_bytes: int
    vsize_bytes: int
    cpu_raw_pct: float
    cpu_norm_pct: float
    uptime_s: int
    disk_read_bytes: int
    disk_written_bytes: int
    candidate_ports: list[int]
    responding_port: int | None
    osc_reachable: bool
    install_match: str


@dataclass(slots=True)
class ScSnapshot:
    logical_cpus: int
    install: ScInstallInfo | None
    servers: list[ScServerInstance]
    candidates: list[ScProcessCandidate]
    osc_port_results: dict[int, bool]


def is_scsynth(name_lower: str) -> bool:
    return "scsynth" in name_lower


def is_supernova(name_lower: str) -> bool:
    return "supernova" in name_lower


def is_sclang(name_lower: str) -> bool:
    return "sclang" in name_lower


def is_scide(name_lower: str) -> bool:
    return "scide" in name_lower


def mib(bytes_count: int) -> float:
    return float(bytes_count) / (1024.0 * 1024.0)


def _osc_padded_string(s: str) -> bytes:
    raw = s.encode("utf-8") + b"\x00"
    while len(raw) % 4 != 0:
        raw += b"\x00"
    return raw


def _osc_status_packet() -> bytes:
    return _osc_padded_string("/status") + _osc_padded_string(",")


def _osc_quit_packet() -> bytes:
    return _osc_padded_string("/quit") + _osc_padded_string(",")


def _align4(offset: int) -> int:
    return (offset + 3) & ~3


def _osc_read_padded_string(data: bytes, offset: int) -> tuple[str, int] | None:
    if offset >= len(data):
        return None
    try:
        end = data.index(0, offset)
    except ValueError:
        return None
    try:
        value = data[offset:end].decode("utf-8", errors="replace")
    except Exception:
        return None
    return value, _align4(end + 1)


def _osc_int32(v: int) -> bytes:
    return int(v).to_bytes(4, byteorder="big", signed=True)


def _osc_float32(v: float) -> bytes:
    import struct

    return struct.pack(">f", float(v))


def _osc_message(address: str, args: list[object]) -> bytes:
    tags = [","]
    payload = bytearray()
    for arg in args:
        if isinstance(arg, bool):
            tags.append("i")
            payload.extend(_osc_int32(1 if arg else 0))
        elif isinstance(arg, int):
            tags.append("i")
            payload.extend(_osc_int32(arg))
        elif isinstance(arg, float):
            tags.append("f")
            payload.extend(_osc_float32(arg))
        elif isinstance(arg, str):
            tags.append("s")
            payload.extend(_osc_padded_string(arg))
        else:
            raise ValueError(f"unsupported OSC arg type: {type(arg)!r}")
    return _osc_padded_string(address) + _osc_padded_string("".join(tags)) + bytes(payload)


def send_osc_message(port: int, address: str, args: list[object]) -> bool:
    try:
        packet = _osc_message(address, args)
        with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as sock:
            sock.settimeout(0.5)
            sock.sendto(packet, ("127.0.0.1", port))
        return True
    except OSError:
        return False
    except ValueError:
        return False


def osc_status_metrics(port: int) -> dict[str, float | int | str] | None:
    try:
        with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as sock:
            sock.settimeout(0.35)
            sock.sendto(_osc_status_packet(), ("127.0.0.1", port))
            data, _ = sock.recvfrom(4096)
    except OSError:
        return None

    head = _osc_read_padded_string(data, 0)
    if head is None:
        return None
    address, idx = head
    if address != "/status.reply":
        return None

    tags_read = _osc_read_padded_string(data, idx)
    if tags_read is None:
        return None
    tags, idx = tags_read
    if not tags.startswith(","):
        return None

    args: list[int | float] = []
    import struct

    for tag in tags[1:]:
        if tag == "i":
            if idx + 4 > len(data):
                return None
            args.append(int.from_bytes(data[idx:idx + 4], byteorder="big", signed=True))
            idx += 4
        elif tag == "f":
            if idx + 4 > len(data):
                return None
            args.append(float(struct.unpack(">f", data[idx:idx + 4])[0]))
            idx += 4
        elif tag == "d":
            if idx + 8 > len(data):
                return None
            args.append(float(struct.unpack(">d", data[idx:idx + 8])[0]))
            idx += 8
        else:
            return None

    out: dict[str, float | int | str] = {
        "port": port,
        "address": address,
        "raw_args": str(args),
    }
    # Typical scsynth layout:
    # [unused, ugen_count, synth_count, group_count, synthdef_count, avg_cpu, peak_cpu, nominal_sr, actual_sr]
    if len(args) >= 9:
        out["ugen_count"] = int(args[1])
        out["synth_count"] = int(args[2])
        out["group_count"] = int(args[3])
        out["synthdef_count"] = int(args[4])
        out["avg_cpu"] = float(args[5])
        out["peak_cpu"] = float(args[6])
        out["nominal_sr"] = float(args[7])
        out["actual_sr"] = float(args[8])
    return out


def osc_status_alive(port: int) -> bool:
    try:
        with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as sock:
            sock.settimeout(0.25)
            sock.sendto(_osc_status_packet(), ("127.0.0.1", port))
            data, _ = sock.recvfrom(2048)
            return b"/status.reply" in data
    except OSError:
        return False


def send_osc_quit(port: int) -> bool:
    try:
        with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as sock:
            sock.settimeout(0.5)
            sock.sendto(_osc_quit_packet(), ("127.0.0.1", port))
        return True
    except OSError:
        return False


def parse_port_candidate(token: str) -> int | None:
    try:
        value = int(token)
    except ValueError:
        return None
    return value if value > 0 else None


def parse_sc_ports(cmdline: str) -> list[int]:
    tokens = cmdline.split()
    ports: set[int] = set()
    for idx, tok in enumerate(tokens):
        if tok in {"-u", "--udp-port"}:
            if idx + 1 < len(tokens):
                p = parse_port_candidate(tokens[idx + 1])
                if p is not None:
                    ports.add(p)
            continue
        if tok.startswith("-u="):
            p = parse_port_candidate(tok[3:])
            if p is not None:
                ports.add(p)
            continue
        if tok.startswith("-u") and len(tok) > 2:
            p = parse_port_candidate(tok[2:])
            if p is not None:
                ports.add(p)
    return sorted(ports)


def detect_install_candidates() -> list[Path]:
    candidates: set[Path] = set()
    roots: list[Path] = []
    for env_name in ("ProgramFiles", "ProgramFiles(x86)"):
        v = os.getenv(env_name)
        if v:
            roots.append(Path(v))
    roots.extend([Path(r"C:\Program Files"), Path(r"C:\Program Files (x86)")])

    for root in roots:
        candidates.add(root / "SuperCollider")
        candidates.add(root / "SuperCollider" / "bin")
        if not root.exists():
            continue
        for entry in root.iterdir():
            if not entry.is_dir():
                continue
            name = entry.name.lower()
            if "supercollider" in name:
                candidates.add(entry)
                candidates.add(entry / "bin")

    return sorted(candidates)


def paths_from_where(binary: str) -> list[Path]:
    try:
        out = subprocess.check_output(["where", binary], text=True, stderr=subprocess.DEVNULL)
    except Exception:
        return []
    results = []
    for line in out.splitlines():
        path = Path(line.strip())
        if path.exists():
            results.append(path)
    return results


def process_hint_dirs() -> list[Path]:
    dirs: set[Path] = set()
    for proc in psutil.process_iter(["name", "exe"]):
        try:
            name = (proc.info.get("name") or "").lower()
            if not (is_scsynth(name) or is_supernova(name) or is_sclang(name) or is_scide(name)):
                continue
            exe = proc.info.get("exe")
            if not exe:
                continue
            parent = Path(exe).parent
            dirs.add(parent)
            if parent.name.lower() == "bin":
                dirs.add(parent.parent)
            else:
                dirs.add(parent / "bin")
        except (psutil.NoSuchProcess, psutil.AccessDenied):
            continue
    return sorted(dirs)


def detect_install_impl() -> ScInstallInfo | None:
    candidates: set[Path] = set(detect_install_candidates())

    for p in paths_from_where("sclang.exe") + paths_from_where("scsynth.exe") + paths_from_where("supernova.exe"):
        parent = p.parent
        candidates.add(parent)
        if parent.name.lower() == "bin":
            candidates.add(parent.parent)
        else:
            candidates.add(parent / "bin")

    for hint in process_hint_dirs():
        candidates.add(hint)

    for base in sorted(candidates):
        sclang = base / "sclang.exe"
        scsynth = base / "scsynth.exe"
        supernova = base / "supernova.exe"
        if sclang.exists() or scsynth.exists() or supernova.exists():
            return ScInstallInfo(
                base_dir=str(base),
                sclang_path=str(sclang) if sclang.exists() else None,
                scsynth_path=str(scsynth) if scsynth.exists() else None,
                supernova_path=str(supernova) if supernova.exists() else None,
            )
    return None


def detect_install_base_dir() -> Path | None:
    install = detect_install_impl()
    return Path(install.base_dir) if install else None


def parse_version_from_text(text: str) -> str | None:
    m = re.search(r"\d+(?:\.\d+)+", text)
    return m.group(0) if m else None


def version_from_sclang_binary(path: str) -> str | None:
    try:
        proc = subprocess.run([path, "-v"], capture_output=True, text=True, timeout=10)
    except Exception:
        return None
    return parse_version_from_text(f"{proc.stdout}\n{proc.stderr}")


def _command_output_with_timeout(command: list[str], timeout_s: int) -> tuple[int, str, str] | str:
    proc = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    try:
        stdout, stderr = proc.communicate(timeout=timeout_s)
    except subprocess.TimeoutExpired:
        proc.kill()
        proc.communicate()
        return f"subprocess timed out after {timeout_s}s (process killed)"
    return proc.returncode, stdout, stderr


def _tail_lines(text: str, max_lines: int) -> str:
    lines = text.splitlines()
    return "\n".join(lines[-max_lines:])


def check_sclang_syntax(code: str) -> str:
    install = detect_install_impl()
    if not install:
        return "check_sclang_syntax failed: SuperCollider install not detected - run detect_supercollider_install."
    if not install.sclang_path:
        return "check_sclang_syntax failed: sclang executable not found next to detected install."

    workdir = Path(tempfile.mkdtemp(prefix="supercollider-mcp-syntax-"))
    try:
        boot = workdir / "bootstrap.sc"
        snippet = workdir / "snippet.sc"
        boot.write_text(SCLANG_SYNTAX_BOOTSTRAP, encoding="utf-8")
        snippet.write_text(code, encoding="utf-8")

        result = _command_output_with_timeout([install.sclang_path, str(boot), str(snippet)], 30)
        if isinstance(result, str):
            return f"check_sclang_syntax failed: {result}"
        return_code, stdout, stderr = result

        if return_code == 0:
            return (
                "sclang syntax: OK\n"
                f"- sclang={install.sclang_path}\n"
                "- note: snippet was compile-checked only (not executed). "
                "Loading SC class library adds ~0.5-2s overhead per run."
            )

        return (
            "sclang syntax: ERROR\n"
            f"- sclang={install.sclang_path}\n"
            f"- exit_code={return_code}\n"
            "--- stderr (last lines) ---\n"
            f"{_tail_lines(stderr, 40)}\n"
            "--- stdout (last lines) ---\n"
            f"{_tail_lines(stdout, 20)}"
        )
    except Exception as exc:
        return f"check_sclang_syntax failed: {exc}"
    finally:
        shutil.rmtree(workdir, ignore_errors=True)


def process_alive(pid: int) -> bool:
    return psutil.pid_exists(pid)


def wait_process_gone(pid: int) -> bool:
    deadline = time.time() + SERVER_QUIT_WAIT_MAX_S
    while time.time() < deadline:
        if not process_alive(pid):
            return True
        time.sleep(SERVER_OSC_CONTROL_POLL_S)
    return False


def wait_server_responding(port: int) -> bool:
    deadline = time.time() + SERVER_BOOT_WAIT_MAX_S
    while time.time() < deadline:
        if osc_status_alive(port):
            return True
        time.sleep(SERVER_OSC_CONTROL_POLL_S)
    return False


def get_global_supercolider_app_pid() -> int | None:
    with _GLOBAL_SUPERCOLIDER_APP_PID_LOCK:
        return GLOBAL_SUPERCOLIDER_APP_PID


def set_global_supercolider_app_pid(pid: int | None) -> None:
    global GLOBAL_SUPERCOLIDER_APP_PID
    with _GLOBAL_SUPERCOLIDER_APP_PID_LOCK:
        GLOBAL_SUPERCOLIDER_APP_PID = pid


def get_global_supercolider_active() -> bool:
    with _GLOBAL_SUPERCOLIDER_ACTIVE_LOCK:
        return GLOBAL_SUPERCOLIDER_ACTIVE


def set_global_supercolider_active(active: bool) -> None:
    global GLOBAL_SUPERCOLIDER_ACTIVE
    with _GLOBAL_SUPERCOLIDER_ACTIVE_LOCK:
        GLOBAL_SUPERCOLIDER_ACTIVE = bool(active)


def _adopt_active_server_from_snapshot(snapshot: ScSnapshot) -> ScServerInstance | None:
    running = [s for s in snapshot.servers if s.osc_reachable]
    if not running:
        set_global_supercolider_active(False)
        set_global_supercolider_app_pid(None)
        return None

    tracked_pid = get_global_supercolider_app_pid()
    chosen = next((s for s in running if tracked_pid is not None and s.pid == tracked_pid), None)
    if chosen is None:
        chosen = running[0]

    set_global_supercolider_app_pid(chosen.pid)
    set_global_supercolider_active(True)
    return chosen


def _set_server_log_path(pid: int, path: str) -> None:
    with _SERVER_LOG_PATHS_LOCK:
        SERVER_LOG_PATHS[pid] = path


def _get_server_log_path(pid: int) -> str | None:
    with _SERVER_LOG_PATHS_LOCK:
        return SERVER_LOG_PATHS.get(pid)


def _clear_server_log_path(pid: int) -> None:
    with _SERVER_LOG_PATHS_LOCK:
        SERVER_LOG_PATHS.pop(pid, None)


def _is_mcp_tone_loaded_for_port(port: int) -> bool:
    with _LOADED_MCP_TONE_PORTS_LOCK:
        return port in LOADED_MCP_TONE_PORTS


def _mark_mcp_tone_loaded_for_port(port: int) -> None:
    with _LOADED_MCP_TONE_PORTS_LOCK:
        LOADED_MCP_TONE_PORTS.add(port)


def _tail_file(path: str, max_lines: int = 40) -> str:
    p = Path(path)
    if not p.exists():
        return "<log file missing>"
    try:
        text = p.read_text(encoding="utf-8", errors="replace")
    except Exception as exc:
        return f"<failed to read log: {exc}>"
    return _tail_lines(text, max_lines)


def _run_powershell_json(script: str) -> list[dict[str, object]]:
    cmd = [
        "powershell",
        "-NoProfile",
        "-ExecutionPolicy",
        "Bypass",
        "-Command",
        script,
    ]
    try:
        proc = subprocess.run(cmd, capture_output=True, text=True, timeout=30)
    except Exception:
        return []
    if proc.returncode != 0:
        return []
    raw = (proc.stdout or "").strip()
    if not raw:
        return []
    try:
        data = json.loads(raw)
    except Exception:
        return []
    if isinstance(data, list):
        return [d for d in data if isinstance(d, dict)]
    if isinstance(data, dict):
        return [data]
    return []


def _get_sound_mapper_defaults() -> dict[str, str]:
    out: dict[str, str] = {}
    if winreg is None:
        return out
    try:
        with winreg.OpenKey(winreg.HKEY_CURRENT_USER, r"Software\Microsoft\Multimedia\Sound Mapper") as key:
            for value_name in ("Playback", "Record"):
                try:
                    value, _ = winreg.QueryValueEx(key, value_name)
                    out[value_name.lower()] = str(value)
                except OSError:
                    continue
    except OSError:
        pass
    return out


def _get_winmm_wave_caps() -> dict[str, list[dict[str, object]]]:
    if os.name != "nt":
        return {"wave_out": [], "wave_in": []}

    class WAVEOUTCAPSW(ctypes.Structure):
        _fields_ = [
            ("wMid", ctypes.c_ushort),
            ("wPid", ctypes.c_ushort),
            ("vDriverVersion", ctypes.c_uint),
            ("szPname", ctypes.c_wchar * 32),
            ("dwFormats", ctypes.c_uint),
            ("wChannels", ctypes.c_ushort),
            ("wReserved1", ctypes.c_ushort),
            ("dwSupport", ctypes.c_uint),
        ]

    class WAVEINCAPSW(ctypes.Structure):
        _fields_ = [
            ("wMid", ctypes.c_ushort),
            ("wPid", ctypes.c_ushort),
            ("vDriverVersion", ctypes.c_uint),
            ("szPname", ctypes.c_wchar * 32),
            ("dwFormats", ctypes.c_uint),
            ("wChannels", ctypes.c_ushort),
            ("wReserved1", ctypes.c_ushort),
        ]

    wave_out: list[dict[str, object]] = []
    wave_in: list[dict[str, object]] = []
    try:
        winmm = ctypes.windll.winmm  # type: ignore[attr-defined]
    except Exception:
        return {"wave_out": wave_out, "wave_in": wave_in}

    try:
        out_count = int(winmm.waveOutGetNumDevs())
    except Exception:
        out_count = 0
    for i in range(out_count):
        caps = WAVEOUTCAPSW()
        try:
            mmres = int(winmm.waveOutGetDevCapsW(i, ctypes.byref(caps), ctypes.sizeof(caps)))
        except Exception:
            mmres = -1
        if mmres == 0:
            wave_out.append(
                {
                    "id": i,
                    "name": caps.szPname,
                    "channels": int(caps.wChannels),
                    "formats_mask": int(caps.dwFormats),
                    "support_mask": int(caps.dwSupport),
                    "mid": int(caps.wMid),
                    "pid": int(caps.wPid),
                    "driver_version": int(caps.vDriverVersion),
                }
            )

    try:
        in_count = int(winmm.waveInGetNumDevs())
    except Exception:
        in_count = 0
    for i in range(in_count):
        caps = WAVEINCAPSW()
        try:
            mmres = int(winmm.waveInGetDevCapsW(i, ctypes.byref(caps), ctypes.sizeof(caps)))
        except Exception:
            mmres = -1
        if mmres == 0:
            wave_in.append(
                {
                    "id": i,
                    "name": caps.szPname,
                    "channels": int(caps.wChannels),
                    "formats_mask": int(caps.dwFormats),
                    "mid": int(caps.wMid),
                    "pid": int(caps.wPid),
                    "driver_version": int(caps.vDriverVersion),
                }
            )

    return {"wave_out": wave_out, "wave_in": wave_in}


def get_audio_cards_raw_info() -> str:
    script = (
        "Get-CimInstance Win32_SoundDevice | "
        "Select-Object Name,Manufacturer,Status,PNPDeviceID,ProductName,DeviceID | "
        "ConvertTo-Json -Depth 4"
    )
    rows = _run_powershell_json(script)
    payload = {
        "host_platform": platform.platform(),
        "os_name": os.name,
        "count": len(rows),
        "sound_devices": rows,
    }
    return json.dumps(payload, indent=2)


def get_audio_endpoints_raw_info() -> str:
    # MEDIA class includes playback/capture endpoints and drivers visible to PnP.
    script = (
        "Get-CimInstance Win32_PnPEntity -Filter \"PNPClass='MEDIA'\" | "
        "Select-Object Name,Manufacturer,Status,Service,PNPDeviceID,DeviceID,ClassGuid | "
        "ConvertTo-Json -Depth 4"
    )
    rows = _run_powershell_json(script)
    payload = {
        "host_platform": platform.platform(),
        "count": len(rows),
        "media_endpoints": rows,
    }
    return json.dumps(payload, indent=2)


def get_audio_stack_report() -> str:
    cards = _run_powershell_json(
        "Get-CimInstance Win32_SoundDevice | "
        "Select-Object Name,Manufacturer,Status,PNPDeviceID,ProductName,DeviceID | "
        "ConvertTo-Json -Depth 4"
    )
    media = _run_powershell_json(
        "Get-CimInstance Win32_PnPEntity -Filter \"PNPClass='MEDIA'\" | "
        "Select-Object Name,Manufacturer,Status,Service,PNPDeviceID,DeviceID,ClassGuid | "
        "ConvertTo-Json -Depth 4"
    )
    payload = {
        "host_platform": platform.platform(),
        "os_name": os.name,
        "sound_mapper_defaults": _get_sound_mapper_defaults(),
        "winmm_caps": _get_winmm_wave_caps(),
        "sound_device_count": len(cards),
        "media_endpoint_count": len(media),
        "sound_devices": cards,
        "media_endpoints": media,
    }
    return json.dumps(payload, indent=2)


def pick_server_exe(srv: ScServerInstance, install: ScInstallInfo | None) -> Path | None:
    if srv.exe_path and Path(srv.exe_path).is_file():
        return Path(srv.exe_path)
    if not install:
        return None
    if is_supernova(srv.name.lower()) and install.supernova_path:
        return Path(install.supernova_path)
    if install.scsynth_path:
        return Path(install.scsynth_path)
    return None


def install_match_for(process_name_lower: str, exe_path: str | None, install: ScInstallInfo | None) -> str:
    if not install:
        return "install_not_detected"
    if not exe_path:
        return "exe_path_unavailable"
    exe_lower = exe_path.lower()

    expected: str | None = None
    if is_scsynth(process_name_lower):
        expected = install.scsynth_path
    elif is_supernova(process_name_lower):
        expected = install.supernova_path
    elif is_sclang(process_name_lower):
        expected = install.sclang_path

    if not expected:
        return "install_binary_missing"
    if exe_lower == expected.lower():
        return "exact_match"
    return f"mismatch (expected={expected})"


def _safe_cmdline(cmdline: Iterable[str] | None) -> str:
    if not cmdline:
        return ""
    return " ".join(cmdline)


def collect_snapshot() -> ScSnapshot:
    logical_cpus = max(psutil.cpu_count(logical=True) or 1, 1)
    install = detect_install_impl()

    candidates: list[ScProcessCandidate] = []
    servers: list[ScServerInstance] = []

    probe_ports: set[int] = set(DEFAULT_SC_PORTS)
    per_pid_ports: dict[int, list[int]] = {}

    for proc in psutil.process_iter(["pid", "name", "exe", "cmdline", "memory_info", "cpu_percent", "create_time", "io_counters"]):
        try:
            pid = int(proc.info.get("pid") or 0)
            name = proc.info.get("name") or ""
            name_lower = name.lower()

            role = ""
            if is_scsynth(name_lower):
                role = "scsynth"
            elif is_supernova(name_lower):
                role = "supernova"
            elif is_sclang(name_lower):
                role = "sclang"
            elif is_scide(name_lower):
                role = "scide"
            else:
                continue

            cmdline = _safe_cmdline(proc.info.get("cmdline"))
            exe_path = proc.info.get("exe")
            parsed_ports = parse_sc_ports(cmdline)
            per_pid_ports[pid] = parsed_ports
            probe_ports.update(parsed_ports)

            candidates.append(
                ScProcessCandidate(
                    pid=pid,
                    name=name,
                    exe_path=exe_path,
                    cmdline=cmdline,
                    role=role,
                )
            )

            if role in {"scsynth", "supernova"}:
                mem = proc.info.get("memory_info")
                io = proc.info.get("io_counters")
                rss = int(mem.rss) if mem else 0
                vms = int(mem.vms) if mem else 0
                cpu_raw = float(proc.info.get("cpu_percent") or 0.0)
                create_time = float(proc.info.get("create_time") or time.time())
                uptime = max(int(time.time() - create_time), 0)
                read_b = int(io.read_bytes) if io else 0
                write_b = int(io.write_bytes) if io else 0

                servers.append(
                    ScServerInstance(
                        pid=pid,
                        name=name,
                        exe_path=exe_path,
                        cmdline=cmdline,
                        rss_bytes=rss,
                        vsize_bytes=vms,
                        cpu_raw_pct=cpu_raw,
                        cpu_norm_pct=cpu_raw / float(logical_cpus),
                        uptime_s=uptime,
                        disk_read_bytes=read_b,
                        disk_written_bytes=write_b,
                        candidate_ports=[],
                        responding_port=None,
                        osc_reachable=False,
                        install_match=install_match_for(name_lower, exe_path, install),
                    )
                )
        except (psutil.NoSuchProcess, psutil.AccessDenied):
            continue

    osc_port_results: dict[int, bool] = {}
    for port in sorted(probe_ports):
        osc_port_results[port] = osc_status_alive(port)

    for srv in servers:
        ports = sorted(set((per_pid_ports.get(srv.pid) or []) + list(DEFAULT_SC_PORTS)))
        srv.candidate_ports = ports
        srv.responding_port = next((p for p in ports if osc_port_results.get(p, False)), None)
        srv.osc_reachable = srv.responding_port is not None

    candidates.sort(key=lambda c: c.pid)
    servers.sort(key=lambda s: s.pid)

    return ScSnapshot(
        logical_cpus=logical_cpus,
        install=install,
        servers=servers,
        candidates=candidates,
        osc_port_results=osc_port_results,
    )


def _fmt_disk(read: int, write: int) -> str:
    return f"disk_read={read} disk_written={write} (bytes total since measure)"


def probe(message: str | None = None) -> str:
    try:
        snapshot = collect_snapshot()
    except Exception as exc:
        return (
            "Process scan failed unexpectedly. Check supercollider-mcp stderr.\n"
            f"Reason: {exc}"
        )

    _adopt_active_server_from_snapshot(snapshot)

    scsynth = [f"pid {c.pid} ({c.name})" for c in snapshot.candidates if c.role == "scsynth"]
    supernova = [f"pid {c.pid} ({c.name})" for c in snapshot.candidates if c.role == "supernova"]
    sclang = [f"pid {c.pid} ({c.name})" for c in snapshot.candidates if c.role == "sclang"]

    lines = ["SuperCollider health check (process + OSC + install):"]
    lines.append(f"- scsynth: {'running - ' + ', '.join(scsynth) if scsynth else 'not found'}")
    lines.append(f"- supernova: {'running - ' + ', '.join(supernova) if supernova else 'not found'}")
    lines.append(f"- sclang: {'running - ' + ', '.join(sclang) if sclang else 'not found'}")

    alive_ports = sorted([p for p, ok in snapshot.osc_port_results.items() if ok])
    if alive_ports:
        lines.append(f"- osc localhost: /status.reply on ports {alive_ports}")
    else:
        checked = sorted(snapshot.osc_port_results.keys())
        lines.append(f"- osc localhost: no /status.reply on checked ports {checked}")

    server_alive = any(s.osc_reachable for s in snapshot.servers)
    lines.append(f"- server_alive={server_alive}")
    lines.append(f"- GLOBAL_SUPERCOLIDER_APP_PID={get_global_supercolider_app_pid()}")
    lines.append(f"- GLOBAL_SUPERCOLIDER_ACTIVE={get_global_supercolider_active()}")

    if snapshot.install:
        lines.append(f"- install: detected at {snapshot.install.base_dir}")
        if not server_alive:
            lines.append("- action: call start_supercollider_server() to boot scsynth (no IDE needed).")
    else:
        lines.append("- install: not detected in standard paths")

    lines.append("- supercollider-mcp: running")
    if message and message.strip():
        lines.append(f"Note: {message}")
    return "\n".join(lines)


def get_servers() -> str:
    try:
        snapshot = collect_snapshot()
    except Exception as exc:
        return f"get_servers failed unexpectedly. Check supercollider-mcp stderr.\nReason: {exc}"

    _adopt_active_server_from_snapshot(snapshot)
    tracked_pid = get_global_supercolider_app_pid()
    tracked_active = get_global_supercolider_active()

    if not snapshot.servers:
        lines = ["No SuperCollider server process found (scsynth/supernova)."]
        lines.append(f"- GLOBAL_SUPERCOLIDER_APP_PID={tracked_pid}")
        lines.append(f"- GLOBAL_SUPERCOLIDER_ACTIVE={tracked_active}")
        if snapshot.install:
            lines.append(f"- install detected at {snapshot.install.base_dir}")
            lines.append("- action: call start_supercollider_server() to boot scsynth directly (no IDE needed).")
        else:
            lines.append("- action: call detect_supercollider_install() first — no SC install found.")
        return "\n".join(lines)

    lines = [
        "SuperCollider server process(es) on this machine:",
        f"- GLOBAL_SUPERCOLIDER_APP_PID={tracked_pid}",
        f"- GLOBAL_SUPERCOLIDER_ACTIVE={tracked_active}",
        "",
    ]
    for row in snapshot.servers:
        line = (
            f"pid={row.pid} name={row.name} rss={mib(row.rss_bytes):.1f} MiB "
            f"vsize={mib(row.vsize_bytes):.1f} MiB cpu~={row.cpu_norm_pct:.1f}% "
            f"cpu_raw={row.cpu_raw_pct:.1f}% uptime={row.uptime_s}s "
            f"ports={row.candidate_ports} responding_port={row.responding_port} "
            f"osc_reachable={row.osc_reachable} install_match={row.install_match} "
            f"{_fmt_disk(row.disk_read_bytes, row.disk_written_bytes)}"
        )
        lines.append(f"- {line}")

    lines.append("")
    lines.append(
        f"CPU % is estimated from OS counters ({snapshot.logical_cpus} logical CPUs); not SuperCollider DSP load."
    )
    return "\n".join(lines)


def discover_supercollider() -> str:
    try:
        snapshot = collect_snapshot()
    except Exception as exc:
        return f"discover_supercollider failed unexpectedly. Reason: {exc}"

    _adopt_active_server_from_snapshot(snapshot)
    alive = any(s.osc_reachable for s in snapshot.servers)
    header = (
        f"SuperCollider discovery: server_alive={alive}\n"
        f"servers={len(snapshot.servers)} candidates={len(snapshot.candidates)} "
        f"checked_ports={len(snapshot.osc_port_results)}\n\nJSON:\n"
    )
    payload = {
        "logical_cpus": snapshot.logical_cpus,
        "GLOBAL_SUPERCOLIDER_APP_PID": get_global_supercolider_app_pid(),
        "GLOBAL_SUPERCOLIDER_ACTIVE": get_global_supercolider_active(),
        "install": asdict(snapshot.install) if snapshot.install else None,
        "servers": [asdict(s) for s in snapshot.servers],
        "candidates": [asdict(c) for c in snapshot.candidates],
        "osc_port_results": snapshot.osc_port_results,
    }
    return header + json.dumps(payload, indent=2)


def get_server_status(pid: int | None = None) -> str:
    try:
        snapshot = collect_snapshot()
    except Exception as exc:
        return f"get_server_status failed unexpectedly. Reason: {exc}"

    _adopt_active_server_from_snapshot(snapshot)

    target = None
    if pid is not None:
        target = next((s for s in snapshot.servers if s.pid == pid), None)
    else:
        target = next((s for s in snapshot.servers if s.osc_reachable), None)
        if target is None and snapshot.servers:
            target = snapshot.servers[0]

    if not target:
        install = detect_install_impl()
        if install:
            return (
                "No SuperCollider server process currently running (scsynth/supernova).\n"
                f"- install detected at {install.base_dir}\n"
                f"- GLOBAL_SUPERCOLIDER_APP_PID={get_global_supercolider_app_pid()}\n"
                f"- GLOBAL_SUPERCOLIDER_ACTIVE={get_global_supercolider_active()}\n"
                "- action: call start_supercollider_server() to boot scsynth directly."
            )
        return (
            "No SuperCollider server process currently running (scsynth/supernova).\n"
            f"- GLOBAL_SUPERCOLIDER_APP_PID={get_global_supercolider_app_pid()}\n"
            f"- GLOBAL_SUPERCOLIDER_ACTIVE={get_global_supercolider_active()}\n"
            "- action: call detect_supercollider_install() — no SC install found yet."
        )

    set_global_supercolider_app_pid(target.pid)
    set_global_supercolider_active(True)
    return "\n".join(
        [
            f"Server status for pid={target.pid}",
            f"- GLOBAL_SUPERCOLIDER_APP_PID={get_global_supercolider_app_pid()}",
            f"- GLOBAL_SUPERCOLIDER_ACTIVE={get_global_supercolider_active()}",
            f"- name={target.name}",
            f"- exe_path={target.exe_path or '<unknown>'}",
            f"- osc_reachable={target.osc_reachable}",
            f"- responding_port={target.responding_port}",
            f"- candidate_ports={target.candidate_ports}",
            f"- install_match={target.install_match}",
            f"- rss_mib={mib(target.rss_bytes):.1f}",
            f"- cpu_norm_pct={target.cpu_norm_pct:.1f}",
            f"- uptime_s={target.uptime_s}",
        ]
    )


def detect_supercollider_install() -> str:
    install = detect_install_impl()
    if not install:
        return "SuperCollider install not detected in standard Windows paths.\n"

    return "\n".join(
        [
            "SuperCollider install detected.",
            f"- base_dir={install.base_dir}",
            f"- sclang_path={install.sclang_path or '<missing>'}",
            f"- scsynth_path={install.scsynth_path or '<missing>'}",
            f"- supernova_path={install.supernova_path or '<missing>'}",
        ]
    )


def _push_path_if_exists(label: str, path: Path, out: list[str]) -> None:
    if path.exists():
        out.append(f"- {label}={path}")


def get_server_docs() -> str:
    install = detect_install_impl()
    out = [
        "SuperCollider docs pointers:",
        "- note: Prefer local paths and search_supercollider_docs / answer_supercollider_docs (offline). Online entries are optional mirrors of doc.sccode.org.",
    ]

    if install:
        base = Path(install.base_dir)
        help_dir = base / "Help"
        help_source = base / "HelpSource"
        out.append(f"- local_install_dir={install.base_dir}")
        _push_path_if_exists("local_help_dir", help_dir, out)
        _push_path_if_exists("local_help_source_dir", help_source, out)
        _push_path_if_exists(
            "local_server_command_reference_schelp",
            help_source / "Reference" / "Server-Command-Reference.schelp",
            out,
        )
        _push_path_if_exists(
            "local_server_guide_schelp",
            help_source / "Guides" / "Server-Guide.schelp",
            out,
        )
        _push_path_if_exists(
            "local_server_class_schelp",
            help_source / "Classes" / "Server.schelp",
            out,
        )
        _push_path_if_exists(
            "local_server_command_reference_html",
            help_dir / "Reference" / "Server-Command-Reference.html",
            out,
        )
    else:
        out.append("- local_install_dir=<not detected>")

    out.extend(
        [
            "- online_docs_mirror=https://doc.sccode.org/",
            "- online_docs=https://doc.sccode.org/ (alias of online_docs_mirror)",
            "- online_server_command_reference_mirror=https://doc.sccode.org/Reference/Server-Command-Reference.html",
            "- server_command_reference=https://doc.sccode.org/Reference/Server-Command-Reference.html (alias: same URL as online_server_command_reference_mirror)",
        ]
    )
    return "\n".join(out)


def list_server_candidates() -> str:
    try:
        snapshot = collect_snapshot()
    except Exception as exc:
        return f"list_server_candidates failed unexpectedly. Reason: {exc}"

    if not snapshot.candidates:
        return "No SuperCollider-related processes found (scsynth/supernova/sclang/scide)."

    lines = ["SuperCollider process candidates:"]
    for c in snapshot.candidates:
        lines.append(
            f"- pid={c.pid} role={c.role} name={c.name} "
            f"exe={c.exe_path or '<unknown>'} cmdline={c.cmdline or '<empty>'}"
        )
    return "\n".join(lines)


def resolve_control_target(server_pid: int | None, osc_port: int | None) -> tuple[ScServerInstance, int]:
    snapshot = collect_snapshot()

    if osc_port and osc_port > 0:
        srv = next((s for s in snapshot.servers if s.responding_port == osc_port), None)
        if not srv and snapshot.servers:
            srv = snapshot.servers[0]
        if not srv:
            raise ValueError("No SuperCollider audio server found; boot scsynth/supernova first.")
        set_global_supercolider_app_pid(srv.pid)
        set_global_supercolider_active(True)
        return srv, osc_port

    if server_pid is not None:
        srv = next((s for s in snapshot.servers if s.pid == server_pid), None)
        if not srv:
            raise ValueError(f"No server process with pid={server_pid} (scsynth/supernova).")
    else:
        global_pid = get_global_supercolider_app_pid()
        srv = next((s for s in snapshot.servers if global_pid is not None and s.pid == global_pid), None)
        if srv is None:
            srv = next((s for s in snapshot.servers if s.osc_reachable), None)
        if srv is None and snapshot.servers:
            srv = snapshot.servers[0]
        if not srv:
            raise ValueError("No SuperCollider audio server found; boot scsynth/supernova first.")

    if srv.responding_port is None:
        raise ValueError(
            f"Could not resolve OSC UDP port for pid {srv.pid} (no /status.reply on probed ports)."
        )
    set_global_supercolider_app_pid(srv.pid)
    set_global_supercolider_active(True)
    return srv, srv.responding_port


def _execution_warnings_for_code(code: str) -> list[str]:
    text = code.lower()
    warnings: list[str] = []
    if "waitforboot" in text:
        warnings.append("avoid s.waitForBoot in MCP mode; the target server is already expected to be running")
    if ".sleep(" in text:
        warnings.append("avoid s.sleep unless you create a Routine/Task; plain top-level sleep is often ignored or errors")
    if "s.boot" in text or "server.default.boot" in text:
        warnings.append("avoid booting from snippet; use start_supercollider_server() so PID tracking stays consistent")
    if "s.quit" in text or "server.default.quit" in text:
        warnings.append("snippet asks to quit server; prefer quit_supercollider_server() for tracked shutdown")
    return warnings


def execute_supercollider_code(code: str, server_pid: int | None = None, osc_port: int | None = None) -> str:
    if not code.strip():
        return "execute_supercollider_code: code is empty."

    try:
        srv, port = resolve_control_target(server_pid, osc_port)
    except Exception as exc:
        return f"execute_supercollider_code failed: {exc}"

    code_warnings = _execution_warnings_for_code(code)

    install = detect_install_impl()
    if not install:
        return "execute_supercollider_code failed: SuperCollider install not detected."
    if not install.sclang_path:
        return "execute_supercollider_code failed: sclang not found in install."

    workdir = Path(tempfile.mkdtemp(prefix="supercollider-mcp-exec-"))
    try:
        boot = workdir / "bootstrap.sc"
        user_sc = workdir / "user.sc"
        boot.write_text(SCLANG_REMOTE_EXECUTE_BOOTSTRAP, encoding="utf-8")
        user_sc.write_text(code, encoding="utf-8")

        result = _command_output_with_timeout(
            [install.sclang_path, str(boot), str(user_sc), str(port)],
            SCLANG_EXECUTE_TIMEOUT_S,
        )
        if isinstance(result, str):
            warnings_block = ""
            if code_warnings:
                warnings_block = "\n- warnings:\n  - " + "\n  - ".join(code_warnings)
            return (
                f"execute_supercollider_code failed: {result}\n"
                f"- sclang={install.sclang_path}\n"
                f"- target_server_pid={srv.pid}\n"
                f"- GLOBAL_SUPERCOLIDER_APP_PID={get_global_supercolider_app_pid()}\n"
                f"- osc_udp_port={port}\n"
                f"{warnings_block}"
                "- hint: if this timed out, user code may be stuck in an infinite loop, "
                "or the server may be unreachable / refusing extra clients (see scsynth maxLogins)."
            )

        return_code, stdout, stderr = result
        output_joined = f"{stdout}\n{stderr}".lower()
        has_sc_error_markers = (
            "error:" in output_joined
            or "command line parse failed" in output_joined
            or "warning: server 'mcpexec' not running." in output_joined
            or "exception" in output_joined
        )

        if return_code == 0 and not has_sc_error_markers:
            warnings_block = ""
            if code_warnings:
                warnings_block = "\n- warnings:\n  - " + "\n  - ".join(code_warnings)
            return (
                "execute_supercollider_code: OK\n"
                f"- sclang={install.sclang_path}\n"
                f"- target_server_pid={srv.pid}\n"
                f"- GLOBAL_SUPERCOLIDER_APP_PID={get_global_supercolider_app_pid()}\n"
                f"- osc_udp_port={port}\n"
                f"{warnings_block}"
                f"- note: interpreted via Server.remote(127.0.0.1:{port}). "
                "Schedules may outlive this short sclang process.\n"
                "--- stdout (tail) ---\n"
                f"{_tail_lines(stdout, 25)}"
            )
        warnings_block = ""
        if code_warnings:
            warnings_block = "\n- warnings:\n  - " + "\n  - ".join(code_warnings)
        marker_note = "- detected_sc_errors=true\n" if has_sc_error_markers else ""
        return (
            "execute_supercollider_code: ERROR\n"
            f"- sclang={install.sclang_path}\n"
            f"- target_server_pid={srv.pid}\n"
            f"- GLOBAL_SUPERCOLIDER_APP_PID={get_global_supercolider_app_pid()}\n"
            f"- osc_udp_port={port}\n"
            f"{warnings_block}"
            f"{marker_note}"
            f"- exit_code={return_code}\n"
            "--- stderr (tail) ---\n"
            f"{_tail_lines(stderr, 45)}\n"
            "--- stdout (tail) ---\n"
            f"{_tail_lines(stdout, 25)}"
        )
    except Exception as exc:
        return f"execute_supercollider_code failed: {exc}"
    finally:
        shutil.rmtree(workdir, ignore_errors=True)


def _ensure_mcp_tone_synthdef_loaded(port: int) -> str | None:
    if _is_mcp_tone_loaded_for_port(port):
        return None

    install = detect_install_impl()
    if not install:
        return "SuperCollider install not detected while loading mcpTone SynthDef."
    if not install.sclang_path:
        return "sclang not found while loading mcpTone SynthDef."

    script = (
        "(\n"
        "var port, addr, def;\n"
        f"port = {port};\n"
        "addr = NetAddr(\"127.0.0.1\", port);\n"
        "def = SynthDef(\\mcpTone, { |out=0 freq=440 amp=0.15 gate=1|\n"
        "  var env = EnvGen.kr(Env.asr(0.005, 1.0, 0.02), gate, doneAction: 2);\n"
        "  var sig = SinOsc.ar(freq, 0, amp) * env;\n"
        "  Out.ar(out, sig ! 2);\n"
        "});\n"
        "addr.sendMsg(\"/d_recv\", def.asBytes);\n"
        "\"MCP_TONE_DEF_OK\".postln;\n"
        "0.exit;\n"
        ")\n"
    )

    workdir = Path(tempfile.mkdtemp(prefix="supercollider-mcp-tone-def-"))
    try:
        script_path = workdir / "load_mcp_tone.sc"
        script_path.write_text(script, encoding="utf-8")

        result = _command_output_with_timeout([install.sclang_path, str(script_path)], 40)
        if isinstance(result, str):
            return f"timed out loading mcpTone SynthDef: {result}"

        return_code, stdout, stderr = result
        all_out = f"{stdout}\n{stderr}"
        if return_code != 0 or "MCP_TONE_DEF_OK" not in all_out:
            return (
                "failed loading mcpTone SynthDef via sclang\n"
                f"- exit_code={return_code}\n"
                "--- stderr (tail) ---\n"
                f"{_tail_lines(stderr, 40)}\n"
                "--- stdout (tail) ---\n"
                f"{_tail_lines(stdout, 25)}"
            )

        _mark_mcp_tone_loaded_for_port(port)
        return None
    finally:
        shutil.rmtree(workdir, ignore_errors=True)


def start_supercollider_server(port: int = 57110, use_supernova: bool = False) -> str:
    """Spawn scsynth (or supernova) from the detected install and wait for OSC response."""
    # If any reachable server is already running, adopt it instead of spawning another one.
    try:
        snapshot = collect_snapshot()
    except Exception:
        snapshot = None

    if snapshot:
        running = [s for s in snapshot.servers if s.osc_reachable]
        if running:
            chosen = next((s for s in running if s.responding_port == port), None)
            if chosen is None:
                global_pid = get_global_supercolider_app_pid()
                chosen = next((s for s in running if global_pid is not None and s.pid == global_pid), None)
            if chosen is None:
                chosen = running[0]
            set_global_supercolider_app_pid(chosen.pid)
            set_global_supercolider_active(True)
            return (
                "start_supercollider_server: already running (no spawn)\n"
                f"- GLOBAL_SUPERCOLIDER_APP_PID={chosen.pid}\n"
                f"- GLOBAL_SUPERCOLIDER_ACTIVE={get_global_supercolider_active()}\n"
                f"- pid={chosen.pid}\n"
                f"- name={chosen.name}\n"
                f"- osc_udp_port={chosen.responding_port}\n"
                "- note: reusing existing active server process."
            )

    install = detect_install_impl()
    if not install:
        return (
            "start_supercollider_server failed: SuperCollider install not detected. "
            "Run detect_supercollider_install to diagnose."
        )

    if use_supernova and install.supernova_path:
        exe = Path(install.supernova_path)
        kind = "supernova"
    elif install.scsynth_path:
        exe = Path(install.scsynth_path)
        kind = "scsynth"
    elif install.supernova_path:
        exe = Path(install.supernova_path)
        kind = "supernova"
    else:
        return (
            "start_supercollider_server failed: neither scsynth.exe nor supernova.exe "
            f"found under {install.base_dir}."
        )

    if osc_status_alive(port):
        post = collect_snapshot()
        existing = next((s for s in post.servers if s.responding_port == port), None)
        if existing is not None:
            set_global_supercolider_app_pid(existing.pid)
            set_global_supercolider_active(True)
        return (
            f"start_supercollider_server: server already running on port {port} "
            "(OSC /status.reply received). No action taken.\n"
            f"- GLOBAL_SUPERCOLIDER_APP_PID={get_global_supercolider_app_pid()}\n"
            f"- GLOBAL_SUPERCOLIDER_ACTIVE={get_global_supercolider_active()}\n"
            "- hint: call get_servers for full process info, or execute_supercollider_code to play audio."
        )

    try:
        log_dir = Path(tempfile.gettempdir()) / "supercollider-mcp-logs"
        log_dir.mkdir(parents=True, exist_ok=True)
        log_path = log_dir / f"{kind}-{int(time.time())}-{port}.log"
        log_file = log_path.open("a", encoding="utf-8", errors="replace")
        child = subprocess.Popen(
            [str(exe), "-u", str(port)],
            stdout=log_file,
            stderr=log_file,
        )
        new_pid = child.pid
        _set_server_log_path(new_pid, str(log_path))
    except Exception as exc:
        return f"start_supercollider_server failed: could not spawn {exe} -u {port}: {exc}"

    if wait_server_responding(port):
        try:
            post = collect_snapshot()
            chosen = next((s for s in post.servers if s.responding_port == port), None)
            if chosen is not None:
                set_global_supercolider_app_pid(chosen.pid)
                set_global_supercolider_active(True)
        except Exception:
            pass
        return (
            "start_supercollider_server: OK\n"
            f"- kind={kind}\n"
            f"- exe={exe}\n"
            f"- osc_udp_port={port}\n"
            f"- spawned_pid={new_pid}\n"
            f"- server_log={_get_server_log_path(new_pid) or '<unknown>'}\n"
            f"- GLOBAL_SUPERCOLIDER_APP_PID={get_global_supercolider_app_pid()}\n"
            f"- GLOBAL_SUPERCOLIDER_ACTIVE={get_global_supercolider_active()}\n"
            "- /status.reply: yes (server is accepting OSC)\n"
            "- next: call execute_supercollider_code to run sclang/audio on the live server."
        )

    return (
        "start_supercollider_server: PARTIAL\n"
        f"- kind={kind}\n"
        f"- exe={exe}\n"
        f"- osc_udp_port={port}\n"
        f"- spawned_pid={new_pid}\n"
        f"- server_log={_get_server_log_path(new_pid) or '<unknown>'}\n"
        f"- GLOBAL_SUPERCOLIDER_APP_PID={get_global_supercolider_app_pid()}\n"
        f"- GLOBAL_SUPERCOLIDER_ACTIVE={get_global_supercolider_active()}\n"
        f"- /status.reply: no within {SERVER_BOOT_WAIT_MAX_S}s (process spawned but not yet responding)\n"
        "- call ping_supercollider or get_server_status to re-check readiness."
    )


def play_test_tone(
    freq_hz: float = 440.0,
    amp: float = 0.15,
    duration_s: float = 1.0,
    server_pid: int | None = None,
    osc_port: int | None = None,
) -> str:
    if freq_hz <= 0:
        return "play_test_tone failed: freq_hz must be > 0."
    if not (0 < amp <= 1.0):
        return "play_test_tone failed: amp must be in (0, 1]."
    if duration_s <= 0:
        return "play_test_tone failed: duration_s must be > 0."

    try:
        srv, port = resolve_control_target(server_pid, osc_port)
    except Exception as exc:
        return f"play_test_tone failed: {exc}"

    load_err = _ensure_mcp_tone_synthdef_loaded(port)
    if load_err:
        return (
            "play_test_tone failed: could not prepare SynthDef for direct OSC playback.\n"
            f"- target_server_pid={srv.pid}\n"
            f"- GLOBAL_SUPERCOLIDER_APP_PID={get_global_supercolider_app_pid()}\n"
            f"- osc_udp_port={port}\n"
            f"- reason={load_err}"
        )

    node_id = random.randint(20000, 39999)
    before = osc_status_metrics(port)
    before_synth = int(before.get("synth_count", 0)) if before and "synth_count" in before else 0

    # /s_new mcpTone, node_id, addAction=0, target=0 (root group)
    # Retry a few times because /d_recv completion timing can vary on slower boots.
    created = False
    create_errors: list[str] = []
    during: dict[str, float | int | str] | None = None
    for attempt in range(1, 4):
        ok_new = send_osc_message(
            port,
            "/s_new",
            ["mcpTone", node_id, 0, 0, "freq", float(freq_hz), "amp", float(amp)],
        )
        if not ok_new:
            create_errors.append(f"attempt_{attempt}=udp_send_failed")
            continue

        time.sleep(0.08)
        during = osc_status_metrics(port)
        during_synth = int(during.get("synth_count", 0)) if during and "synth_count" in during else 0
        if during_synth > before_synth:
            created = True
            break
        create_errors.append(f"attempt_{attempt}=no_synth_count_increase")

    if not created:
        return (
            "play_test_tone: ERROR\n"
            "- reason=synth_node_not_created\n"
            f"- target_server_pid={srv.pid}\n"
            f"- GLOBAL_SUPERCOLIDER_APP_PID={get_global_supercolider_app_pid()}\n"
            f"- osc_udp_port={port}\n"
            f"- synth_count_before={before.get('synth_count') if before else None}\n"
            f"- synth_count_during={during.get('synth_count') if during else None}\n"
            f"- status_raw_before={before.get('raw_args') if before else None}\n"
            f"- status_raw_during={during.get('raw_args') if during else None}\n"
            f"- attempts={create_errors}\n"
            "- hint: scsynth is reachable but did not instantiate mcpTone; run get_audio_diagnostics() and inspect server_log."
        )

    if during is None:
        during = osc_status_metrics(port)

    # Let it sound, then free explicitly.
    time.sleep(duration_s)
    if not send_osc_message(port, "/n_free", [node_id]):
        return (
            "play_test_tone: PARTIAL\n"
            "- reason=failed_to_free_node\n"
            f"- target_server_pid={srv.pid}\n"
            f"- GLOBAL_SUPERCOLIDER_APP_PID={get_global_supercolider_app_pid()}\n"
            f"- osc_udp_port={port}\n"
            f"- node_id={node_id}\n"
            "- note: tone may keep sounding until stop_supercollider_synths is called."
        )
    time.sleep(0.05)
    after = osc_status_metrics(port)

    synth_before = before.get("synth_count") if before else None
    synth_during = during.get("synth_count") if during else None
    synth_after = after.get("synth_count") if after else None

    return (
        "play_test_tone: OK\n"
        "- mode=direct_osc\n"
        f"- target_server_pid={srv.pid}\n"
        f"- GLOBAL_SUPERCOLIDER_APP_PID={get_global_supercolider_app_pid()}\n"
        f"- osc_udp_port={port}\n"
        f"- node_id={node_id}\n"
        f"- freq_hz={freq_hz}\n"
        f"- amp={amp}\n"
        f"- duration_s={duration_s}\n"
        f"- synth_count_before={synth_before}\n"
        f"- synth_count_during={synth_during}\n"
        f"- synth_count_after={synth_after}\n"
        "- note: this talks directly to scsynth over OSC; IDE window state may not change."
    )


def get_audio_diagnostics(server_pid: int | None = None, osc_port: int | None = None) -> str:
    lines: list[str] = ["SuperCollider audio diagnostics"]
    try:
        snapshot = collect_snapshot()
    except Exception as exc:
        return f"get_audio_diagnostics failed: {exc}"

    lines.append(f"- GLOBAL_SUPERCOLIDER_APP_PID={get_global_supercolider_app_pid()}")
    lines.append(f"- GLOBAL_SUPERCOLIDER_ACTIVE={get_global_supercolider_active()}")
    lines.append(f"- detected_servers={len(snapshot.servers)}")
    lines.append(f"- osc_alive_ports={[p for p, ok in snapshot.osc_port_results.items() if ok]}")

    try:
        srv, port = resolve_control_target(server_pid, osc_port)
    except Exception as exc:
        lines.append(f"- control_target_error={exc}")
        lines.append("- action: call start_supercollider_server(), then retry diagnostics.")
        return "\n".join(lines)

    lines.append(f"- target_pid={srv.pid}")
    lines.append(f"- target_name={srv.name}")
    lines.append(f"- target_port={port}")
    lines.append(f"- target_osc_reachable={srv.osc_reachable}")

    metrics = osc_status_metrics(port)
    if metrics is None:
        lines.append("- status_metrics=<unavailable>")
    else:
        lines.append(f"- synth_count={metrics.get('synth_count')}")
        lines.append(f"- group_count={metrics.get('group_count')}")
        lines.append(f"- avg_cpu={metrics.get('avg_cpu')}")
        lines.append(f"- peak_cpu={metrics.get('peak_cpu')}")
        lines.append(f"- nominal_sr={metrics.get('nominal_sr')}")
        lines.append(f"- actual_sr={metrics.get('actual_sr')}")

    log_path = _get_server_log_path(srv.pid)
    if log_path:
        lines.append(f"- server_log={log_path}")
        lines.append("--- server_log_tail ---")
        lines.append(_tail_file(log_path, 30))
    else:
        lines.append("- server_log=<not tracked> (server may have been launched outside MCP)")

    lines.append(
        "- note: if diagnostics are healthy but no sound is audible, check Windows output device and app volume for scsynth.exe."
    )
    return "\n".join(lines)


def set_supercollider_server_active(server_pid: int | None = None, osc_port: int | None = None) -> str:
    """Ensure an active control target exists. If no server is running, start one."""
    try:
        snapshot = collect_snapshot()
    except Exception as exc:
        return f"set_supercollider_server_active failed: {exc}"

    running = [s for s in snapshot.servers if s.osc_reachable]
    if not running:
        started = start_supercollider_server()
        return (
            "set_supercollider_server_active: no running server found, started one.\n"
            f"{started}"
        )

    chosen = None
    if osc_port is not None and osc_port > 0:
        chosen = next((s for s in running if s.responding_port == osc_port), None)
        if chosen is None:
            return f"set_supercollider_server_active failed: no running server responds on osc_port={osc_port}."
    elif server_pid is not None:
        chosen = next((s for s in running if s.pid == server_pid), None)
        if chosen is None:
            return f"set_supercollider_server_active failed: no running server pid={server_pid}."
    else:
        tracked = get_global_supercolider_app_pid()
        chosen = next((s for s in running if tracked is not None and s.pid == tracked), None)
        if chosen is None:
            chosen = running[0]

    set_global_supercolider_app_pid(chosen.pid)
    set_global_supercolider_active(True)
    return (
        "set_supercollider_server_active: OK\n"
        f"- GLOBAL_SUPERCOLIDER_APP_PID={get_global_supercolider_app_pid()}\n"
        f"- GLOBAL_SUPERCOLIDER_ACTIVE={get_global_supercolider_active()}\n"
        f"- pid={chosen.pid}\n"
        f"- name={chosen.name}\n"
        f"- osc_udp_port={chosen.responding_port}"
    )


def set_supercollider_server_inactive() -> str:
    """Mark MCP control state as inactive without stopping the OS process."""
    prev_pid = get_global_supercolider_app_pid()
    set_global_supercolider_active(False)
    set_global_supercolider_app_pid(None)
    return (
        "set_supercollider_server_inactive: OK\n"
        f"- previous_pid={prev_pid}\n"
        f"- GLOBAL_SUPERCOLIDER_APP_PID={get_global_supercolider_app_pid()}\n"
        f"- GLOBAL_SUPERCOLIDER_ACTIVE={get_global_supercolider_active()}\n"
        "- note: process may still be running; this only clears active control state."
    )


def turn_down_supercollider_server(server_pid: int | None = None, osc_port: int | None = None) -> str:
    """Turn server down (stop it) and mark MCP state inactive."""
    result = quit_supercollider_server(server_pid=server_pid, osc_port=osc_port)
    # Regardless of quit result, move control state to inactive to avoid stale target usage.
    set_global_supercolider_active(False)
    if "quit_supercollider_server: OK" in result:
        set_global_supercolider_app_pid(None)
    return (
        "turn_down_supercollider_server:\n"
        f"{result}\n"
        f"- GLOBAL_SUPERCOLIDER_APP_PID={get_global_supercolider_app_pid()}\n"
        f"- GLOBAL_SUPERCOLIDER_ACTIVE={get_global_supercolider_active()}"
    )


STOP_ALL_SYNTHS_SCLANG = "Server.default.freeAll;"


def stop_supercollider_synths(server_pid: int | None = None, osc_port: int | None = None) -> str:
    inner = execute_supercollider_code(STOP_ALL_SYNTHS_SCLANG, server_pid, osc_port)
    if inner.startswith("execute_supercollider_code: OK"):
        return inner.replace(
            "execute_supercollider_code: OK",
            "stop_supercollider_synths: OK (interpreted Server.default.freeAll)",
            1,
        )
    if inner.startswith("execute_supercollider_code: ERROR"):
        return inner.replace("execute_supercollider_code: ERROR", "stop_supercollider_synths: ERROR", 1)
    if inner.startswith("execute_supercollider_code failed:"):
        return inner.replace("execute_supercollider_code failed:", "stop_supercollider_synths failed:", 1)
    return f"stop_supercollider_synths:\n{inner}"


def quit_supercollider_server(server_pid: int | None = None, osc_port: int | None = None) -> str:
    try:
        srv, port = resolve_control_target(server_pid, osc_port)
    except Exception as exc:
        return f"quit_supercollider_server failed: {exc}"

    if not send_osc_quit(port):
        return f"quit_supercollider_server failed: UDP send /quit to 127.0.0.1:{port} failed"

    gone = wait_process_gone(srv.pid)
    if gone and get_global_supercolider_app_pid() == srv.pid:
        set_global_supercolider_app_pid(None)
    if gone:
        set_global_supercolider_active(False)
    if gone:
        _clear_server_log_path(srv.pid)
    status = "OK" if gone else "PARTIAL (PID still alive or still exiting; check get_servers)"
    return (
        f"quit_supercollider_server: {status}\n"
        f"- sent OSC /quit to 127.0.0.1:{port}\n"
        f"- previous_pid={srv.pid}\n"
        f"- process_exited={gone}\n"
        f"- GLOBAL_SUPERCOLIDER_APP_PID={get_global_supercolider_app_pid()}\n"
        f"- GLOBAL_SUPERCOLIDER_ACTIVE={get_global_supercolider_active()}\n"
        "- note: call start_supercollider_server() to boot a fresh scsynth, "
        "or reboot_supercollider_server() if you want /quit + respawn in one step."
    )


def reboot_supercollider_server(server_pid: int | None = None, osc_port: int | None = None) -> str:
    install = detect_install_impl()
    try:
        srv, port = resolve_control_target(server_pid, osc_port)
    except Exception as exc:
        return f"reboot_supercollider_server failed: {exc}"

    exe = pick_server_exe(srv, install)
    if not exe:
        return (
            "reboot_supercollider_server failed: could not resolve scsynth.exe / "
            "supernova.exe path (exe_path missing and install not detected)."
        )

    if not send_osc_quit(port):
        return f"reboot_supercollider_server failed: UDP send /quit to 127.0.0.1:{port} failed"

    if not wait_process_gone(srv.pid):
        return (
            "reboot_supercollider_server failed: "
            f"pid {srv.pid} still alive after /quit (waited {SERVER_QUIT_WAIT_MAX_S}s). "
            "Try quit_supercollider_server again or end the process manually."
        )

    try:
        log_dir = Path(tempfile.gettempdir()) / "supercollider-mcp-logs"
        log_dir.mkdir(parents=True, exist_ok=True)
        log_path = log_dir / f"reboot-{int(time.time())}-{port}.log"
        log_file = log_path.open("a", encoding="utf-8", errors="replace")
        child = subprocess.Popen([str(exe), "-u", str(port)], stdout=log_file, stderr=log_file)
        new_id = child.pid
        _set_server_log_path(new_id, str(log_path))
    except Exception as exc:
        return f"reboot_supercollider_server failed: spawn {exe} -u {port}: {exc}"

    if wait_server_responding(port):
        set_global_supercolider_app_pid(new_id)
        set_global_supercolider_active(True)
        return (
            "reboot_supercollider_server: OK\n"
            f"- exe={exe}\n"
            f"- osc_udp_port={port}\n"
            f"- previous_pid={srv.pid}\n"
            f"- spawned_pid={new_id}\n"
            f"- server_log={_get_server_log_path(new_id) or '<unknown>'}\n"
            f"- GLOBAL_SUPERCOLIDER_APP_PID={get_global_supercolider_app_pid()}\n"
            f"- GLOBAL_SUPERCOLIDER_ACTIVE={get_global_supercolider_active()}\n"
            "- /status.reply: yes\n"
            "- note: spawned with minimal args (-u <port>). Call execute_supercollider_code to play audio."
        )

    return (
        "reboot_supercollider_server: PARTIAL\n"
        f"- exe={exe}\n"
        f"- osc_udp_port={port}\n"
        f"- previous_pid={srv.pid}\n"
        f"- spawned_pid={new_id}\n"
        f"- /status.reply: no within {SERVER_BOOT_WAIT_MAX_S}s\n"
        "- process may still be starting; check get_servers / ping_supercollider."
    )


def get_supercollider_version() -> str:
    lines: list[str] = []
    install = detect_install_impl()

    if install:
        lines.append(f"SuperCollider install detected at {install.base_dir}")
        v_path = parse_version_from_text(install.base_dir)
        if v_path:
            lines.append(f"- version_from_path={v_path}")
        if install.sclang_path:
            lines.append(f"- sclang_path={install.sclang_path}")
            v_sclang = version_from_sclang_binary(install.sclang_path)
            lines.append(f"- version_from_sclang={v_sclang if v_sclang else '<unavailable>'}")
        if install.scsynth_path:
            lines.append(f"- scsynth_path={install.scsynth_path}")
    else:
        lines.append("SuperCollider install not detected.")

    try:
        snapshot = collect_snapshot()
    except Exception:
        return "\n".join(lines)

    seen = False
    for c in snapshot.candidates:
        if c.role not in {"scsynth", "sclang", "supernova"}:
            continue
        if not c.exe_path:
            continue
        if not seen:
            lines.append("Running binaries:")
            seen = True
        lines.append(f"- pid={c.pid} role={c.role} exe={c.exe_path}")
        v = parse_version_from_text(c.exe_path)
        if v:
            lines.append(f"  version_hint={v}")

    return "\n".join(lines)
