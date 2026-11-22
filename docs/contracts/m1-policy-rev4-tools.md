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

(Regenerated in full for `rg`, `grep`, `egrep`, `fgrep`, `zgrep`, `ggrep`, `rgrep`, `ag`, `ack`, and `find` by `env UPDATE_TOOL_TABLE=1 cargo test -p atrium-core tools`.)

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
