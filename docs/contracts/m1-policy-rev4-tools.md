# Policy engine rev 4: normative tool table (annex to §6)

Generated from `crates/atrium-core/src/policy/tools.rs`. Do not edit by hand; regenerate with `env UPDATE_TOOL_TABLE=1 cargo test -p atrium-core tools`.

Any tool or flag absent from this table (and every entry in **Outside the model** below) makes the unit **Unread** (§6).

## Modeled tools

| Tool | Platforms | Flags | Operands | Effects & pinning | Home read-only | Changes default (§11) |
|---|---|---|---|---|---|---|
| `command` | Linux, macOS | none (any flag → Unread) | — | links(root=Y, below=N); wrapper: Command | No | §11 item 10 |
| `exec` | Linux, macOS | none (any flag → Unread) | — | links(root=Y, below=N); wrapper: Exec | No | §11 item 10 |
| `nice` | Linux, macOS | `-n` (Text) | — | links(root=Y, below=N); wrapper: Nice | No | §11 item 10 |
| `timeout` | Linux, macOS | `-s` (Text), `-k` (Text) | [0] Text | links(root=Y, below=N); wrapper: Timeout | No | §11 item 10 |
| `time` | Linux, macOS | none (any flag → Unread) | — | links(root=Y, below=N); wrapper: Time | No | §11 item 10 |
| `env` | Linux, macOS | none (any flag → Unread) | — | links(root=Y, below=N); wrapper: Env | No | §11 item 10 |
| `sh` | Linux, macOS | `-e`, `-u`, `-x`, `-c` (Code) | [0] ProgramFile, rest: Text | writes; links(root=Y, below=N); code: program/inline (pin: FirstOperand); unpinned-prefix-invalid; nested-shell | No | §11 item 2; §11 item 4; §11 item 8 |
| `bash` | Linux, macOS | `-e`, `-u`, `-x`, `-c` (Code) | [0] ProgramFile, rest: Text | writes; links(root=Y, below=N); code: program/inline (pin: FirstOperand); unpinned-prefix-invalid; nested-shell | No | §11 item 2; §11 item 4; §11 item 8 |
| `dash` | Linux, macOS | `-e`, `-u`, `-x`, `-c` (Code) | [0] ProgramFile, rest: Text | writes; links(root=Y, below=N); code: program/inline (pin: FirstOperand); unpinned-prefix-invalid; nested-shell | No | §11 item 2; §11 item 4; §11 item 8 |
| `ls` | Linux, macOS | none (any flag → Unread) | rest: Path | links(root=Y, below=N) | Yes | — |
| `pwd` | Linux, macOS | none (any flag → Unread) | — | links(root=Y, below=N) | Yes | — |
| `cat` | Linux, macOS | none (any flag → Unread) | rest: Path | links(root=Y, below=N) | Yes | — |
| `head` | Linux, macOS | none (any flag → Unread) | rest: Path | links(root=Y, below=N) | Yes | — |
| `tail` | Linux, macOS | none (any flag → Unread) | rest: Path | links(root=Y, below=N) | Yes | — |
| `wc` | Linux, macOS | none (any flag → Unread) | rest: Path | links(root=Y, below=N) | Yes | — |
| `file` | Linux, macOS | none (any flag → Unread) | rest: Path | links(root=Y, below=N) | Yes (forbids -C, --compile, -m, --magic-file, -f, --files-from) | — |
| `stat` | Linux, macOS | none (any flag → Unread) | rest: Path | links(root=N, below=N) | Yes | — |
| `du` | Linux, macOS | none (any flag → Unread) | rest: Path | links(root=Y, below=N) | Yes | — |
| `df` | Linux, macOS | none (any flag → Unread) | rest: Path | links(root=Y, below=N) | Yes | — |
| `which` | Linux, macOS | none (any flag → Unread) | rest: Text | links(root=Y, below=N) | Yes | §11 item 9 |
| `uname` | Linux, macOS | none (any flag → Unread) | — | links(root=Y, below=N) | Yes | — |
| `date` | Linux, macOS | none (any flag → Unread) | rest: Text | links(root=Y, below=N) | Yes (forbids -s, --set, -f, --file) | §11 item 9 |
| `whoami` | Linux, macOS | none (any flag → Unread) | — | links(root=Y, below=N) | Yes | — |
| `tree` | Linux, macOS | `-o/--output` (Path) | rest: Path | writes (flag); links(root=Y, below=N) | Yes (forbids -o, --output, -R) | — |
| `rg` | Linux, macOS | 83 flags (53 bool, 30 value; see flag table below) | [0] Pattern, rest: Path | recursive; links(root=Y, below=N); filter: RgGlob | Yes (forbids --pre, --pre-glob, -z, --search-zip, --hostname-bin, -f, --file) [recursive] | §11 item 9 |
| `grep` | Linux, macOS | 56 flags (40 bool, 16 value; see flag table below) | [0] Pattern, rest: Path | recursive (flag); links(root=Y, below=N); filter: GrepInclude | Yes (forbids -f, --file) [recursive w/ flags] | §11 item 9 |
| `egrep` | Linux, macOS | 56 flags (40 bool, 16 value; see flag table below) | [0] Pattern, rest: Path | recursive (flag); links(root=Y, below=N); filter: GrepInclude | No | §11 item 9 |
| `fgrep` | Linux, macOS | 56 flags (40 bool, 16 value; see flag table below) | [0] Pattern, rest: Path | recursive (flag); links(root=Y, below=N); filter: GrepInclude | No | §11 item 9 |
| `zgrep` | Linux, macOS | 56 flags (40 bool, 16 value; see flag table below) | [0] Pattern, rest: Path | recursive (flag); links(root=Y, below=N); filter: GrepInclude | No | §11 item 9 |
| `ggrep` | Linux, macOS | 56 flags (40 bool, 16 value; see flag table below) | [0] Pattern, rest: Path | recursive (flag); links(root=Y, below=N); filter: GrepInclude | No | §11 item 9 |
| `rgrep` | Linux, macOS | 56 flags (40 bool, 16 value; see flag table below) | [0] Pattern, rest: Path | recursive; links(root=Y, below=N); filter: GrepInclude | No | §11 item 9 |
| `ag` | Linux, macOS | 79 flags (62 bool, 17 value; see flag table below) | [0] Pattern, rest: Path | recursive; links(root=Y, below=N); code: flag | No | §11 item 9 |
| `ack` | Linux, macOS | 41 flags (31 bool, 10 value; see flag table below) | [0] Pattern, rest: Path | recursive; links(root=Y, below=N); code: flag | No | §11 item 9 |
| `git` | Linux, macOS | `-C` (Path), `-c` (Code), `--config-env` (Code), `--exec-path` (Code), `--git-dir` (Path), `--work-tree` (Path), `--namespace` (Text), `--super-prefix` (Text) | — | links(root=Y, below=N); code: flag; unpinned-prefix-invalid | No | §11 item 4 |
| `cargo` | Linux, macOS | none (any flag → Unread) | — | writes; links(root=Y, below=N); code: build (pin: CommandAndSubcommand); unpinned-prefix-invalid | No | §11 item 4; §11 item 8 |
| `pipx` | Linux, macOS | none (any flag → Unread) | — | writes; links(root=Y, below=N); code: subarg run; unpinned-prefix-invalid | No | §11 item 4; §11 item 8 |
| `cd` | Linux, macOS | `-L`, `-P`, `-e`, `-@` | [0] Path | links(root=Y, below=N) | No | — |
| `find` | Linux, macOS | 22 flags (10 bool, 12 value; see flag table below) | rest: Path | writes (flag); recursive; links(root=N, below=N); code: flag; unpinned-prefix-invalid | No | §11 item 4 |
| `ln` | Linux, macOS | `-s` | [0] Path, [1] Path | writes; links(root=N, below=N) | No | — |
| `cp` | Linux, macOS | `-r`, `-R/--recursive`, `-a/--archive`, `-L/--dereference`, `-P`, `-d` | rest: Path, last: DestinationRoot | writes; derived-dest (flag); recursive (flag); links(root=Y, below=N) | No | §11 item 10 |
| `mv` | Linux, macOS | none (any flag → Unread) | rest: Path, last: DestinationRoot | writes; derived-dest (dir src); links(root=N, below=N) | No | §11 item 10 |
| `rsync` | Linux, macOS | `-L/--copy-links`, `-k/--copy-dirlinks`, `--copy-unsafe-links` | rest: Path, last: DestinationRoot | writes; derived-dest; recursive; links(root=Y, below=N) | No | §11 item 10 |
| `tar` | Linux, macOS | `-x`, `-c`, `-r`, `-u`, `-h/--dereference` | rest: ArchiveMember | writes (flag); derived-dest (flag); recursive; links(root=N, below=N) | No | §11 item 9; §11 item 10 |
| `unzip` | Linux, macOS | none (any flag → Unread) | [0] Path, rest: ArchiveMember | writes; derived-dest; links(root=Y, below=N) | No | §11 item 9; §11 item 10 |
| `rm` | Linux, macOS | `-r/--recursive`, `-R` | rest: Path | writes; recursive (flag); links(root=N, below=N) | No | — |
| `tee` | Linux, macOS | none (any flag → Unread) | rest: Path | writes; links(root=Y, below=N) | No | — |
| `touch` | Linux, macOS | none (any flag → Unread) | rest: Path | writes; links(root=Y, below=N) | No | — |
| `chmod` | Linux, macOS | none (any flag → Unread) | [0] Text, rest: Path | writes; links(root=Y, below=N) | No | §11 item 9 |
| `chown` | Linux, macOS | none (any flag → Unread) | [0] Text, rest: Path | writes; links(root=Y, below=N) | No | §11 item 9 |
| `chgrp` | Linux, macOS | none (any flag → Unread) | [0] Text, rest: Path | writes; links(root=Y, below=N) | No | §11 item 9 |
| `truncate` | Linux, macOS | none (any flag → Unread) | rest: Path | writes; links(root=Y, below=N) | No | — |
| `install` | Linux, macOS | none (any flag → Unread) | rest: Path | writes; links(root=Y, below=N) | No | — |
| `mkdir` | Linux, macOS | none (any flag → Unread) | rest: Path | writes; links(root=Y, below=N) | No | — |
| `rmdir` | Linux, macOS | none (any flag → Unread) | rest: Path | writes; links(root=N, below=N) | No | — |
| `unlink` | Linux, macOS | none (any flag → Unread) | rest: Path | writes; links(root=N, below=N) | No | — |
| `shred` | Linux, macOS | none (any flag → Unread) | rest: Path | writes; links(root=Y, below=N) | No | — |
| `patch` | Linux, macOS | none (any flag → Unread) | rest: Path | writes; links(root=Y, below=N) | No | — |
| `wipefs` | Linux | none (any flag → Unread) | rest: Path | writes; links(root=Y, below=N) | No | — |
| `blkdiscard` | Linux | none (any flag → Unread) | rest: Path | writes; links(root=Y, below=N) | No | — |
| `diff` | Linux, macOS | none (any flag → Unread) | rest: Path | links(root=Y, below=N) | No | — |
| `cmp` | Linux, macOS | none (any flag → Unread) | rest: Path | links(root=Y, below=N) | No | — |
| `md5` | macOS | none (any flag → Unread) | rest: Path | links(root=Y, below=N) | No | — |
| `md5sum` | Linux | none (any flag → Unread) | rest: Path | links(root=Y, below=N) | No | — |
| `shasum` | Linux, macOS | none (any flag → Unread) | rest: Path | links(root=Y, below=N) | No | — |
| `sha1sum` | Linux, macOS | none (any flag → Unread) | rest: Path | links(root=Y, below=N) | No | — |
| `sha256sum` | Linux, macOS | none (any flag → Unread) | rest: Path | links(root=Y, below=N) | No | — |
| `readlink` | Linux, macOS | none (any flag → Unread) | rest: Path | links(root=N, below=N) | No | — |
| `realpath` | Linux, macOS | none (any flag → Unread) | rest: Path | links(root=Y, below=Y) | No | — |
| `basename` | Linux, macOS | none (any flag → Unread) | rest: Path | links(root=N, below=N) | No | — |
| `dirname` | Linux, macOS | none (any flag → Unread) | rest: Path | links(root=N, below=N) | No | — |
| `nl` | Linux, macOS | none (any flag → Unread) | rest: Path | links(root=Y, below=N) | No | — |
| `od` | Linux, macOS | none (any flag → Unread) | rest: Path | links(root=Y, below=N) | No | — |
| `hexdump` | Linux, macOS | none (any flag → Unread) | rest: Path | links(root=Y, below=N) | No | — |
| `strings` | Linux, macOS | none (any flag → Unread) | rest: Path | links(root=Y, below=N) | No | — |
| `bat` | Linux, macOS | none (any flag → Unread) | rest: Path | links(root=Y, below=N) | No | — |
| `column` | Linux, macOS | none (any flag → Unread) | rest: Path | links(root=Y, below=N) | No | — |
| `fold` | Linux, macOS | none (any flag → Unread) | rest: Path | links(root=Y, below=N) | No | — |
| `expand` | Linux, macOS | none (any flag → Unread) | rest: Path | links(root=Y, below=N) | No | — |
| `less` | Linux, macOS | none (any flag → Unread) | rest: Path | links(root=Y, below=N) | No | — |
| `more` | Linux, macOS | none (any flag → Unread) | rest: Path | links(root=Y, below=N) | No | — |
| `cut` | Linux, macOS | none (any flag → Unread) | rest: Path | links(root=Y, below=N) | No | — |
| `sort` | Linux, macOS | `-o/--output` (Path) | rest: Path | writes (flag); links(root=Y, below=N) | No | — |
| `uniq` | Linux, macOS | none (any flag → Unread) | [0] Path, [1] Path | writes (>= 2 args); links(root=Y, below=N) | No | — |
| `xxd` | Linux, macOS | none (any flag → Unread) | [0] Path, [1] Path | writes (>= 2 args); links(root=Y, below=N) | No | — |
| `echo` | Linux, macOS | `-n`, `-e`, `-E` | rest: Text | links(root=Y, below=N) | No | §11 item 9 |
| `printf` | Linux, macOS | `-v` (Text) | rest: Text | links(root=Y, below=N) | No | §11 item 9 |
| `whereis` | Linux, macOS | none (any flag → Unread) | rest: Text | links(root=Y, below=N) | No | §11 item 9 |
| `type` | Linux, macOS | none (any flag → Unread) | rest: Text | links(root=Y, below=N) | No | §11 item 9 |
| `tr` | Linux, macOS | none (any flag → Unread) | rest: Text | links(root=Y, below=N) | No | §11 item 9 |
| `true` | Linux, macOS | none (any flag → Unread) | rest: Text | links(root=Y, below=N) | No | — |
| `false` | Linux, macOS | none (any flag → Unread) | rest: Text | links(root=Y, below=N) | No | — |
| `id` | Linux, macOS | none (any flag → Unread) | rest: Text | links(root=Y, below=N) | No | — |
| `jq` | Linux, macOS | none (any flag → Unread) | [0] Pattern, rest: Path | links(root=Y, below=N) | No | §11 item 9 |
| `mdls` | macOS | none (any flag → Unread) | rest: Path | links(root=Y, below=N) | No | — |
| `codesign` | macOS | `-s/--sign` (Text), `-f/--force`, `--remove-signature`, `-d/--display`, `-v/--verify` | rest: Path | writes (flag); links(root=Y, below=N) | No | — |
| `awk` | Linux, macOS | `-i/--include` (Code) | [0] Code, rest: Path | writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 4 |
| `gawk` | Linux, macOS | `-i/--include` (Code) | [0] Code, rest: Path | writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 4 |
| `mawk` | Linux, macOS | `-i/--include` (Code) | [0] Code, rest: Path | writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 4 |
| `sed` | Linux, macOS | `-i/--in-place`, `-e/--expression` (Code) | [0] Code, rest: Path | writes (flag); links(root=Y, below=N); code: flag; unpinned-prefix-invalid | No | §11 item 4 |
| `perl` | Linux, macOS | `-i`, `-e` (Code), `-E` (Code) | [0] ProgramFile, rest: Text | writes; links(root=Y, below=N); code: program/inline (pin: FirstOperand); unpinned-prefix-invalid | No | §11 item 4; §11 item 8 |
| `python` | Linux, macOS | `-c` (Code), `-m` (Code) | [0] ProgramFile, rest: Text | writes; links(root=Y, below=N); code: program/inline (pin: FirstOperand); unpinned-prefix-invalid | No | §11 item 4; §11 item 8 |
| `python3` | Linux, macOS | `-c` (Code), `-m` (Code) | [0] ProgramFile, rest: Text | writes; links(root=Y, below=N); code: program/inline (pin: FirstOperand); unpinned-prefix-invalid | No | §11 item 4; §11 item 8 |
| `node` | Linux, macOS | `-e/--eval` (Code), `-p/--print` (Code) | [0] ProgramFile, rest: Text | writes; links(root=Y, below=N); code: program/inline (pin: FirstOperand); unpinned-prefix-invalid | No | §11 item 4; §11 item 8 |
| `ruby` | Linux, macOS | `-e` (Code) | [0] ProgramFile, rest: Text | writes; links(root=Y, below=N); code: program/inline (pin: FirstOperand); unpinned-prefix-invalid | No | §11 item 4; §11 item 8 |
| `eval` | Linux, macOS | none (any flag → Unread) | rest: Code | writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 4 |
| `source` | Linux, macOS | none (any flag → Unread) | [0] ProgramFile, rest: Text | writes; links(root=Y, below=N); code: program/inline (pin: FirstOperand); unpinned-prefix-invalid | No | §11 item 4; §11 item 8 |
| `.` | Linux, macOS | none (any flag → Unread) | [0] ProgramFile, rest: Text | writes; links(root=Y, below=N); code: program/inline (pin: FirstOperand); unpinned-prefix-invalid | No | §11 item 4; §11 item 8 |
| `npx` | Linux, macOS | none (any flag → Unread) | [0] ProgramFile, rest: Text | writes; links(root=Y, below=N); code: program/inline (pin: FirstOperand); unpinned-prefix-invalid | No | §11 item 4; §11 item 8 |
| `xargs` | Linux, macOS | none (any flag → Unread) | rest: Code | writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 4 |
| `ssh` | Linux, macOS | none (any flag → Unread) | rest: Text | writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 4 |
| `mosh` | Linux, macOS | none (any flag → Unread) | rest: Text | writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 4 |
| `autossh` | Linux, macOS | none (any flag → Unread) | rest: Text | writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 4 |
| `nsenter` | Linux | none (any flag → Unread) | rest: Text | writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 4 |
| `unshare` | Linux | none (any flag → Unread) | rest: Text | writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 4 |
| `docker` | Linux, macOS | none (any flag → Unread) | rest: Text | writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 4 |
| `podman` | Linux, macOS | none (any flag → Unread) | rest: Text | writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 4 |
| `nerdctl` | Linux, macOS | none (any flag → Unread) | rest: Text | writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 4 |
| `kubectl` | Linux, macOS | none (any flag → Unread) | rest: Text | writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 4 |
| `oc` | Linux, macOS | none (any flag → Unread) | rest: Text | writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 4 |
| `lxc` | Linux, macOS | none (any flag → Unread) | rest: Text | writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 4 |
| `incus` | Linux, macOS | none (any flag → Unread) | rest: Text | writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 4 |
| `machinectl` | Linux | none (any flag → Unread) | rest: Text | writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 4 |
| `systemd-run` | Linux | none (any flag → Unread) | rest: Text | writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 4 |
| `systemd-nspawn` | Linux | none (any flag → Unread) | rest: Text | writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 4 |
| `firejail` | Linux | none (any flag → Unread) | rest: Text | writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 4 |
| `bwrap` | Linux | none (any flag → Unread) | rest: Text | writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 4 |
| `flatpak-spawn` | Linux | none (any flag → Unread) | rest: Text | writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 4 |
| `toolbox` | Linux, macOS | none (any flag → Unread) | rest: Text | writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 4 |
| `distrobox` | Linux, macOS | none (any flag → Unread) | rest: Text | writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 4 |
| `distrobox-enter` | Linux, macOS | none (any flag → Unread) | rest: Text | writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 4 |
| `multipass` | Linux, macOS | none (any flag → Unread) | rest: Text | writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 4 |
| `vagrant` | Linux, macOS | none (any flag → Unread) | rest: Text | writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 4 |
| `limactl` | Linux, macOS | none (any flag → Unread) | rest: Text | writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 4 |
| `colima` | Linux, macOS | none (any flag → Unread) | rest: Text | writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 4 |
| `runuser` | Linux | none (any flag → Unread) | rest: Text | writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 4 |
| `setpriv` | Linux | none (any flag → Unread) | rest: Text | writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 4 |
| `parallel` | Linux, macOS | none (any flag → Unread) | rest: Text | writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 4 |
| `sudo` | Linux, macOS | none (any flag → Unread) | rest: Code | writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid; hard-deny: `privilege-escalation` | No | — |
| `sudoedit` | Linux, macOS | none (any flag → Unread) | rest: Path | writes; links(root=Y, below=N); hard-deny: `privilege-escalation` | No | — |
| `su` | Linux, macOS | none (any flag → Unread) | rest: Code | writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid; hard-deny: `privilege-escalation` | No | — |
| `doas` | Linux, macOS | none (any flag → Unread) | rest: Code | writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid; hard-deny: `privilege-escalation` | No | — |
| `pkexec` | Linux | none (any flag → Unread) | rest: Code | writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid; hard-deny: `privilege-escalation` | No | — |
| `run0` | Linux | none (any flag → Unread) | rest: Code | writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid; hard-deny: `privilege-escalation` | No | — |
| `curl` | Linux, macOS | none (any flag → Unread) | rest: Path | writes; links(root=Y, below=N); downloader | No | — |
| `wget` | Linux, macOS | none (any flag → Unread) | rest: Path | writes; links(root=Y, below=N); downloader | No | — |
| `fetch` | Linux, macOS | none (any flag → Unread) | rest: Path | writes; links(root=Y, below=N); downloader | No | — |

## Subcommands

### `git`

| Subcommand | Flags | Operands | Effects & pinning | Home read-only | Changes default (§11) |
|---|---|---|---|---|---|
| `git status` | none (any flag → Unread) | rest: Path | links(root=Y, below=N) | Yes (forbids --output, --ext-diff, --exec-path, --config-env, -c) | §11 item 10 |
| `git log` | `-p`, `-L/--line-range` (Text) | rest: Ref | links(root=Y, below=N); git-cred-select | Yes (forbids --output, --ext-diff, --exec-path, --config-env, -c) | §11 item 9; §11 item 10 |
| `git diff` | `--output` (Path), `--ext-diff` | rest: Ref | writes (flag); links(root=Y, below=N); code: flag; git-cred-select | Yes (forbids --output, --ext-diff, --exec-path, --config-env, -c) | §11 item 9; §11 item 10 |
| `git show` | none (any flag → Unread) | rest: Ref | links(root=Y, below=N); git-cred-select | Yes (forbids --output, --ext-diff, --exec-path, --config-env, -c) | §11 item 9; §11 item 10 |
| `git branch` | `-a/--all`, `-r/--remotes`, `-v/--verbose`, `-l/--list`, `--show-current` | rest: Ref | links(root=Y, below=N) | Yes (forbids --output, --ext-diff, --exec-path, --config-env, -c) (only -a, --all, -r, --remotes, -v, -vv, --verbose, --list, -l, --show-current) | §11 item 9 |
| `git rev-parse` | none (any flag → Unread) | rest: Ref | links(root=Y, below=N) | Yes (forbids --output, --ext-diff, --exec-path, --config-env, -c) | §11 item 9 |
| `git ls-files` | none (any flag → Unread) | rest: Path | links(root=Y, below=N) | Yes (forbids --output, --ext-diff, --exec-path, --config-env, -c) | §11 item 10 |
| `git cat-file` | none (any flag → Unread) | rest: Ref | links(root=Y, below=N); git-cred-select | No | §11 item 9 |
| `git push` | `-f/--force`, `--force-with-lease`, `--force-if-includes`, `--mirror`, `--all`, `--tags`, `--branches`, `--prune`, `-o/--push-option` (Text), `--receive-pack` (Code), `--exec` (Code) | rest: Ref | writes; links(root=Y, below=N); code: flag | No | §11 item 9 |
| `git clone` | none (any flag → Unread) | [0] Ref, [1] DestinationRoot | writes; derived-dest; links(root=Y, below=N) | No | §11 item 10 |
| `git checkout` | none (any flag → Unread) | rest: Ref | writes; derived-dest; links(root=Y, below=N) | No | §11 item 10 |
| `git merge-index` | none (any flag → Unread) | rest: Code | writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 4 |
| `git filter-branch` | none (any flag → Unread) | rest: Code | writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 4 |
| `git bisect` | none (any flag → Unread) | rest: Text | writes; links(root=Y, below=N); code: subarg run; unpinned-prefix-invalid | No | §11 item 4 |
| `git rebase` | `-x/--exec` (Code) | rest: Ref | writes; links(root=Y, below=N); code: flag; unpinned-prefix-invalid | No | §11 item 4 |
| `git submodule` | none (any flag → Unread) | rest: Text | writes; links(root=Y, below=N); code: subarg foreach; unpinned-prefix-invalid | No | §11 item 4 |
| `git difftool` | `-x/--tool-cmd` (Code) | rest: Ref | links(root=Y, below=N); code: flag; unpinned-prefix-invalid | No | §11 item 4 |
| `git mergetool` | `-x/--tool-cmd` (Code) | rest: Path | writes; links(root=Y, below=N); code: flag; unpinned-prefix-invalid | No | §11 item 4 |

### `cargo`

| Subcommand | Flags | Operands | Effects & pinning | Home read-only | Changes default (§11) |
|---|---|---|---|---|---|
| `cargo test` | none (any flag → Unread) | rest: Text | writes; links(root=Y, below=N); code: build (pin: CommandAndSubcommand) | No | §11 item 8 |
| `cargo build` | none (any flag → Unread) | rest: Text | writes; links(root=Y, below=N); code: build (pin: CommandAndSubcommand) | No | §11 item 8 |
| `cargo run` | none (any flag → Unread) | rest: Text | writes; links(root=Y, below=N); code: build (pin: CommandAndSubcommand) | No | §11 item 8 |
| `cargo check` | none (any flag → Unread) | rest: Text | writes; links(root=Y, below=N); code: build (pin: CommandAndSubcommand) | No | §11 item 8 |
| `cargo bench` | none (any flag → Unread) | rest: Text | writes; links(root=Y, below=N); code: build (pin: CommandAndSubcommand) | No | §11 item 8 |
| `cargo doc` | none (any flag → Unread) | rest: Text | writes; links(root=Y, below=N); code: build (pin: CommandAndSubcommand) | No | §11 item 8 |
| `cargo clippy` | none (any flag → Unread) | rest: Text | writes; links(root=Y, below=N); code: build (pin: CommandAndSubcommand) | No | §11 item 8 |

### `pipx`

| Subcommand | Flags | Operands | Effects & pinning | Home read-only | Changes default (§11) |
|---|---|---|---|---|---|
| `pipx run` | none (any flag → Unread) | [0] ProgramFile, rest: Text | writes; links(root=Y, below=N); code: program/inline (pin: FirstOperand); unpinned-prefix-invalid | No | §11 item 4; §11 item 8 |

## Detailed flag tables

### `rg` (83 flags)

| Short | Long | Kind | Effects |
|---|---|---|---|
| `-i` | `--ignore-case` | Bool | — |
| `-S` | `--smart-case` | Bool | — |
| `-s` | `--case-sensitive` | Bool | — |
| `-n` | `--line-number` | Bool | — |
| `-N` | `--no-line-number` | Bool | — |
| `-H` | `--with-filename` | Bool | — |
| `-I` | `--no-filename` | Bool | — |
| `-l` | `--files-with-matches` | Bool | — |
| `-c` | `--count` | Bool | — |
| `-o` | `--only-matching` | Bool | — |
| `-v` | `--invert-match` | Bool | — |
| `-w` | `--word-regexp` | Bool | — |
| `-x` | `--line-regexp` | Bool | — |
| `-u` | `--unrestricted` | Bool | — |
| `-a` | `--text` | Bool | — |
| `-U` | `--multiline` | Bool | — |
| `-F` | `--fixed-strings` | Bool | — |
| `-P` | `--pcre2` | Bool | — |
| `-q` | `--quiet` | Bool | — |
| `-0` | `--null` | Bool | — |
| `-p` | `--pretty` | Bool | — |
| `-b` | `--byte-offset` | Bool | — |
| `-L` | `--follow` | Bool | links(root=Y, below=Y) |
| — | `--hidden` | Bool | — |
| — | `--no-ignore` | Bool | — |
| — | `--no-ignore-vcs` | Bool | — |
| — | `--no-ignore-dot` | Bool | — |
| — | `--no-ignore-parent` | Bool | — |
| — | `--no-ignore-global` | Bool | — |
| — | `--no-ignore-files` | Bool | — |
| — | `--no-messages` | Bool | — |
| — | `--no-heading` | Bool | — |
| — | `--heading` | Bool | — |
| — | `--count-matches` | Bool | — |
| — | `--files-without-match` | Bool | — |
| — | `--multiline-dotall` | Bool | — |
| — | `--binary` | Bool | — |
| — | `--files` | Bool | provides-pattern |
| — | `--type-list` | Bool | provides-pattern |
| — | `--json` | Bool | — |
| — | `--column` | Bool | — |
| — | `--no-column` | Bool | — |
| — | `--vimgrep` | Bool | — |
| — | `--trim` | Bool | — |
| — | `--stats` | Bool | — |
| — | `--no-config` | Bool | — |
| — | `--version` | Bool | — |
| — | `--help` | Bool | — |
| — | `--no-pcre2` | Bool | — |
| — | `--crlf` | Bool | — |
| — | `--include-zero` | Bool | — |
| — | `--glob-case-insensitive` | Bool | — |
| — | `--no-glob-case-insensitive` | Bool | — |
| `-e` | `--regexp` | Value(Pattern) | provides-pattern |
| `-g` | `--glob` | Value(Pattern) | filter(RgGlob, ci=false) |
| — | `--iglob` | Value(Pattern) | filter(RgGlob, ci=true) |
| `-f` | `--file` | Value(Path) | provides-pattern |
| `-t` | `--type` | Value(Text) | — |
| `-T` | `--type-not` | Value(Text) | — |
| `-m` | `--max-count` | Value(Text) | — |
| `-A` | `--after-context` | Value(Text) | — |
| `-B` | `--before-context` | Value(Text) | — |
| `-C` | `--context` | Value(Text) | — |
| `-d` | `--max-depth` | Value(Text) | — |
| — | `--maxdepth` | Value(Text) | — |
| `-j` | `--threads` | Value(Text) | — |
| `-M` | `--max-columns` | Value(Text) | — |
| `-E` | `--encoding` | Value(Text) | — |
| `-r` | `--replace` | Value(Text) | — |
| — | `--colors` | Value(Text) | — |
| — | `--color` | Value(Text) | — |
| — | `--sort` | Value(Text) | — |
| — | `--sortr` | Value(Text) | — |
| — | `--max-filesize` | Value(Text) | — |
| — | `--path-separator` | Value(Text) | — |
| — | `--engine` | Value(Text) | — |
| — | `--context-separator` | Value(Text) | — |
| — | `--type-add` | Value(Text) | — |
| — | `--type-clear` | Value(Text) | — |
| — | `--field-context-separator` | Value(Text) | — |
| — | `--field-match-separator` | Value(Text) | — |
| — | `--dfa-size-limit` | Value(Text) | — |
| — | `--regex-size-limit` | Value(Text) | — |

### `grep` (56 flags)

| Short | Long | Kind | Effects |
|---|---|---|---|
| `-i` | `--ignore-case` | Bool | — |
| — | `--no-ignore-case` | Bool | — |
| `-v` | `--invert-match` | Bool | — |
| `-n` | `--line-number` | Bool | — |
| `-c` | `--count` | Bool | — |
| `-l` | `--files-with-matches` | Bool | — |
| `-L` | `--files-without-match` | Bool | — |
| `-o` | `--only-matching` | Bool | — |
| `-q` | `--quiet` | Bool | — |
| — | `--silent` | Bool | — |
| `-s` | `--no-messages` | Bool | — |
| `-w` | `--word-regexp` | Bool | — |
| `-x` | `--line-regexp` | Bool | — |
| `-h` | `--no-filename` | Bool | — |
| `-H` | `--with-filename` | Bool | — |
| `-E` | `--extended-regexp` | Bool | — |
| `-F` | `--fixed-strings` | Bool | — |
| `-G` | `--basic-regexp` | Bool | — |
| `-P` | `--perl-regexp` | Bool | — |
| `-a` | `--text` | Bool | — |
| `-z` | `--null-data` | Bool | — |
| `-Z` | `--null` | Bool | — |
| `-b` | `--byte-offset` | Bool | — |
| `-T` | `--initial-tab` | Bool | — |
| `-r` | `--recursive` | Bool | recursive |
| `-R` | `--dereference-recursive` | Bool | recursive; links(root=Y, below=Y) |
| — | `--no-group-separator` | Bool | — |
| `-I` | — | Bool | — |
| `-U` | — | Bool | — |
| `-y` | — | Bool | — |
| `-0` | — | Bool | — |
| `-1` | — | Bool | — |
| `-2` | — | Bool | — |
| `-3` | — | Bool | — |
| `-4` | — | Bool | — |
| `-5` | — | Bool | — |
| `-6` | — | Bool | — |
| `-7` | — | Bool | — |
| `-8` | — | Bool | — |
| `-9` | — | Bool | — |
| `-e` | `--regexp` | Value(Pattern) | provides-pattern |
| `-f` | `--file` | Value(Path) | provides-pattern |
| `-m` | `--max-count` | Value(Text) | — |
| `-A` | `--after-context` | Value(Text) | — |
| `-B` | `--before-context` | Value(Text) | — |
| `-C` | `--context` | Value(Text) | — |
| `-d` | `--directories` | Value(Text) | recursive |
| `-D` | `--devices` | Value(Text) | — |
| — | `--include` | Value(Pattern) | filter(GrepInclude, ci=false) |
| — | `--exclude` | Value(Pattern) | — |
| — | `--exclude-dir` | Value(Pattern) | — |
| — | `--label` | Value(Text) | — |
| — | `--color` | Value(Text) | — |
| — | `--colour` | Value(Text) | — |
| — | `--binary-files` | Value(Text) | — |
| — | `--group-separator` | Value(Text) | — |

### `egrep` (56 flags)

| Short | Long | Kind | Effects |
|---|---|---|---|
| `-i` | `--ignore-case` | Bool | — |
| — | `--no-ignore-case` | Bool | — |
| `-v` | `--invert-match` | Bool | — |
| `-n` | `--line-number` | Bool | — |
| `-c` | `--count` | Bool | — |
| `-l` | `--files-with-matches` | Bool | — |
| `-L` | `--files-without-match` | Bool | — |
| `-o` | `--only-matching` | Bool | — |
| `-q` | `--quiet` | Bool | — |
| — | `--silent` | Bool | — |
| `-s` | `--no-messages` | Bool | — |
| `-w` | `--word-regexp` | Bool | — |
| `-x` | `--line-regexp` | Bool | — |
| `-h` | `--no-filename` | Bool | — |
| `-H` | `--with-filename` | Bool | — |
| `-E` | `--extended-regexp` | Bool | — |
| `-F` | `--fixed-strings` | Bool | — |
| `-G` | `--basic-regexp` | Bool | — |
| `-P` | `--perl-regexp` | Bool | — |
| `-a` | `--text` | Bool | — |
| `-z` | `--null-data` | Bool | — |
| `-Z` | `--null` | Bool | — |
| `-b` | `--byte-offset` | Bool | — |
| `-T` | `--initial-tab` | Bool | — |
| `-r` | `--recursive` | Bool | recursive |
| `-R` | `--dereference-recursive` | Bool | recursive; links(root=Y, below=Y) |
| — | `--no-group-separator` | Bool | — |
| `-I` | — | Bool | — |
| `-U` | — | Bool | — |
| `-y` | — | Bool | — |
| `-0` | — | Bool | — |
| `-1` | — | Bool | — |
| `-2` | — | Bool | — |
| `-3` | — | Bool | — |
| `-4` | — | Bool | — |
| `-5` | — | Bool | — |
| `-6` | — | Bool | — |
| `-7` | — | Bool | — |
| `-8` | — | Bool | — |
| `-9` | — | Bool | — |
| `-e` | `--regexp` | Value(Pattern) | provides-pattern |
| `-f` | `--file` | Value(Path) | provides-pattern |
| `-m` | `--max-count` | Value(Text) | — |
| `-A` | `--after-context` | Value(Text) | — |
| `-B` | `--before-context` | Value(Text) | — |
| `-C` | `--context` | Value(Text) | — |
| `-d` | `--directories` | Value(Text) | recursive |
| `-D` | `--devices` | Value(Text) | — |
| — | `--include` | Value(Pattern) | filter(GrepInclude, ci=false) |
| — | `--exclude` | Value(Pattern) | — |
| — | `--exclude-dir` | Value(Pattern) | — |
| — | `--label` | Value(Text) | — |
| — | `--color` | Value(Text) | — |
| — | `--colour` | Value(Text) | — |
| — | `--binary-files` | Value(Text) | — |
| — | `--group-separator` | Value(Text) | — |

### `fgrep` (56 flags)

| Short | Long | Kind | Effects |
|---|---|---|---|
| `-i` | `--ignore-case` | Bool | — |
| — | `--no-ignore-case` | Bool | — |
| `-v` | `--invert-match` | Bool | — |
| `-n` | `--line-number` | Bool | — |
| `-c` | `--count` | Bool | — |
| `-l` | `--files-with-matches` | Bool | — |
| `-L` | `--files-without-match` | Bool | — |
| `-o` | `--only-matching` | Bool | — |
| `-q` | `--quiet` | Bool | — |
| — | `--silent` | Bool | — |
| `-s` | `--no-messages` | Bool | — |
| `-w` | `--word-regexp` | Bool | — |
| `-x` | `--line-regexp` | Bool | — |
| `-h` | `--no-filename` | Bool | — |
| `-H` | `--with-filename` | Bool | — |
| `-E` | `--extended-regexp` | Bool | — |
| `-F` | `--fixed-strings` | Bool | — |
| `-G` | `--basic-regexp` | Bool | — |
| `-P` | `--perl-regexp` | Bool | — |
| `-a` | `--text` | Bool | — |
| `-z` | `--null-data` | Bool | — |
| `-Z` | `--null` | Bool | — |
| `-b` | `--byte-offset` | Bool | — |
| `-T` | `--initial-tab` | Bool | — |
| `-r` | `--recursive` | Bool | recursive |
| `-R` | `--dereference-recursive` | Bool | recursive; links(root=Y, below=Y) |
| — | `--no-group-separator` | Bool | — |
| `-I` | — | Bool | — |
| `-U` | — | Bool | — |
| `-y` | — | Bool | — |
| `-0` | — | Bool | — |
| `-1` | — | Bool | — |
| `-2` | — | Bool | — |
| `-3` | — | Bool | — |
| `-4` | — | Bool | — |
| `-5` | — | Bool | — |
| `-6` | — | Bool | — |
| `-7` | — | Bool | — |
| `-8` | — | Bool | — |
| `-9` | — | Bool | — |
| `-e` | `--regexp` | Value(Pattern) | provides-pattern |
| `-f` | `--file` | Value(Path) | provides-pattern |
| `-m` | `--max-count` | Value(Text) | — |
| `-A` | `--after-context` | Value(Text) | — |
| `-B` | `--before-context` | Value(Text) | — |
| `-C` | `--context` | Value(Text) | — |
| `-d` | `--directories` | Value(Text) | recursive |
| `-D` | `--devices` | Value(Text) | — |
| — | `--include` | Value(Pattern) | filter(GrepInclude, ci=false) |
| — | `--exclude` | Value(Pattern) | — |
| — | `--exclude-dir` | Value(Pattern) | — |
| — | `--label` | Value(Text) | — |
| — | `--color` | Value(Text) | — |
| — | `--colour` | Value(Text) | — |
| — | `--binary-files` | Value(Text) | — |
| — | `--group-separator` | Value(Text) | — |

### `zgrep` (56 flags)

| Short | Long | Kind | Effects |
|---|---|---|---|
| `-i` | `--ignore-case` | Bool | — |
| — | `--no-ignore-case` | Bool | — |
| `-v` | `--invert-match` | Bool | — |
| `-n` | `--line-number` | Bool | — |
| `-c` | `--count` | Bool | — |
| `-l` | `--files-with-matches` | Bool | — |
| `-L` | `--files-without-match` | Bool | — |
| `-o` | `--only-matching` | Bool | — |
| `-q` | `--quiet` | Bool | — |
| — | `--silent` | Bool | — |
| `-s` | `--no-messages` | Bool | — |
| `-w` | `--word-regexp` | Bool | — |
| `-x` | `--line-regexp` | Bool | — |
| `-h` | `--no-filename` | Bool | — |
| `-H` | `--with-filename` | Bool | — |
| `-E` | `--extended-regexp` | Bool | — |
| `-F` | `--fixed-strings` | Bool | — |
| `-G` | `--basic-regexp` | Bool | — |
| `-P` | `--perl-regexp` | Bool | — |
| `-a` | `--text` | Bool | — |
| `-z` | `--null-data` | Bool | — |
| `-Z` | `--null` | Bool | — |
| `-b` | `--byte-offset` | Bool | — |
| `-T` | `--initial-tab` | Bool | — |
| `-r` | `--recursive` | Bool | recursive |
| `-R` | `--dereference-recursive` | Bool | recursive; links(root=Y, below=Y) |
| — | `--no-group-separator` | Bool | — |
| `-I` | — | Bool | — |
| `-U` | — | Bool | — |
| `-y` | — | Bool | — |
| `-0` | — | Bool | — |
| `-1` | — | Bool | — |
| `-2` | — | Bool | — |
| `-3` | — | Bool | — |
| `-4` | — | Bool | — |
| `-5` | — | Bool | — |
| `-6` | — | Bool | — |
| `-7` | — | Bool | — |
| `-8` | — | Bool | — |
| `-9` | — | Bool | — |
| `-e` | `--regexp` | Value(Pattern) | provides-pattern |
| `-f` | `--file` | Value(Path) | provides-pattern |
| `-m` | `--max-count` | Value(Text) | — |
| `-A` | `--after-context` | Value(Text) | — |
| `-B` | `--before-context` | Value(Text) | — |
| `-C` | `--context` | Value(Text) | — |
| `-d` | `--directories` | Value(Text) | recursive |
| `-D` | `--devices` | Value(Text) | — |
| — | `--include` | Value(Pattern) | filter(GrepInclude, ci=false) |
| — | `--exclude` | Value(Pattern) | — |
| — | `--exclude-dir` | Value(Pattern) | — |
| — | `--label` | Value(Text) | — |
| — | `--color` | Value(Text) | — |
| — | `--colour` | Value(Text) | — |
| — | `--binary-files` | Value(Text) | — |
| — | `--group-separator` | Value(Text) | — |

### `ggrep` (56 flags)

| Short | Long | Kind | Effects |
|---|---|---|---|
| `-i` | `--ignore-case` | Bool | — |
| — | `--no-ignore-case` | Bool | — |
| `-v` | `--invert-match` | Bool | — |
| `-n` | `--line-number` | Bool | — |
| `-c` | `--count` | Bool | — |
| `-l` | `--files-with-matches` | Bool | — |
| `-L` | `--files-without-match` | Bool | — |
| `-o` | `--only-matching` | Bool | — |
| `-q` | `--quiet` | Bool | — |
| — | `--silent` | Bool | — |
| `-s` | `--no-messages` | Bool | — |
| `-w` | `--word-regexp` | Bool | — |
| `-x` | `--line-regexp` | Bool | — |
| `-h` | `--no-filename` | Bool | — |
| `-H` | `--with-filename` | Bool | — |
| `-E` | `--extended-regexp` | Bool | — |
| `-F` | `--fixed-strings` | Bool | — |
| `-G` | `--basic-regexp` | Bool | — |
| `-P` | `--perl-regexp` | Bool | — |
| `-a` | `--text` | Bool | — |
| `-z` | `--null-data` | Bool | — |
| `-Z` | `--null` | Bool | — |
| `-b` | `--byte-offset` | Bool | — |
| `-T` | `--initial-tab` | Bool | — |
| `-r` | `--recursive` | Bool | recursive |
| `-R` | `--dereference-recursive` | Bool | recursive; links(root=Y, below=Y) |
| — | `--no-group-separator` | Bool | — |
| `-I` | — | Bool | — |
| `-U` | — | Bool | — |
| `-y` | — | Bool | — |
| `-0` | — | Bool | — |
| `-1` | — | Bool | — |
| `-2` | — | Bool | — |
| `-3` | — | Bool | — |
| `-4` | — | Bool | — |
| `-5` | — | Bool | — |
| `-6` | — | Bool | — |
| `-7` | — | Bool | — |
| `-8` | — | Bool | — |
| `-9` | — | Bool | — |
| `-e` | `--regexp` | Value(Pattern) | provides-pattern |
| `-f` | `--file` | Value(Path) | provides-pattern |
| `-m` | `--max-count` | Value(Text) | — |
| `-A` | `--after-context` | Value(Text) | — |
| `-B` | `--before-context` | Value(Text) | — |
| `-C` | `--context` | Value(Text) | — |
| `-d` | `--directories` | Value(Text) | recursive |
| `-D` | `--devices` | Value(Text) | — |
| — | `--include` | Value(Pattern) | filter(GrepInclude, ci=false) |
| — | `--exclude` | Value(Pattern) | — |
| — | `--exclude-dir` | Value(Pattern) | — |
| — | `--label` | Value(Text) | — |
| — | `--color` | Value(Text) | — |
| — | `--colour` | Value(Text) | — |
| — | `--binary-files` | Value(Text) | — |
| — | `--group-separator` | Value(Text) | — |

### `rgrep` (56 flags)

| Short | Long | Kind | Effects |
|---|---|---|---|
| `-i` | `--ignore-case` | Bool | — |
| — | `--no-ignore-case` | Bool | — |
| `-v` | `--invert-match` | Bool | — |
| `-n` | `--line-number` | Bool | — |
| `-c` | `--count` | Bool | — |
| `-l` | `--files-with-matches` | Bool | — |
| `-L` | `--files-without-match` | Bool | — |
| `-o` | `--only-matching` | Bool | — |
| `-q` | `--quiet` | Bool | — |
| — | `--silent` | Bool | — |
| `-s` | `--no-messages` | Bool | — |
| `-w` | `--word-regexp` | Bool | — |
| `-x` | `--line-regexp` | Bool | — |
| `-h` | `--no-filename` | Bool | — |
| `-H` | `--with-filename` | Bool | — |
| `-E` | `--extended-regexp` | Bool | — |
| `-F` | `--fixed-strings` | Bool | — |
| `-G` | `--basic-regexp` | Bool | — |
| `-P` | `--perl-regexp` | Bool | — |
| `-a` | `--text` | Bool | — |
| `-z` | `--null-data` | Bool | — |
| `-Z` | `--null` | Bool | — |
| `-b` | `--byte-offset` | Bool | — |
| `-T` | `--initial-tab` | Bool | — |
| `-r` | `--recursive` | Bool | recursive |
| `-R` | `--dereference-recursive` | Bool | recursive; links(root=Y, below=Y) |
| — | `--no-group-separator` | Bool | — |
| `-I` | — | Bool | — |
| `-U` | — | Bool | — |
| `-y` | — | Bool | — |
| `-0` | — | Bool | — |
| `-1` | — | Bool | — |
| `-2` | — | Bool | — |
| `-3` | — | Bool | — |
| `-4` | — | Bool | — |
| `-5` | — | Bool | — |
| `-6` | — | Bool | — |
| `-7` | — | Bool | — |
| `-8` | — | Bool | — |
| `-9` | — | Bool | — |
| `-e` | `--regexp` | Value(Pattern) | provides-pattern |
| `-f` | `--file` | Value(Path) | provides-pattern |
| `-m` | `--max-count` | Value(Text) | — |
| `-A` | `--after-context` | Value(Text) | — |
| `-B` | `--before-context` | Value(Text) | — |
| `-C` | `--context` | Value(Text) | — |
| `-d` | `--directories` | Value(Text) | recursive |
| `-D` | `--devices` | Value(Text) | — |
| — | `--include` | Value(Pattern) | filter(GrepInclude, ci=false) |
| — | `--exclude` | Value(Pattern) | — |
| — | `--exclude-dir` | Value(Pattern) | — |
| — | `--label` | Value(Text) | — |
| — | `--color` | Value(Text) | — |
| — | `--colour` | Value(Text) | — |
| — | `--binary-files` | Value(Text) | — |
| — | `--group-separator` | Value(Text) | — |

### `ag` (79 flags)

| Short | Long | Kind | Effects |
|---|---|---|---|
| `-a` | `--all-types` | Bool | — |
| `-D` | `--debug` | Bool | — |
| `-c` | `--count` | Bool | — |
| `-F` | `--fixed-strings` | Bool | — |
| `-f` | `--follow` | Bool | links(root=Y, below=Y) |
| `-H` | `--heading` | Bool | — |
| `-h` | — | Bool | — |
| `-i` | `--ignore-case` | Bool | — |
| `-L` | `--files-without-matches` | Bool | — |
| `-l` | `--files-with-matches` | Bool | — |
| `-n` | `--norecurse` | Bool | — |
| `-o` | `--only-matching` | Bool | — |
| `-Q` | `--literal` | Bool | — |
| `-R` | `--recurse` | Bool | — |
| `-r` | — | Bool | — |
| `-S` | `--smart-case` | Bool | — |
| `-s` | `--case-sensitive` | Bool | — |
| `-v` | `--invert-match` | Bool | — |
| `-V` | — | Bool | — |
| `-t` | `--all-text` | Bool | — |
| `-u` | `--unrestricted` | Bool | — |
| `-U` | `--skip-vcs-ignores` | Bool | — |
| `-w` | `--word-regexp` | Bool | — |
| `-z` | `--search-zip` | Bool | — |
| `-0` | `--null` | Bool | — |
| — | `--ackmate` | Bool | — |
| — | `--affinity` | Bool | — |
| — | `--noaffinity` | Bool | — |
| — | `--hidden` | Bool | — |
| — | `--column` | Bool | — |
| — | `--nocolor` | Bool | — |
| — | `--color` | Bool | — |
| — | `--color-win-ansi` | Bool | — |
| — | `--noheading` | Bool | — |
| — | `--nobreak` | Bool | — |
| — | `--break` | Bool | — |
| — | `--nofollow` | Bool | — |
| — | `--search-binary` | Bool | — |
| — | `--silent` | Bool | — |
| — | `--stats` | Bool | — |
| — | `--stats-only` | Bool | — |
| — | `--vimgrep` | Bool | — |
| — | `--print0` | Bool | — |
| — | `--one-device` | Bool | — |
| — | `--filename` | Bool | — |
| — | `--nofilename` | Bool | — |
| — | `--numbers` | Bool | — |
| — | `--nonumbers` | Bool | — |
| — | `--line-numbers` | Bool | — |
| — | `--group` | Bool | — |
| — | `--nogroup` | Bool | — |
| — | `--match` | Bool | — |
| — | `--mmap` | Bool | — |
| — | `--nommap` | Bool | — |
| — | `--multiline` | Bool | — |
| — | `--nomultiline` | Bool | — |
| — | `--nopager` | Bool | — |
| — | `--parallel` | Bool | — |
| — | `--passthrough` | Bool | — |
| — | `--passthru` | Bool | — |
| — | `--print-all-files` | Bool | — |
| — | `--print-long-lines` | Bool | — |
| `-G` | `--file-search-regex` | Value(Pattern) | — |
| `-g` | `--filename-pattern` | Value(Pattern) | provides-pattern |
| `-m` | `--max-count` | Value(Text) | — |
| `-p` | `--path-to-ignore` | Value(Path) | — |
| `-W` | `--width` | Value(Text) | — |
| — | `--ackmate-dir-filter` | Value(Pattern) | — |
| — | `--depth` | Value(Text) | — |
| — | `--ignore` | Value(Pattern) | — |
| — | `--ignore-dir` | Value(Pattern) | — |
| — | `--pager` | Value(Code) | runs-code |
| — | `--workers` | Value(Text) | — |
| — | `--color-line-number` | Value(Text) | — |
| — | `--color-match` | Value(Text) | — |
| — | `--color-path` | Value(Text) | — |
| `-A` | `--after` | OptionalValue(Text) | — |
| `-B` | `--before` | OptionalValue(Text) | — |
| `-C` | `--context` | OptionalValue(Text) | — |

### `ack` (41 flags)

| Short | Long | Kind | Effects |
|---|---|---|---|
| `-i` | `--ignore-case` | Bool | — |
| `-I` | — | Bool | — |
| `-S` | `--smart-case` | Bool | — |
| `-v` | `--invert-match` | Bool | — |
| `-w` | `--word-regexp` | Bool | — |
| `-Q` | `--literal` | Bool | — |
| `-l` | `--files-with-matches` | Bool | — |
| `-L` | `--files-without-matches` | Bool | — |
| `-c` | `--count` | Bool | — |
| `-h` | `--no-filename` | Bool | — |
| `-H` | `--with-filename` | Bool | — |
| `-o` | — | Bool | — |
| `-1` | — | Bool | — |
| `-n` | `--no-recurse` | Bool | — |
| `-r` | `--recurse` | Bool | — |
| `-R` | — | Bool | — |
| `-s` | — | Bool | — |
| `-x` | — | Bool | — |
| `-f` | — | Bool | provides-pattern |
| `-k` | — | Bool | — |
| — | `--column` | Bool | — |
| — | `--nocolor` | Bool | — |
| — | `--color` | Bool | — |
| — | `--noheading` | Bool | — |
| — | `--heading` | Bool | — |
| — | `--nobreak` | Bool | — |
| — | `--break` | Bool | — |
| — | `--follow` | Bool | links(root=Y, below=Y) |
| — | `--nofollow` | Bool | — |
| — | `--sort-files` | Bool | — |
| — | `--nopager` | Bool | — |
| `-A` | `--after-context` | Value(Text) | — |
| `-B` | `--before-context` | Value(Text) | — |
| `-m` | `--max-count` | Value(Text) | — |
| `-g` | — | Value(Pattern) | provides-pattern |
| `-t` | `--type` | Value(Text) | — |
| — | `--ignore-dir` | Value(Pattern) | — |
| — | `--noignore-dir` | Value(Pattern) | — |
| — | `--match` | Value(Pattern) | provides-pattern |
| — | `--output` | Value(Code) | runs-code |
| `-C` | `--context` | OptionalValue(Text) | — |

### `find` (22 flags)

| Short | Long | Kind | Effects |
|---|---|---|---|
| `-H` | — | Bool | links(root=Y, below=N) |
| `-L` | `--follow` | Bool | links(root=Y, below=Y) |
| `-P` | — | Bool | — |
| `-E` | — | Bool | — |
| `-X` | — | Bool | — |
| `-d` | — | Bool | — |
| `-s` | — | Bool | — |
| `-x` | — | Bool | — |
| `-D` | — | Value(Text) | — |
| `-f` | — | Value(Path) | — |
| — | `--delete` | Bool | writes |
| — | `--depth` | Bool | — |
| — | `--maxdepth` | Value(Text) | — |
| — | `--mindepth` | Value(Text) | — |
| — | `--exec` | Value(Code) | writes; runs-code |
| — | `--execdir` | Value(Code) | writes; runs-code |
| — | `--ok` | Value(Code) | writes; runs-code |
| — | `--okdir` | Value(Code) | writes; runs-code |
| — | `--fprint` | Value(Path) | writes |
| — | `--fprint0` | Value(Path) | writes |
| — | `--fprintf` | Value(Path) | writes |
| — | `--fls` | Value(Path) | writes |

## Outside the model (`OUTSIDE_MODEL`)

| Name | Platforms | Unpinned prefix invalid | Contract | Changes default (§11) | Reason |
|---|---|---|---|---|---|
| `zsh` | Linux, macOS | Yes | §4 | §11 item 4 | zsh glob qualifiers and `=cmd` run code; outside §4's nested-shell model |
| `ksh` | Linux, macOS | Yes | §4 | §11 item 4 | Shell outside §4's modeled sh/bash/dash subset; makes the unit Unread |
| `fish` | Linux, macOS | Yes | §4 | §11 item 4 | Shell outside §4's modeled sh/bash/dash subset; makes the unit Unread |
| `ash` | Linux, macOS | Yes | §4 | §11 item 4 | Shell outside §4's modeled sh/bash/dash subset; makes the unit Unread |
| `csh` | Linux, macOS | Yes | §4 | §11 item 4 | Shell outside §4's modeled sh/bash/dash subset; makes the unit Unread |
| `tcsh` | Linux, macOS | Yes | §4 | §11 item 4 | Shell outside §4's modeled sh/bash/dash subset; makes the unit Unread |
| `mksh` | Linux, macOS | Yes | §4 | §11 item 4 | Shell outside §4's modeled sh/bash/dash subset; makes the unit Unread |
| `yash` | Linux, macOS | Yes | §4 | §11 item 4 | Shell outside §4's modeled sh/bash/dash subset; makes the unit Unread |
| `rbash` | Linux, macOS | Yes | §4 | §11 item 4 | Shell outside §4's modeled sh/bash/dash subset; makes the unit Unread |
| `posh` | Linux, macOS | Yes | §4 | §11 item 4 | Shell outside §4's modeled sh/bash/dash subset; makes the unit Unread |
| `elvish` | Linux, macOS | Yes | §4 | §11 item 4 | Shell outside §4's modeled sh/bash/dash subset; makes the unit Unread |
| `xonsh` | Linux, macOS | Yes | §4 | §11 item 4 | Shell outside §4's modeled sh/bash/dash subset; makes the unit Unread |
| `nohup` | Linux, macOS | Yes | §4 | §11 item 10 | Wrapper outside §4's closed list; makes the unit Unread |
| `builtin` | Linux, macOS | Yes | §4 | §11 item 10 | Wrapper outside §4's closed list; makes the unit Unread |
| `busybox` | Linux | Yes | §4 | §11 item 10 | Wrapper outside §4's closed list; makes the unit Unread |
| `toybox` | Linux | Yes | §4 | §11 item 10 | Wrapper outside §4's closed list; makes the unit Unread |
| `setsid` | Linux | Yes | §4 | §11 item 10 | Wrapper outside §4's closed list; makes the unit Unread |
| `unbuffer` | Linux, macOS | Yes | §4 | §11 item 10 | Wrapper outside §4's closed list; makes the unit Unread |
| `caffeinate` | macOS | Yes | §4 | §11 item 10 | Wrapper outside §4's closed list; makes the unit Unread |
| `sandbox-exec` | macOS | Yes | §4 | §11 item 10 | Wrapper outside §4's closed list; makes the unit Unread |
| `chroot` | Linux, macOS | Yes | §4 | §11 item 10 | Wrapper outside §4's closed list; makes the unit Unread |
| `ionice` | Linux | Yes | §4 | §11 item 10 | Wrapper outside §4's closed list; makes the unit Unread |
| `stdbuf` | Linux, macOS | Yes | §4 | §11 item 10 | Wrapper outside §4's closed list; makes the unit Unread |
| `chrt` | Linux | Yes | §4 | §11 item 10 | Wrapper outside §4's closed list; makes the unit Unread |
| `taskset` | Linux | Yes | §4 | §11 item 10 | Wrapper outside §4's closed list; makes the unit Unread |
| `flock` | Linux | Yes | §4 | §11 item 10 | Wrapper outside §4's closed list; makes the unit Unread |
| `script` | Linux, macOS | Yes | §4 | §11 item 10 | Wrapper outside §4's closed list; makes the unit Unread |
| `arch` | macOS | Yes | §4 | §11 item 10 | Wrapper outside §4's closed list; makes the unit Unread |
| `if` | Linux, macOS | No | §3 | — | Shell reserved word or compound construct outside §3's simple-command model |
| `then` | Linux, macOS | No | §3 | — | Shell reserved word or compound construct outside §3's simple-command model |
| `else` | Linux, macOS | No | §3 | — | Shell reserved word or compound construct outside §3's simple-command model |
| `elif` | Linux, macOS | No | §3 | — | Shell reserved word or compound construct outside §3's simple-command model |
| `fi` | Linux, macOS | No | §3 | — | Shell reserved word or compound construct outside §3's simple-command model |
| `for` | Linux, macOS | No | §3 | — | Shell reserved word or compound construct outside §3's simple-command model |
| `while` | Linux, macOS | No | §3 | — | Shell reserved word or compound construct outside §3's simple-command model |
| `until` | Linux, macOS | No | §3 | — | Shell reserved word or compound construct outside §3's simple-command model |
| `do` | Linux, macOS | No | §3 | — | Shell reserved word or compound construct outside §3's simple-command model |
| `done` | Linux, macOS | No | §3 | — | Shell reserved word or compound construct outside §3's simple-command model |
| `case` | Linux, macOS | No | §3 | — | Shell reserved word or compound construct outside §3's simple-command model |
| `esac` | Linux, macOS | No | §3 | — | Shell reserved word or compound construct outside §3's simple-command model |
| `function` | Linux, macOS | No | §3 | — | Shell reserved word or compound construct outside §3's simple-command model |
| `select` | Linux, macOS | No | §3 | — | Shell reserved word or compound construct outside §3's simple-command model |
| `{` | Linux, macOS | No | §3 | — | Shell reserved word or compound construct outside §3's simple-command model |
| `}` | Linux, macOS | No | §3 | — | Shell reserved word or compound construct outside §3's simple-command model |
| `!` | Linux, macOS | No | §3 | — | Shell reserved word or compound construct outside §3's simple-command model |
| `[[` | Linux, macOS | No | §3 | — | Shell reserved word or compound construct outside §3's simple-command model |
| `coproc` | Linux, macOS | No | §3 | — | Shell reserved word or compound construct outside §3's simple-command model |
| `ed` | Linux, macOS | Yes | §4 | §11 item 4 | Interactive line editor capable of shell escapes (`!cmd`); outside the model |
| `ex` | Linux, macOS | Yes | §4 | §11 item 4 | Interactive line editor capable of shell escapes (`!cmd`); outside the model |
| `pushd` | Linux, macOS | No | §5 | — | Mutates shell directory stack; makes cwd unknown (§5) |
| `popd` | Linux, macOS | No | §5 | — | Mutates shell directory stack; makes cwd unknown (§5) |
| `test` | Linux, macOS | No | §6 | — | Conditional primaries (`-f`, `-r`, …) have no flag table in `dev` |
| `[` | Linux, macOS | No | §6 | — | Conditional primaries (`-f`, `-r`, …) have no flag table in `dev` |
| `open` | macOS | Yes | §6 | §11 item 4 | macOS launcher that opens files/URLs with external applications |
| `spctl` | macOS | No | §6 | — | macOS Gatekeeper policy tool with modifying flags; no flag table in `dev` |
| `mdfind` | macOS | No | §6 | — | macOS Spotlight search across volumes; no flag table in `dev` |

## Summary of rows changing `dev` defaults (§11)

| Entry | §11 item(s) |
|---|---|
| `command` | §11 item 10 |
| `exec` | §11 item 10 |
| `nice` | §11 item 10 |
| `timeout` | §11 item 10 |
| `time` | §11 item 10 |
| `env` | §11 item 10 |
| `sh` | §11 item 2; §11 item 4; §11 item 8 |
| `bash` | §11 item 2; §11 item 4; §11 item 8 |
| `dash` | §11 item 2; §11 item 4; §11 item 8 |
| `which` | §11 item 9 |
| `date` | §11 item 9 |
| `rg` | §11 item 9 |
| `grep` | §11 item 9 |
| `egrep` | §11 item 9 |
| `fgrep` | §11 item 9 |
| `zgrep` | §11 item 9 |
| `ggrep` | §11 item 9 |
| `rgrep` | §11 item 9 |
| `ag` | §11 item 9 |
| `ack` | §11 item 9 |
| `git` | §11 item 4 |
| `git status` | §11 item 10 |
| `git log` | §11 item 9; §11 item 10 |
| `git diff` | §11 item 9; §11 item 10 |
| `git show` | §11 item 9; §11 item 10 |
| `git branch` | §11 item 9 |
| `git rev-parse` | §11 item 9 |
| `git ls-files` | §11 item 10 |
| `git cat-file` | §11 item 9 |
| `git push` | §11 item 9 |
| `git clone` | §11 item 10 |
| `git checkout` | §11 item 10 |
| `git merge-index` | §11 item 4 |
| `git filter-branch` | §11 item 4 |
| `git bisect` | §11 item 4 |
| `git rebase` | §11 item 4 |
| `git submodule` | §11 item 4 |
| `git difftool` | §11 item 4 |
| `git mergetool` | §11 item 4 |
| `cargo` | §11 item 4; §11 item 8 |
| `cargo test` | §11 item 8 |
| `cargo build` | §11 item 8 |
| `cargo run` | §11 item 8 |
| `cargo check` | §11 item 8 |
| `cargo bench` | §11 item 8 |
| `cargo doc` | §11 item 8 |
| `cargo clippy` | §11 item 8 |
| `pipx` | §11 item 4; §11 item 8 |
| `pipx run` | §11 item 4; §11 item 8 |
| `find` | §11 item 4 |
| `cp` | §11 item 10 |
| `mv` | §11 item 10 |
| `rsync` | §11 item 10 |
| `tar` | §11 item 9; §11 item 10 |
| `unzip` | §11 item 9; §11 item 10 |
| `chmod` | §11 item 9 |
| `chown` | §11 item 9 |
| `chgrp` | §11 item 9 |
| `echo` | §11 item 9 |
| `printf` | §11 item 9 |
| `whereis` | §11 item 9 |
| `type` | §11 item 9 |
| `tr` | §11 item 9 |
| `jq` | §11 item 9 |
| `awk` | §11 item 4 |
| `gawk` | §11 item 4 |
| `mawk` | §11 item 4 |
| `sed` | §11 item 4 |
| `perl` | §11 item 4; §11 item 8 |
| `python` | §11 item 4; §11 item 8 |
| `python3` | §11 item 4; §11 item 8 |
| `node` | §11 item 4; §11 item 8 |
| `ruby` | §11 item 4; §11 item 8 |
| `eval` | §11 item 4 |
| `source` | §11 item 4; §11 item 8 |
| `.` | §11 item 4; §11 item 8 |
| `npx` | §11 item 4; §11 item 8 |
| `xargs` | §11 item 4 |
| `ssh` | §11 item 4 |
| `mosh` | §11 item 4 |
| `autossh` | §11 item 4 |
| `nsenter` | §11 item 4 |
| `unshare` | §11 item 4 |
| `docker` | §11 item 4 |
| `podman` | §11 item 4 |
| `nerdctl` | §11 item 4 |
| `kubectl` | §11 item 4 |
| `oc` | §11 item 4 |
| `lxc` | §11 item 4 |
| `incus` | §11 item 4 |
| `machinectl` | §11 item 4 |
| `systemd-run` | §11 item 4 |
| `systemd-nspawn` | §11 item 4 |
| `firejail` | §11 item 4 |
| `bwrap` | §11 item 4 |
| `flatpak-spawn` | §11 item 4 |
| `toolbox` | §11 item 4 |
| `distrobox` | §11 item 4 |
| `distrobox-enter` | §11 item 4 |
| `multipass` | §11 item 4 |
| `vagrant` | §11 item 4 |
| `limactl` | §11 item 4 |
| `colima` | §11 item 4 |
| `runuser` | §11 item 4 |
| `setpriv` | §11 item 4 |
| `parallel` | §11 item 4 |
| `zsh` (outside model) | §11 item 4 |
| `ksh` (outside model) | §11 item 4 |
| `fish` (outside model) | §11 item 4 |
| `ash` (outside model) | §11 item 4 |
| `csh` (outside model) | §11 item 4 |
| `tcsh` (outside model) | §11 item 4 |
| `mksh` (outside model) | §11 item 4 |
| `yash` (outside model) | §11 item 4 |
| `rbash` (outside model) | §11 item 4 |
| `posh` (outside model) | §11 item 4 |
| `elvish` (outside model) | §11 item 4 |
| `xonsh` (outside model) | §11 item 4 |
| `nohup` (outside model) | §11 item 10 |
| `builtin` (outside model) | §11 item 10 |
| `busybox` (outside model) | §11 item 10 |
| `toybox` (outside model) | §11 item 10 |
| `setsid` (outside model) | §11 item 10 |
| `unbuffer` (outside model) | §11 item 10 |
| `caffeinate` (outside model) | §11 item 10 |
| `sandbox-exec` (outside model) | §11 item 10 |
| `chroot` (outside model) | §11 item 10 |
| `ionice` (outside model) | §11 item 10 |
| `stdbuf` (outside model) | §11 item 10 |
| `chrt` (outside model) | §11 item 10 |
| `taskset` (outside model) | §11 item 10 |
| `flock` (outside model) | §11 item 10 |
| `script` (outside model) | §11 item 10 |
| `arch` (outside model) | §11 item 10 |
| `ed` (outside model) | §11 item 4 |
| `ex` (outside model) | §11 item 4 |
| `open` (outside model) | §11 item 4 |
