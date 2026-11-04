"""Linux backend shared by the spike measurement harnesses (D-037, docs/spikes/M0-linux-checks.md).

The macOS harnesses read `footprint` and group WebKit's XPC helpers by their "responsible" process.
On Linux, WebKitGTK spawns its web and network processes as descendants of the app (sometimes
under `bwrap`/`xdg-dbus-proxy` when sandboxed), so ownership is the process tree, and the memory
metric is PSS from /proc/<pid>/smaps_rollup: shared pages are split between the processes that map
them, so a sum over the tree counts each page once. GPU buffers aren't in PSS; they are read
separately from the DRM fdinfo of each process and reported apart, never added to PSS.

Everything here reads /proc and a few read-only commands. Nothing is launched.
"""
import glob, os, re, subprocess

CLK_TCK = os.sysconf("SC_CLK_TCK")
WEBKIT_PREFIX = "WebKit"  # WebKitWebProcess, WebKitNetworkProcess, WebKitGPUProcess (exe basenames)


def _read(path):
    try:
        with open(path, errors="replace") as f:
            return f.read()
    except OSError:
        return None


def _q(*cmd):
    try:
        return subprocess.check_output(cmd, text=True, stderr=subprocess.DEVNULL, timeout=10).strip()
    except (OSError, subprocess.SubprocessError):
        return None


# ---------------------------------------------------------------- processes


def parse_stat(text):
    """(ppid, cpu_seconds) from /proc/<pid>/stat. comm may contain spaces and parentheses, so the
    fields are split after the last ')'."""
    rest = text[text.rindex(")") + 2:].split()
    # rest[0] is field 3 (state); ppid is field 4, utime 14, stime 15
    return int(rest[1]), (int(rest[11]) + int(rest[12])) / CLK_TCK


def proc_name(pid):
    """Executable basename from argv[0] (comm is truncated to 15 characters)."""
    raw = _read(f"/proc/{pid}/cmdline")
    if raw:
        return os.path.basename(raw.split("\0", 1)[0])
    comm = _read(f"/proc/{pid}/comm")
    return comm.strip() if comm else "?"


def ps_rows():
    """{pid: (ppid, rss_kb, cpu_seconds, command line)} for every readable process."""
    rows = {}
    for d in os.listdir("/proc"):
        if not d.isdigit():
            continue
        stat = _read(f"/proc/{d}/stat")
        if not stat:
            continue
        try:
            ppid, cpu = parse_stat(stat)
        except (ValueError, IndexError):
            continue
        status = _read(f"/proc/{d}/status") or ""
        m = re.search(r"^VmRSS:\s+(\d+) kB", status, re.M)
        cmd = (_read(f"/proc/{d}/cmdline") or "").replace("\0", " ").strip()
        rows[int(d)] = (ppid, int(m[1]) if m else 0, cpu, cmd)
    return rows


def descendants(rows, root):
    kids = {}
    for pid, (ppid, *_r) in rows.items():
        kids.setdefault(ppid, []).append(pid)
    out, todo = [], [root]
    while todo:
        for c in kids.get(todo.pop(), []):
            out.append(c)
            todo.append(c)
    return out


# ---------------------------------------------------------------- memory


def parse_smaps_rollup(text):
    """kB values: pss, rss, uss (private clean + dirty), swap_pss."""
    v = {k: int(n) for k, n in re.findall(r"^(\w+):\s+(\d+) kB", text, re.M)}
    if "Pss" not in v:
        raise ValueError("no Pss line")
    return {"pss": v["Pss"], "rss": v.get("Rss", 0), "uss": v.get("Private_Clean", 0) + v.get("Private_Dirty", 0), "swap_pss": v.get("SwapPss", 0)}


def mem_kb(pid):
    """PSS/RSS/USS in kB, or None when the process is gone or unreadable."""
    text = _read(f"/proc/{pid}/smaps_rollup")
    if not text:
        return None
    try:
        return parse_smaps_rollup(text)
    except ValueError:
        return None


UNITS = {"": 1 / 1024, "B": 1 / 1024, "KiB": 1, "MiB": 1024, "GiB": 1024 * 1024}


def parse_fdinfo(text):
    """(client key, {"total": kB, "resident": kB}) for a DRM fd, or None for any other fd.
    Sums every memory region (drm-total-system0, drm-total-vram0, …); several fds of one process
    can share a client, so callers deduplicate by the key."""
    if "drm-client-id" not in text:
        return None
    f = dict(re.findall(r"^(drm-[\w-]+):\s+(.*)$", text, re.M))
    key = (f.get("drm-pdev", "?"), f["drm-client-id"].strip())
    res = {"total": 0.0, "resident": 0.0}
    for k, v in f.items():
        for kind in res:
            # memory regions only: xe also has drm-total-cycles-<engine>, a unitless engine counter
            if k.startswith(f"drm-{kind}-") and not k.startswith(f"drm-{kind}-cycles-"):
                m = re.match(r"(\d+)\s*(\w*)", v.strip())
                if m and m[2] in UNITS:
                    res[kind] += int(m[1]) * UNITS[m[2]]
    return key, res


def gpu_kb(pids):
    """GPU memory of these processes from DRM fdinfo, deduplicated by client: {"total", "resident"}
    in kB, or None when no process has a DRM client (e.g. software rendering)."""
    seen, tot = set(), {"total": 0.0, "resident": 0.0}
    for pid in pids:
        for path in glob.glob(f"/proc/{pid}/fdinfo/*"):
            text = _read(path)
            parsed = parse_fdinfo(text) if text else None
            if parsed and parsed[0] not in seen:  # an inherited or passed fd shows the same client
                seen.add(parsed[0])
                for k in tot:
                    tot[k] += parsed[1][k]
    return tot if seen else None


# ---------------------------------------------------------------- host


def parse_display_state(text):
    """Current mode of each monitor from Mutter's DisplayConfig.GetCurrentState (gdbus text)."""
    out = []
    for chunk in text.split("(('")[1:]:
        connector = chunk.split("'", 1)[0]
        m = re.search(r"'(\d+x\d+@[\d.]+)'[^)]*?'is-current': <true>", chunk)
        if m:
            out.append(f"{connector} {m[1]}")
    return out


def monitors():
    text = _q("gdbus", "call", "--session", "--dest", "org.gnome.Mutter.DisplayConfig", "--object-path",
              "/org/gnome/Mutter/DisplayConfig", "--method", "org.gnome.Mutter.DisplayConfig.GetCurrentState")
    return parse_display_state(text) if text else ["unknown (not GNOME/Mutter)"]


def os_release():
    m = re.search(r'^PRETTY_NAME="?([^"\n]*)', _read("/etc/os-release") or "", re.M)
    return m[1] if m else "Linux"


def cpu_model():
    m = re.search(r"^model name\s*:\s*(.*)$", _read("/proc/cpuinfo") or "", re.M)
    return m[1] if m else "unknown"


def on_ac():
    """Mains or USB-C power supplying the machine; peripherals (e.g. a wireless mouse's
    hidpp_battery, type Battery) also expose `online` and must not count."""
    vals = [(_read(os.path.join(d, "online")) or "").strip() for d in glob.glob("/sys/class/power_supply/*")
            if (_read(os.path.join(d, "type")) or "").strip() in ("Mains", "USB")]
    return None if not vals else any(v == "1" for v in vals)


def package_version(name):
    out = _q("pacman", "-Q", name)
    return out.split()[-1] if out else None


def host_info():
    """Everything a Linux measurement must state (D-037): machine, versions, compositor, displays, power."""
    return {
        "os": os_release(),
        "kernel": os.uname().release,
        "cpu": cpu_model(),
        "cores": os.cpu_count(),
        "webkit2gtk": _q("pkg-config", "--modversion", "webkit2gtk-4.1") or package_version("webkit2gtk-4.1"),
        "mesa": package_version("mesa"),
        "session": os.environ.get("XDG_SESSION_TYPE", "?"),
        "desktop": os.environ.get("XDG_CURRENT_DESKTOP", "?"),
        "compositor": _q("gnome-shell", "--version") or "?",
        "monitors": monitors(),
        "power_profile": _q("powerprofilesctl", "get"),
        "on_ac": on_ac(),
        "webkit_env": {k: v for k, v in os.environ.items() if k.startswith("WEBKIT_")},
        "loadavg_at_start": os.getloadavg(),
    }


def host_line(h=None):
    h = h or host_info()
    return (f"{h['os']} {h['kernel']}, {h['cpu']} ({h['cores']} cores), WebKitGTK {h['webkit2gtk']}, Mesa {h['mesa']}, "
            f"{h['session']} {h['desktop']} ({h['compositor']}), monitors [{'; '.join(h['monitors'])}], "
            f"power {h['power_profile']} ac={h['on_ac']}, WEBKIT_* {h['webkit_env'] or 'unset'}")
