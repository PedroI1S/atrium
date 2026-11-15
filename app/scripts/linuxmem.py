"""Linux memory of the app and its WebKit processes, as D-040 budgets it (L2).

D-040: on Linux the idle budget applies to **private PSS**: PSS without shared-library pages
(anonymous memory plus private file pages), summed over the app and its WebKit processes. Full
PSS, RSS and GPU memory (DRM fdinfo) are reported beside it, not budgeted.

Read from `/proc/<pid>/smaps`, per mapping:
  - a shared library (`*.so`, `*.so.N…`): its clean pages are the library's and are left out;
    its dirty pages (relocations, `.data` written by this process: anonymous copy-on-write
    pages) are the process's own and count (`lib_dirty`);
  - any other file (the app's executable, fonts, caches, `locale-archive`): its PSS counts
    (`file`);
  - shared memory (`memfd:`, `/dev/shm`, SysV): its PSS counts (`shmem`); PSS splits it between
    the processes that map it, so the sum over the tree counts it once;
  - a GPU device mapping (`/dev/dri/…`): its PSS counts too (`gpu_mapped`, usually 0: GPU
    buffers live in DRM, below);
  - everything else (heap, stacks, anonymous mappings, `[anon:…]`): counts (`anon`).
So `private_pss = pss - lib_clean`. The stricter reading of M0's §2.5 table (every page of a
library mapping left out) is `private_pss - lib_dirty`; both are reported.

GPU memory comes from each process's DRM fdinfo, summed over memory regions and deduplicated by
client (an fd passed between processes shows the same client), never added to PSS. Code adapted
from the M0 spike's `spikes/linuxproc.py` (same project).

Pure functions take text so they are tested on fixtures (`test_linuxmem.py`).
"""

import glob
import os
import re

MAPPING = re.compile(r"^[0-9a-f]+-[0-9a-f]+ \S+ \S+ \S+ \S+\s*(.*)$")
FIELD = re.compile(r"^(\w+):\s+(\d+) kB$")
LIBRARY = re.compile(r"\.so(\.[0-9][\w.]*)?$")
KEYS = ("rss", "pss", "private_pss", "uss", "anon", "lib_dirty", "lib_clean", "file", "shmem", "gpu_mapped")


def kind_of(name: str) -> str:
    """The mapping's kind from its smaps name: lib, file, shmem, gpu or anon."""
    name = name.removesuffix(" (deleted)").strip()
    if name.startswith("/memfd:") or name.startswith("/dev/shm/") or name.startswith("/SYSV"):
        return "shmem"
    if name.startswith("/dev/dri/"):
        return "gpu"
    if name.startswith("/"):
        return "lib" if LIBRARY.search(os.path.basename(name)) else "file"
    return "anon"


def parse_smaps(text: str, by_name: dict | None = None) -> dict:
    """kB per kind for one process's smaps text; see the module doc. With `by_name`, also adds
    each mapping's counted PSS (what goes into `private_pss`) under its name."""
    out = dict.fromkeys(KEYS, 0)
    name, kind, fields = "", "anon", {}

    def close() -> None:
        if not fields:
            return
        pss, dirty = fields.get("Pss", 0), fields.get("Pss_Dirty", 0)
        out["rss"] += fields.get("Rss", 0)
        out["pss"] += pss
        out["uss"] += fields.get("Private_Clean", 0) + fields.get("Private_Dirty", 0)
        if kind == "lib":
            out["lib_dirty"] += dirty
            out["lib_clean"] += pss - dirty
            counted = dirty
        else:
            out["gpu_mapped" if kind == "gpu" else kind] += pss
            counted = pss
        if by_name is not None and counted:
            key = name.removesuffix(" (deleted)").strip() or "[anonymous]"
            by_name[key] = by_name.get(key, 0) + counted

    for line in text.splitlines():
        head = MAPPING.match(line)
        if head:
            close()
            name, fields = head.group(1), {}
            kind = kind_of(name)
            continue
        field = FIELD.match(line)
        if field:
            fields[field.group(1)] = int(field.group(2))
    close()
    out["private_pss"] = out["pss"] - out["lib_clean"]
    return out


def smaps_kb(pid: int, by_name: dict | None = None) -> dict | None:
    """`parse_smaps` of a live process, or None when it is gone or unreadable."""
    try:
        with open(f"/proc/{pid}/smaps", errors="replace") as f:
            return parse_smaps(f.read(), by_name)
    except OSError:
        return None


UNITS = {"": 1 / 1024, "B": 1 / 1024, "KiB": 1, "MiB": 1024, "GiB": 1024 * 1024}


def parse_fdinfo(text: str):
    """(client key, {"total": kB, "resident": kB}) for a DRM fd, or None for any other fd.
    Memory regions only: xe also has `drm-total-cycles-<engine>`, an engine counter (M0 L5)."""
    if "drm-client-id" not in text:
        return None
    f = dict(re.findall(r"^(drm-[\w-]+):\s+(.*)$", text, re.M))
    key = (f.get("drm-pdev", "?"), f["drm-client-id"].strip())
    res = {"total": 0.0, "resident": 0.0}
    for k, v in f.items():
        for kind in res:
            if k.startswith(f"drm-{kind}-") and not k.startswith(f"drm-{kind}-cycles-"):
                m = re.match(r"(\d+)\s*(\w*)", v.strip())
                if m and m[2] in UNITS:
                    res[kind] += int(m[1]) * UNITS[m[2]]
    return key, res


def gpu_kb(pids) -> dict | None:
    """GPU memory of these processes from DRM fdinfo, deduplicated by client, in kB; None when
    no process has a DRM client (software rendering)."""
    seen, tot = set(), {"total": 0.0, "resident": 0.0}
    for pid in pids:
        for path in glob.glob(f"/proc/{pid}/fdinfo/*"):
            try:
                with open(path, errors="replace") as f:
                    parsed = parse_fdinfo(f.read())
            except OSError:
                continue
            if parsed and parsed[0] not in seen:
                seen.add(parsed[0])
                for k in tot:
                    tot[k] += parsed[1][k]
    return tot if seen else None


def parse_ppid(stat: str) -> int:
    """The parent pid from /proc/<pid>/stat text (comm may hold spaces and parentheses)."""
    return int(stat[stat.rindex(")") + 2:].split()[1])


def children_map() -> dict[int, list[int]]:
    kids: dict[int, list[int]] = {}
    for d in os.listdir("/proc"):
        if not d.isdigit():
            continue
        try:
            with open(f"/proc/{d}/stat") as f:
                kids.setdefault(parse_ppid(f.read()), []).append(int(d))
        except (OSError, ValueError, IndexError):
            continue
    return kids


def exe_name(pid: int) -> str:
    try:
        with open(f"/proc/{pid}/cmdline", "rb") as f:
            return os.path.basename(f.read().split(b"\0", 1)[0].decode(errors="replace"))
    except OSError:
        return "?"


def app_tree(root: int, skip=("atriumd",)) -> list[int]:
    """The app and its descendants (WebKit's web and network processes, their `bwrap` and
    `xdg-dbus-proxy` helpers), without the daemon the app started and the daemon's children."""
    kids, out, todo = children_map(), [root], [root]
    while todo:
        for child in kids.get(todo.pop(), []):
            if exe_name(child) in skip:
                continue
            out.append(child)
            todo.append(child)
    return out


def role_of(name: str) -> str:
    """A process's row in the breakdown."""
    if name.startswith("WebKitWebProcess"):
        return "web"
    if name.startswith("WebKitNetworkProcess"):
        return "network"
    if name.startswith("WebKitGPUProcess"):
        return "gpu-process"
    if name in ("atrium", "atrium-base"):
        return "ui"
    return "helpers"


def sample(root: int) -> dict:
    """One sample: per-kind kB summed over the app's tree, per-role private PSS, GPU memory."""
    total = dict.fromkeys(KEYS, 0)
    roles: dict[str, dict] = {}
    pids = app_tree(root)
    for pid in pids:
        mem = smaps_kb(pid)
        if mem is None:
            continue
        role = roles.setdefault(role_of(exe_name(pid)), dict.fromkeys(KEYS, 0) | {"count": 0})
        role["count"] += 1
        for k in KEYS:
            total[k] += mem[k]
            role[k] += mem[k]
    gpu = gpu_kb(pids)
    return {"total": total, "roles": roles, "gpu": gpu, "processes": len(pids)}


def mib(kb: float) -> float:
    return kb / 1024
