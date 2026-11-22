# Policy engine rev 4: normative tool table (annex to §6)

Generated from `crates/atrium-core/src/policy/tools.rs`. Do not edit by hand; regenerate with `env UPDATE_TOOL_TABLE=1 cargo test -p atrium-core tools`.

**Unknown tools vs. excluded forms (§4, §6):**
- An **unknown tool** (absent from both **Modeled tools** and **Outside the model** below) stays in the model for hard-deny and store checks (§4): every word counts as a candidate `Path`, the unit counts as a possible writer (§5), and it is never eligible for Home's read-only list. Under a covering prefix/exact Allow rule meeting §7's simple-command conditions, Clear targets remain Clear.
- An **explicitly excluded form** makes its unit **Unread**: every entry in **Outside the model**, any unlisted flag on a `ListedOnly` row, any forbidden flag on an `AnyExceptForbidden` row, or any form triggering an `Unread` effect (§3–§6).

## Modeled tools

| Tool | Platforms | Flags | Operands | Effects & pinning | Home read-only | Changes default (§11) |
|---|---|---|---|---|---|---|
| `command` | Linux, macOS | ListedOnly: none (any flag → Unread) | — | access: AllRead; links(root=Y, below=N); wrapper: Command | No | §11 item 10 |
| `exec` | Linux, macOS | ListedOnly: none (any flag → Unread) | — | access: AllRead; links(root=Y, below=N); wrapper: Exec | No | §11 item 10 |
| `nice` | Linux, macOS | ListedOnly: `-n` (Text) | — | access: AllRead; links(root=Y, below=N); wrapper: Nice | No | §11 item 10 |
| `timeout` | Linux, macOS | ListedOnly: `-s` (Text), `-k` (Text) | [0] Text | access: AllRead; links(root=Y, below=N); wrapper: Timeout | No | §11 item 10 |
| `time` | Linux, macOS | ListedOnly: none (any flag → Unread) | — | access: AllRead; links(root=Y, below=N); wrapper: Time | No | §11 item 10 |
| `env` | Linux, macOS | ListedOnly: none (any flag → Unread) | — | access: AllRead; links(root=Y, below=N); wrapper: Env | No | §11 item 10 |
| `sh` | Linux, macOS | ListedOnly: `-e`, `-u`, `-x`, `-c` (Code) [runs-code] | [0] ProgramFile, rest: Text | access: AllRead; writes; links(root=Y, below=N); code: program/inline (pin: FirstOperand); unpinned-prefix-invalid; nested-shell | No | §11 item 2; §11 item 4; §11 item 8 |
| `bash` | Linux, macOS | ListedOnly: `-e`, `-u`, `-x`, `-c` (Code) [runs-code] | [0] ProgramFile, rest: Text | access: AllRead; writes; links(root=Y, below=N); code: program/inline (pin: FirstOperand); unpinned-prefix-invalid; nested-shell | No | §11 item 2; §11 item 4; §11 item 8 |
| `dash` | Linux, macOS | ListedOnly: `-e`, `-u`, `-x`, `-c` (Code) [runs-code] | [0] ProgramFile, rest: Text | access: AllRead; writes; links(root=Y, below=N); code: program/inline (pin: FirstOperand); unpinned-prefix-invalid; nested-shell | No | §11 item 2; §11 item 4; §11 item 8 |
| `ls` | Linux, macOS | AnyExceptForbidden: any non-forbidden flag | rest: Path | access: AllRead; links(root=Y, below=N) | Yes | — |
| `pwd` | Linux, macOS | AnyExceptForbidden: any non-forbidden flag | rest: Path | access: AllRead; links(root=Y, below=N) | Yes | — |
| `cat` | Linux, macOS | AnyExceptForbidden: any non-forbidden flag | rest: Path | access: AllRead; links(root=Y, below=N) | Yes | — |
| `head` | Linux, macOS | AnyExceptForbidden: any non-forbidden flag | rest: Path | access: AllRead; links(root=Y, below=N) | Yes | — |
| `tail` | Linux, macOS | AnyExceptForbidden: any non-forbidden flag | rest: Path | access: AllRead; links(root=Y, below=N) | Yes | — |
| `wc` | Linux, macOS | AnyExceptForbidden: any non-forbidden flag | rest: Path | access: AllRead; links(root=Y, below=N) | Yes | — |
| `file` | Linux, macOS | AnyExceptForbidden: any non-forbidden flag | rest: Path | access: AllRead; links(root=Y, below=N) | Yes (forbids -C, --compile, -m, --magic-file, -f, --files-from) | — |
| `stat` | Linux, macOS | AnyExceptForbidden: any non-forbidden flag | rest: Path | access: AllRead; links(root=N, below=N) | Yes | — |
| `du` | Linux, macOS | AnyExceptForbidden: any non-forbidden flag | rest: Path | access: AllRead; links(root=Y, below=N) | Yes | — |
| `df` | Linux, macOS | AnyExceptForbidden: any non-forbidden flag | rest: Path | access: AllRead; links(root=Y, below=N) | Yes | — |
| `which` | Linux, macOS | AnyExceptForbidden: any non-forbidden flag | rest: Text | access: AllRead; links(root=Y, below=N) | Yes | §11 item 9 |
| `uname` | Linux, macOS | AnyExceptForbidden: any non-forbidden flag | rest: Path | access: AllRead; links(root=Y, below=N) | Yes | — |
| `date` | Linux, macOS | AnyExceptForbidden: any non-forbidden flag | rest: Path | access: AllRead; links(root=Y, below=N) | Yes (forbids -s, --set, -f, --file) | — |
| `whoami` | Linux, macOS | AnyExceptForbidden: any non-forbidden flag | rest: Path | access: AllRead; links(root=Y, below=N) | Yes | — |
| `tree` | Linux, macOS | AnyExceptForbidden: `-o/--output` (Path) [writes:+] | rest: Path | access: FlagOnlyWrite; writes (flag); links(root=Y, below=N) | Yes (forbids -o, --output, -R) | — |
| `rg` | Linux, macOS | ListedOnly: 83 listed flags (53 bool, 30 value; see flag table below) | [0] Pattern, rest: Path | access: AllRead; recursive; links(root=Y, below=N); filter: RgGlob | Yes (forbids --pre, --pre-glob, -z, --search-zip, --hostname-bin, -f, --file) [recursive] [allows followed search (§11 item 15 proposed)] | §11 item 9; §11 item 15 (proposed; §10 row 15) |
| `grep` | Linux, macOS | ListedOnly: 56 listed flags (40 bool, 16 value; see flag table below) | [0] Pattern, rest: Path | access: AllRead; recursive (flag); links(root=Y, below=N); filter: GrepInclude | Yes (forbids -f, --file) [recursive w/ flags] [allows followed search (§11 item 15 proposed)] | §11 item 9; §11 item 15 (proposed; §10 row 15) |
| `egrep` | Linux, macOS | ListedOnly: 56 listed flags (40 bool, 16 value; see flag table below) | [0] Pattern, rest: Path | access: AllRead; recursive (flag); links(root=Y, below=N); filter: GrepInclude | No | §11 item 9 |
| `fgrep` | Linux, macOS | ListedOnly: 56 listed flags (40 bool, 16 value; see flag table below) | [0] Pattern, rest: Path | access: AllRead; recursive (flag); links(root=Y, below=N); filter: GrepInclude | No | §11 item 9 |
| `zgrep` | Linux, macOS | ListedOnly: 56 listed flags (40 bool, 16 value; see flag table below) | [0] Pattern, rest: Path | access: AllRead; recursive (flag); links(root=Y, below=N); filter: GrepInclude | No | §11 item 9 |
| `ggrep` | Linux, macOS | ListedOnly: 56 listed flags (40 bool, 16 value; see flag table below) | [0] Pattern, rest: Path | access: AllRead; recursive (flag); links(root=Y, below=N); filter: GrepInclude | No | §11 item 9 |
| `rgrep` | Linux, macOS | ListedOnly: 56 listed flags (40 bool, 16 value; see flag table below) | [0] Pattern, rest: Path | access: AllRead; recursive; links(root=Y, below=N); filter: GrepInclude | No | §11 item 9 |
| `ag` | Linux, macOS | ListedOnly: 79 listed flags (62 bool, 17 value; see flag table below) | [0] Pattern, rest: Path | access: AllRead; recursive; links(root=Y, below=N); code: flag; unpinned-prefix-invalid | No | §11 item 4; §11 item 9 |
| `ack` | Linux, macOS | ListedOnly: 41 listed flags (31 bool, 10 value; see flag table below) | [0] Pattern, rest: Path | access: AllRead; recursive; links(root=Y, below=N); code: flag; unpinned-prefix-invalid | No | §11 item 4; §11 item 9 |
| `git` | Linux, macOS | AnyExceptForbidden: `-C` (Path), `-c` (Code) [runs-code], `--config-env` (Code) [runs-code], `--exec-path` (Code) [runs-code], `--git-dir` (Path), `--work-tree` (Path), `--namespace` (Text), `--super-prefix` (Text) | — | access: AllRead; links(root=Y, below=N); code: flag; unpinned-prefix-invalid | No | §11 item 4 |
| `cargo` | Linux, macOS | AnyExceptForbidden: any non-forbidden flag | — | access: AllRead; writes; links(root=Y, below=N); code: build (pin: CommandAndSubcommand); unpinned-prefix-invalid | No | §11 item 4; §11 item 8 |
| `pipx` | Linux, macOS | AnyExceptForbidden: any non-forbidden flag | — | access: AllRead; writes; links(root=Y, below=N); code: subarg run; unpinned-prefix-invalid | No | §11 item 4; §11 item 8 |
| `cd` | Linux, macOS | ListedOnly: `-L`, `-P`, `-e`, `-@` | [0] Path | access: AllRead; links(root=Y, below=N) | No | — |
| `find` | Linux, macOS | AnyExceptForbidden: 22 listed flags (10 bool, 12 value; see flag table below) | rest: Path | access: FlagOnlyWrite; writes (flag); recursive; links(root=N, below=N); code: flag; unpinned-prefix-invalid | No | §11 item 4 |
| `ln` | Linux, macOS | AnyExceptForbidden: `-s` | [0] Path, [1] Path | access: SourcesReadLastWrite; writes; links(root=N, below=N) | No | — |
| `cp` | Linux, macOS | AnyExceptForbidden: `-r` [derived-dest:+; recursive:+], `-R/--recursive` [derived-dest:+; recursive:+], `-a/--archive` [derived-dest:+; recursive:+; links(root=-, below=-)], `-L/--dereference` [links(root=+, below=+)], `-P` [links(root=-, below=-)], `-d` [links(root=-, below=-)] | rest: Path, last: DestinationRoot | access: SourcesReadLastWrite; writes; derived-dest (flag); recursive (flag); links(root=Y, below=N) | No | §11 item 10 |
| `mv` | Linux, macOS | AnyExceptForbidden: any non-forbidden flag | rest: Path, last: DestinationRoot | access: AllWrite; writes; derived-dest (dir src); links(root=N, below=N) | No | §11 item 10 |
| `rsync` | Linux, macOS | AnyExceptForbidden: `-L/--copy-links` [links(root=+, below=+)], `-k/--copy-dirlinks` [links(root=+, below=+)], `--copy-unsafe-links` [links(root=+, below=+)] | rest: Path, last: DestinationRoot | access: SourcesReadLastWrite; writes; derived-dest; recursive; links(root=Y, below=N) | No | §11 item 10 |
| `tar` | Linux, macOS | AnyExceptForbidden: `-c/--create` [writes:+; positional-role(Path)], `-r/--append` [writes:+; positional-role(Path)], `-u/--update` [writes:+; positional-role(Path)], `-x/--extract` [writes:+; derived-dest:+; positional-role(ArchiveMember)], `-t/--list` [positional-role(ArchiveMember)], `-h/--dereference` [links(root=+, below=+)], `-f/--file` (Path), `-C/--directory` (DestinationRoot) | rest: Path | access: FlagOnlyWrite; writes (flag); derived-dest (flag); recursive; links(root=N, below=N) | No | §11 item 9; §11 item 10 |
| `unzip` | Linux, macOS | AnyExceptForbidden: `-d` (DestinationRoot) [writes:+] | [0] Path, rest: ArchiveMember | access: FlagOnlyWrite; writes; derived-dest; links(root=Y, below=N) | No | §11 item 9; §11 item 10 |
| `rm` | Linux, macOS | AnyExceptForbidden: `-r/--recursive` [recursive:+], `-R` [recursive:+] | rest: Path | access: AllWrite; writes; recursive (flag); links(root=N, below=N) | No | — |
| `tee` | Linux, macOS | AnyExceptForbidden: any non-forbidden flag | rest: Path | access: AllWrite; writes; links(root=Y, below=N) | No | — |
| `touch` | Linux, macOS | AnyExceptForbidden: any non-forbidden flag | rest: Path | access: AllWrite; writes; links(root=Y, below=N) | No | — |
| `chmod` | Linux, macOS | AnyExceptForbidden: any non-forbidden flag | [0] Text, rest: Path | access: AllWrite; writes; links(root=Y, below=N) | No | §11 item 9 |
| `chown` | Linux, macOS | AnyExceptForbidden: any non-forbidden flag | [0] Text, rest: Path | access: AllWrite; writes; links(root=Y, below=N) | No | §11 item 9 |
| `chgrp` | Linux, macOS | AnyExceptForbidden: any non-forbidden flag | [0] Text, rest: Path | access: AllWrite; writes; links(root=Y, below=N) | No | §11 item 9 |
| `truncate` | Linux, macOS | AnyExceptForbidden: any non-forbidden flag | rest: Path | access: AllWrite; writes; links(root=Y, below=N) | No | — |
| `install` | Linux, macOS | AnyExceptForbidden: any non-forbidden flag | rest: Path | access: SourcesReadLastWrite; writes; links(root=Y, below=N) | No | — |
| `mkdir` | Linux, macOS | AnyExceptForbidden: any non-forbidden flag | rest: Path | access: AllWrite; writes; links(root=Y, below=N) | No | — |
| `rmdir` | Linux, macOS | AnyExceptForbidden: any non-forbidden flag | rest: Path | access: AllWrite; writes; links(root=N, below=N) | No | — |
| `unlink` | Linux, macOS | AnyExceptForbidden: any non-forbidden flag | rest: Path | access: AllWrite; writes; links(root=N, below=N) | No | — |
| `shred` | Linux, macOS | AnyExceptForbidden: any non-forbidden flag | rest: Path | access: AllWrite; writes; links(root=Y, below=N) | No | — |
| `patch` | Linux, macOS | AnyExceptForbidden: any non-forbidden flag | rest: Path | access: AllWrite; writes; links(root=Y, below=N) | No | — |
| `wipefs` | Linux | AnyExceptForbidden: any non-forbidden flag | rest: Path | access: AllWrite; writes; links(root=Y, below=N) | No | — |
| `blkdiscard` | Linux | AnyExceptForbidden: any non-forbidden flag | rest: Path | access: AllWrite; writes; links(root=Y, below=N) | No | — |
| `ed` | Linux, macOS | AnyExceptForbidden: any non-forbidden flag | rest: Path | access: AllWrite; writes; links(root=Y, below=N) | No | — |
| `ex` | Linux, macOS | AnyExceptForbidden: any non-forbidden flag | rest: Path | access: AllWrite; writes; links(root=Y, below=N) | No | — |
| `diff` | Linux, macOS | AnyExceptForbidden: any non-forbidden flag | rest: Path | access: AllRead; links(root=Y, below=N) | No | — |
| `cmp` | Linux, macOS | AnyExceptForbidden: any non-forbidden flag | rest: Path | access: AllRead; links(root=Y, below=N) | No | — |
| `md5` | macOS | AnyExceptForbidden: any non-forbidden flag | rest: Path | access: AllRead; links(root=Y, below=N) | No | — |
| `md5sum` | Linux | AnyExceptForbidden: any non-forbidden flag | rest: Path | access: AllRead; links(root=Y, below=N) | No | — |
| `shasum` | Linux, macOS | AnyExceptForbidden: any non-forbidden flag | rest: Path | access: AllRead; links(root=Y, below=N) | No | — |
| `sha1sum` | Linux, macOS | AnyExceptForbidden: any non-forbidden flag | rest: Path | access: AllRead; links(root=Y, below=N) | No | — |
| `sha256sum` | Linux, macOS | AnyExceptForbidden: any non-forbidden flag | rest: Path | access: AllRead; links(root=Y, below=N) | No | — |
| `readlink` | Linux, macOS | AnyExceptForbidden: any non-forbidden flag | rest: Path | access: AllRead; links(root=N, below=N) | No | — |
| `realpath` | Linux, macOS | AnyExceptForbidden: any non-forbidden flag | rest: Path | access: AllRead; links(root=Y, below=Y) | No | — |
| `basename` | Linux, macOS | AnyExceptForbidden: any non-forbidden flag | rest: Path | access: AllRead; links(root=N, below=N) | No | — |
| `dirname` | Linux, macOS | AnyExceptForbidden: any non-forbidden flag | rest: Path | access: AllRead; links(root=N, below=N) | No | — |
| `nl` | Linux, macOS | AnyExceptForbidden: any non-forbidden flag | rest: Path | access: AllRead; links(root=Y, below=N) | No | — |
| `od` | Linux, macOS | AnyExceptForbidden: any non-forbidden flag | rest: Path | access: AllRead; links(root=Y, below=N) | No | — |
| `hexdump` | Linux, macOS | AnyExceptForbidden: any non-forbidden flag | rest: Path | access: AllRead; links(root=Y, below=N) | No | — |
| `strings` | Linux, macOS | AnyExceptForbidden: any non-forbidden flag | rest: Path | access: AllRead; links(root=Y, below=N) | No | — |
| `bat` | Linux, macOS | AnyExceptForbidden: any non-forbidden flag | rest: Path | access: AllRead; links(root=Y, below=N) | No | — |
| `column` | Linux, macOS | AnyExceptForbidden: any non-forbidden flag | rest: Path | access: AllRead; links(root=Y, below=N) | No | — |
| `fold` | Linux, macOS | AnyExceptForbidden: any non-forbidden flag | rest: Path | access: AllRead; links(root=Y, below=N) | No | — |
| `expand` | Linux, macOS | AnyExceptForbidden: any non-forbidden flag | rest: Path | access: AllRead; links(root=Y, below=N) | No | — |
| `less` | Linux, macOS | AnyExceptForbidden: any non-forbidden flag | rest: Path | access: AllRead; links(root=Y, below=N) | No | — |
| `more` | Linux, macOS | AnyExceptForbidden: any non-forbidden flag | rest: Path | access: AllRead; links(root=Y, below=N) | No | — |
| `cut` | Linux, macOS | AnyExceptForbidden: any non-forbidden flag | rest: Path | access: AllRead; links(root=Y, below=N) | No | — |
| `sort` | Linux, macOS | AnyExceptForbidden: `-o/--output` (Path) [writes:+] | rest: Path | access: FlagOnlyWrite; writes (flag); links(root=Y, below=N) | No | — |
| `uniq` | Linux, macOS | AnyExceptForbidden: any non-forbidden flag | [0] Path, [1] Path | access: SourcesReadLastWrite; writes (>= 2 args); links(root=Y, below=N) | No | — |
| `xxd` | Linux, macOS | AnyExceptForbidden: any non-forbidden flag | [0] Path, [1] Path | access: SourcesReadLastWrite; writes (>= 2 args); links(root=Y, below=N) | No | — |
| `echo` | Linux, macOS | AnyExceptForbidden: `-n`, `-e`, `-E` | rest: Text | access: AllRead; links(root=Y, below=N) | No | §11 item 9 |
| `printf` | Linux, macOS | AnyExceptForbidden: `-v` (Text) | rest: Text | access: AllRead; links(root=Y, below=N) | No | §11 item 9 |
| `whereis` | Linux, macOS | AnyExceptForbidden: any non-forbidden flag | rest: Text | access: AllRead; links(root=Y, below=N) | No | §11 item 9 |
| `type` | Linux, macOS | AnyExceptForbidden: any non-forbidden flag | rest: Text | access: AllRead; links(root=Y, below=N) | No | §11 item 9 |
| `tr` | Linux, macOS | AnyExceptForbidden: any non-forbidden flag | rest: Text | access: AllRead; links(root=Y, below=N) | No | §11 item 9 |
| `true` | Linux, macOS | AnyExceptForbidden: any non-forbidden flag | rest: Text | access: AllRead; links(root=Y, below=N) | No | §11 item 9 |
| `false` | Linux, macOS | AnyExceptForbidden: any non-forbidden flag | rest: Text | access: AllRead; links(root=Y, below=N) | No | §11 item 9 |
| `id` | Linux, macOS | AnyExceptForbidden: any non-forbidden flag | rest: Text | access: AllRead; links(root=Y, below=N) | No | §11 item 9 |
| `test` | Linux, macOS | AnyExceptForbidden: any non-forbidden flag | rest: Path | access: AllRead; links(root=Y, below=N) | No | — |
| `[` | Linux, macOS | AnyExceptForbidden: any non-forbidden flag | rest: Path | access: AllRead; links(root=Y, below=N) | No | — |
| `jq` | Linux, macOS | AnyExceptForbidden: any non-forbidden flag | [0] Pattern, rest: Path | access: AllRead; links(root=Y, below=N) | No | §11 item 9 |
| `mdls` | macOS | AnyExceptForbidden: any non-forbidden flag | rest: Path | access: AllRead; links(root=Y, below=N) | No | — |
| `open` | macOS | AnyExceptForbidden: any non-forbidden flag | rest: Path | access: AllRead; links(root=Y, below=N) | No | — |
| `spctl` | macOS | AnyExceptForbidden: any non-forbidden flag | rest: Path | access: AllRead; links(root=Y, below=N) | No | — |
| `mdfind` | macOS | AnyExceptForbidden: any non-forbidden flag | rest: Path | access: AllRead; links(root=Y, below=N) | No | — |
| `codesign` | macOS | AnyExceptForbidden: `-s/--sign` (Text) [writes:+], `-f/--force` [writes:+], `--remove-signature` [writes:+], `-d/--display`, `-v/--verify` | rest: Path | access: AllWriteWhenWriting; writes (flag); links(root=Y, below=N) | No | — |
| `awk` | Linux, macOS | AnyExceptForbidden: `-i/--include` (Code) [writes:+; runs-code] | [0] Code, rest: Path | access: AllWriteWhenWriting; writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 4 |
| `gawk` | Linux, macOS | AnyExceptForbidden: `-i/--include` (Code) [writes:+; runs-code] | [0] Code, rest: Path | access: AllWriteWhenWriting; writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 4 |
| `mawk` | Linux, macOS | AnyExceptForbidden: `-i/--include` (Code) [writes:+; runs-code] | [0] Code, rest: Path | access: AllWriteWhenWriting; writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 4 |
| `sed` | Linux, macOS | AnyExceptForbidden: `-i/--in-place` [writes:+], `-e/--expression` (Code) [provides-script], `-f/--file` (ProgramFile) [writes:+; provides-script; runs-code] | [0] Code, rest: Path | access: AllWriteWhenWriting; writes (flag); links(root=Y, below=N); code: script commands (e/w/r); unpinned-prefix-invalid | No | §11 item 4 |
| `perl` | Linux, macOS | AnyExceptForbidden: `-i` [writes:+], `-e` (Code) [runs-code], `-E` (Code) [runs-code] | [0] ProgramFile, rest: Text | access: AllWriteWhenWriting; writes; links(root=Y, below=N); code: program/inline (pin: FirstOperand); unpinned-prefix-invalid | No | §11 item 4; §11 item 8 |
| `python` | Linux, macOS | AnyExceptForbidden: `-c` (Code) [runs-code], `-m` (Code) [runs-code] | [0] ProgramFile, rest: Text | access: AllRead; writes; links(root=Y, below=N); code: program/inline (pin: FirstOperand); unpinned-prefix-invalid | No | §11 item 4; §11 item 8 |
| `python3` | Linux, macOS | AnyExceptForbidden: `-c` (Code) [runs-code], `-m` (Code) [runs-code] | [0] ProgramFile, rest: Text | access: AllRead; writes; links(root=Y, below=N); code: program/inline (pin: FirstOperand); unpinned-prefix-invalid | No | §11 item 4; §11 item 8 |
| `node` | Linux, macOS | AnyExceptForbidden: `-e/--eval` (Code) [runs-code], `-p/--print` (Code) [runs-code] | [0] ProgramFile, rest: Text | access: AllRead; writes; links(root=Y, below=N); code: program/inline (pin: FirstOperand); unpinned-prefix-invalid | No | §11 item 4; §11 item 8 |
| `ruby` | Linux, macOS | AnyExceptForbidden: `-e` (Code) [runs-code] | [0] ProgramFile, rest: Text | access: AllRead; writes; links(root=Y, below=N); code: program/inline (pin: FirstOperand); unpinned-prefix-invalid | No | §11 item 4; §11 item 8 |
| `eval` | Linux, macOS | AnyExceptForbidden: any non-forbidden flag | rest: Code | access: AllRead; writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 4 |
| `source` | Linux, macOS | AnyExceptForbidden: any non-forbidden flag | [0] ProgramFile, rest: Text | access: AllRead; writes; links(root=Y, below=N); code: program/inline (pin: FirstOperand); unpinned-prefix-invalid | No | §11 item 4; §11 item 8 |
| `.` | Linux, macOS | AnyExceptForbidden: any non-forbidden flag | [0] ProgramFile, rest: Text | access: AllRead; writes; links(root=Y, below=N); code: program/inline (pin: FirstOperand); unpinned-prefix-invalid | No | §11 item 4; §11 item 8 |
| `npx` | Linux, macOS | AnyExceptForbidden: any non-forbidden flag | [0] ProgramFile, rest: Text | access: AllRead; writes; links(root=Y, below=N); code: program/inline (pin: FirstOperand); unpinned-prefix-invalid | No | §11 item 4; §11 item 8 |
| `xargs` | Linux, macOS | AnyExceptForbidden: any non-forbidden flag | rest: Code | access: AllRead; writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 4 |
| `ssh` | Linux, macOS | AnyExceptForbidden: any non-forbidden flag | [0] Text, rest: Code | access: AllRead; writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 4 |
| `mosh` | Linux, macOS | AnyExceptForbidden: any non-forbidden flag | [0] Text, rest: Code | access: AllRead; writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 4 |
| `autossh` | Linux, macOS | AnyExceptForbidden: any non-forbidden flag | [0] Text, rest: Code | access: AllRead; writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 4 |
| `nsenter` | Linux | AnyExceptForbidden: any non-forbidden flag | rest: Code | access: AllRead; writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 4 |
| `unshare` | Linux | AnyExceptForbidden: any non-forbidden flag | rest: Code | access: AllRead; writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 4 |
| `docker` | Linux, macOS | AnyExceptForbidden: any non-forbidden flag | [0] Text, rest: Code | access: AllRead; writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 4 |
| `podman` | Linux, macOS | AnyExceptForbidden: any non-forbidden flag | [0] Text, rest: Code | access: AllRead; writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 4 |
| `nerdctl` | Linux, macOS | AnyExceptForbidden: any non-forbidden flag | [0] Text, rest: Code | access: AllRead; writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 4 |
| `kubectl` | Linux, macOS | AnyExceptForbidden: any non-forbidden flag | [0] Text, rest: Code | access: AllRead; writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 4 |
| `oc` | Linux, macOS | AnyExceptForbidden: any non-forbidden flag | [0] Text, rest: Code | access: AllRead; writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 4 |
| `lxc` | Linux, macOS | AnyExceptForbidden: any non-forbidden flag | [0] Text, rest: Code | access: AllRead; writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 4 |
| `incus` | Linux, macOS | AnyExceptForbidden: any non-forbidden flag | [0] Text, rest: Code | access: AllRead; writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 4 |
| `machinectl` | Linux | AnyExceptForbidden: any non-forbidden flag | [0] Text, rest: Code | access: AllRead; writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 4 |
| `systemd-run` | Linux | AnyExceptForbidden: any non-forbidden flag | rest: Code | access: AllRead; writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 4 |
| `systemd-nspawn` | Linux | AnyExceptForbidden: any non-forbidden flag | rest: Code | access: AllRead; writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 4 |
| `firejail` | Linux | AnyExceptForbidden: any non-forbidden flag | rest: Code | access: AllRead; writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 4 |
| `bwrap` | Linux | AnyExceptForbidden: any non-forbidden flag | rest: Code | access: AllRead; writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 4 |
| `flatpak-spawn` | Linux | AnyExceptForbidden: any non-forbidden flag | rest: Code | access: AllRead; writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 4 |
| `toolbox` | Linux, macOS | AnyExceptForbidden: any non-forbidden flag | [0] Text, rest: Code | access: AllRead; writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 4 |
| `distrobox` | Linux, macOS | AnyExceptForbidden: any non-forbidden flag | [0] Text, rest: Code | access: AllRead; writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 4 |
| `distrobox-enter` | Linux, macOS | AnyExceptForbidden: any non-forbidden flag | [0] Text, rest: Code | access: AllRead; writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 4 |
| `multipass` | Linux, macOS | AnyExceptForbidden: any non-forbidden flag | [0] Text, rest: Code | access: AllRead; writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 4 |
| `vagrant` | Linux, macOS | AnyExceptForbidden: any non-forbidden flag | [0] Text, rest: Code | access: AllRead; writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 4 |
| `limactl` | Linux, macOS | AnyExceptForbidden: any non-forbidden flag | [0] Text, rest: Code | access: AllRead; writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 4 |
| `colima` | Linux, macOS | AnyExceptForbidden: any non-forbidden flag | [0] Text, rest: Code | access: AllRead; writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 4 |
| `runuser` | Linux | AnyExceptForbidden: any non-forbidden flag | [0] Text, rest: Code | access: AllRead; writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 4 |
| `setpriv` | Linux | AnyExceptForbidden: any non-forbidden flag | rest: Code | access: AllRead; writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 4 |
| `parallel` | Linux, macOS | AnyExceptForbidden: any non-forbidden flag | rest: Code | access: AllRead; writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 4 |
| `sudo` | Linux, macOS | AnyExceptForbidden: any non-forbidden flag | rest: Code | access: AllRead; writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid; hard-deny: `privilege-escalation` | No | — |
| `sudoedit` | Linux, macOS | AnyExceptForbidden: any non-forbidden flag | rest: Path | access: AllWrite; writes; links(root=Y, below=N); hard-deny: `privilege-escalation` | No | — |
| `su` | Linux, macOS | AnyExceptForbidden: any non-forbidden flag | rest: Code | access: AllRead; writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid; hard-deny: `privilege-escalation` | No | — |
| `doas` | Linux, macOS | AnyExceptForbidden: any non-forbidden flag | rest: Code | access: AllRead; writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid; hard-deny: `privilege-escalation` | No | — |
| `pkexec` | Linux | AnyExceptForbidden: any non-forbidden flag | rest: Code | access: AllRead; writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid; hard-deny: `privilege-escalation` | No | — |
| `run0` | Linux | AnyExceptForbidden: any non-forbidden flag | rest: Code | access: AllRead; writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid; hard-deny: `privilege-escalation` | No | — |
| `curl` | Linux, macOS | AnyExceptForbidden: any non-forbidden flag | rest: Path | access: AllWrite; writes; links(root=Y, below=N); downloader | No | — |
| `wget` | Linux, macOS | AnyExceptForbidden: any non-forbidden flag | rest: Path | access: AllWrite; writes; links(root=Y, below=N); downloader | No | — |
| `fetch` | Linux, macOS | AnyExceptForbidden: any non-forbidden flag | rest: Path | access: AllWrite; writes; links(root=Y, below=N); downloader | No | — |

## Subcommands

### `git`

| Subcommand | Flags | Operands | Effects & pinning | Home read-only | Changes default (§11) |
|---|---|---|---|---|---|
| `git status` | AnyExceptForbidden: any non-forbidden flag | rest: Path | access: AllRead; links(root=Y, below=N) | Yes (forbids --output, --ext-diff, --exec-path, --config-env, -c) | §11 item 10 |
| `git log` | AnyExceptForbidden: `-p`, `-L/--line-range` (Text) | rest: Ref | access: AllRead; links(root=Y, below=N); git-cred-select | Yes (forbids --output, --ext-diff, --exec-path, --config-env, -c) | §11 item 9; §11 item 10 |
| `git diff` | AnyExceptForbidden: `--output` (Path) [writes:+], `--ext-diff` [runs-code] | rest: Ref | access: FlagOnlyWrite; writes (flag); links(root=Y, below=N); code: flag; unpinned-prefix-invalid; git-cred-select | Yes (forbids --output, --ext-diff, --exec-path, --config-env, -c) | §11 item 4; §11 item 9; §11 item 10 |
| `git show` | AnyExceptForbidden: any non-forbidden flag | rest: Ref | access: AllRead; links(root=Y, below=N); git-cred-select | Yes (forbids --output, --ext-diff, --exec-path, --config-env, -c) | §11 item 9; §11 item 10 |
| `git branch` | AnyExceptForbidden: 14 listed flags (13 bool, 1 value; see flag table below) | rest: Ref | access: AllRead; writes (flag or >= 1 args unless disabled); links(root=Y, below=N) | Yes (forbids --output, --ext-diff, --exec-path, --config-env, -c) (only -a, --all, -r, --remotes, -v, -vv, --verbose, --list, -l, --show-current) | §11 item 9 |
| `git rev-parse` | AnyExceptForbidden: any non-forbidden flag | rest: Ref | access: AllRead; links(root=Y, below=N) | Yes (forbids --output, --ext-diff, --exec-path, --config-env, -c) | §11 item 9 |
| `git ls-files` | AnyExceptForbidden: any non-forbidden flag | rest: Path | access: AllRead; links(root=Y, below=N) | Yes (forbids --output, --ext-diff, --exec-path, --config-env, -c) | §11 item 10 |
| `git cat-file` | AnyExceptForbidden: any non-forbidden flag | rest: Ref | access: AllRead; links(root=Y, below=N); git-cred-select | No | §11 item 9 |
| `git push` | AnyExceptForbidden: `-f/--force`, `--force-with-lease`, `--force-if-includes`, `--mirror`, `--all`, `--tags`, `--branches`, `--prune`, `-o/--push-option` (Text), `--receive-pack` (Code) [runs-code], `--exec` (Code) [runs-code] | rest: Ref | access: AllRead; writes; links(root=Y, below=N); code: flag; unpinned-prefix-invalid | No | §11 item 4; §11 item 9 |
| `git clone` | AnyExceptForbidden: any non-forbidden flag | [0] Ref, [1] DestinationRoot | access: SourcesReadLastWrite; writes; derived-dest; links(root=Y, below=N) | No | §11 item 9; §11 item 10 |
| `git checkout` | AnyExceptForbidden: `-b` (Ref), `-B` (Ref), `--orphan` (Ref) | [0] Ref, rest: DestinationRoot, after `--`: DestinationRoot | access: AllWrite; writes; derived-dest; links(root=Y, below=N) | No | §11 item 9; §11 item 10 |
| `git merge-index` | AnyExceptForbidden: any non-forbidden flag | rest: Code | access: AllRead; writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 4 |
| `git filter-branch` | AnyExceptForbidden: any non-forbidden flag | rest: Code | access: AllRead; writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 4 |
| `git bisect` | AnyExceptForbidden: any non-forbidden flag | [0] Text, rest: Code | access: AllRead; writes; links(root=Y, below=N); code: subarg run; unpinned-prefix-invalid | No | §11 item 4 |
| `git rebase` | AnyExceptForbidden: `-x/--exec` (Code) [runs-code] | rest: Ref | access: AllRead; writes; links(root=Y, below=N); code: flag; unpinned-prefix-invalid | No | §11 item 4 |
| `git submodule` | AnyExceptForbidden: any non-forbidden flag | [0] Text, rest: Code | access: AllRead; writes; links(root=Y, below=N); code: subarg foreach; unpinned-prefix-invalid | No | §11 item 4 |
| `git difftool` | AnyExceptForbidden: `-x/--tool-cmd` (Code) [runs-code] | rest: Ref | access: AllRead; links(root=Y, below=N); code: flag; unpinned-prefix-invalid | No | §11 item 4 |
| `git mergetool` | AnyExceptForbidden: `-x/--tool-cmd` (Code) [runs-code] | rest: Path | access: AllWrite; writes; links(root=Y, below=N); code: flag; unpinned-prefix-invalid | No | §11 item 4 |

### `cargo`

| Subcommand | Flags | Operands | Effects & pinning | Home read-only | Changes default (§11) |
|---|---|---|---|---|---|
| `cargo test` | AnyExceptForbidden: any non-forbidden flag | rest: Text | access: AllRead; writes; links(root=Y, below=N); code: build (pin: CommandAndSubcommand) | No | §11 item 8 |
| `cargo build` | AnyExceptForbidden: any non-forbidden flag | rest: Text | access: AllRead; writes; links(root=Y, below=N); code: build (pin: CommandAndSubcommand) | No | §11 item 8 |
| `cargo run` | AnyExceptForbidden: any non-forbidden flag | rest: Text | access: AllRead; writes; links(root=Y, below=N); code: build (pin: CommandAndSubcommand) | No | §11 item 8 |
| `cargo check` | AnyExceptForbidden: any non-forbidden flag | rest: Text | access: AllRead; writes; links(root=Y, below=N); code: build (pin: CommandAndSubcommand) | No | §11 item 8 |
| `cargo bench` | AnyExceptForbidden: any non-forbidden flag | rest: Text | access: AllRead; writes; links(root=Y, below=N); code: build (pin: CommandAndSubcommand) | No | §11 item 8 |
| `cargo doc` | AnyExceptForbidden: any non-forbidden flag | rest: Text | access: AllRead; writes; links(root=Y, below=N); code: build (pin: CommandAndSubcommand) | No | §11 item 8 |
| `cargo clippy` | AnyExceptForbidden: any non-forbidden flag | rest: Text | access: AllRead; writes; links(root=Y, below=N); code: build (pin: CommandAndSubcommand) | No | §11 item 8 |

### `pipx`

| Subcommand | Flags | Operands | Effects & pinning | Home read-only | Changes default (§11) |
|---|---|---|---|---|---|
| `pipx run` | AnyExceptForbidden: any non-forbidden flag | [0] ProgramFile, rest: Text | access: AllRead; writes; links(root=Y, below=N); code: program/inline (pin: FirstOperand); unpinned-prefix-invalid | No | §11 item 4; §11 item 8 |

## Detailed flag tables

### `nice` (1 flags, ListedOnly)

| Short | Long | Kind | Effects |
|---|---|---|---|
| `-n` | — | Value(Text) | — |

### `timeout` (2 flags, ListedOnly)

| Short | Long | Kind | Effects |
|---|---|---|---|
| `-s` | — | Value(Text) | — |
| `-k` | — | Value(Text) | — |

### `sh` (4 flags, ListedOnly)

| Short | Long | Kind | Effects |
|---|---|---|---|
| `-e` | — | Bool | — |
| `-u` | — | Bool | — |
| `-x` | — | Bool | — |
| `-c` | — | Value(Code) | runs-code |

### `bash` (4 flags, ListedOnly)

| Short | Long | Kind | Effects |
|---|---|---|---|
| `-e` | — | Bool | — |
| `-u` | — | Bool | — |
| `-x` | — | Bool | — |
| `-c` | — | Value(Code) | runs-code |

### `dash` (4 flags, ListedOnly)

| Short | Long | Kind | Effects |
|---|---|---|---|
| `-e` | — | Bool | — |
| `-u` | — | Bool | — |
| `-x` | — | Bool | — |
| `-c` | — | Value(Code) | runs-code |

### `tree` (1 flags, AnyExceptForbidden)

| Short | Long | Kind | Effects |
|---|---|---|---|
| `-o` | `--output` | Value(Path) | writes:+ |

### `rg` (83 flags, ListedOnly)

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
| `-L` | `--follow` | Bool | links(root=+, below=+) |
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

### `grep` (56 flags, ListedOnly)

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
| `-r` | `--recursive` | Bool | recursive:+; links(root==, below=-) |
| `-R` | `--dereference-recursive` | Bool | recursive:+; links(root=+, below=+) |
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
| `-d` | `--directories` | Value(Text) | recursive:+ |
| `-D` | `--devices` | Value(Text) | — |
| — | `--include` | Value(Pattern) | filter(GrepInclude, ci=false) |
| — | `--exclude` | Value(Pattern) | — |
| — | `--exclude-dir` | Value(Pattern) | — |
| — | `--label` | Value(Text) | — |
| — | `--color` | Value(Text) | — |
| — | `--colour` | Value(Text) | — |
| — | `--binary-files` | Value(Text) | — |
| — | `--group-separator` | Value(Text) | — |

### `egrep` (56 flags, ListedOnly)

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
| `-r` | `--recursive` | Bool | recursive:+; links(root==, below=-) |
| `-R` | `--dereference-recursive` | Bool | recursive:+; links(root=+, below=+) |
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
| `-d` | `--directories` | Value(Text) | recursive:+ |
| `-D` | `--devices` | Value(Text) | — |
| — | `--include` | Value(Pattern) | filter(GrepInclude, ci=false) |
| — | `--exclude` | Value(Pattern) | — |
| — | `--exclude-dir` | Value(Pattern) | — |
| — | `--label` | Value(Text) | — |
| — | `--color` | Value(Text) | — |
| — | `--colour` | Value(Text) | — |
| — | `--binary-files` | Value(Text) | — |
| — | `--group-separator` | Value(Text) | — |

### `fgrep` (56 flags, ListedOnly)

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
| `-r` | `--recursive` | Bool | recursive:+; links(root==, below=-) |
| `-R` | `--dereference-recursive` | Bool | recursive:+; links(root=+, below=+) |
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
| `-d` | `--directories` | Value(Text) | recursive:+ |
| `-D` | `--devices` | Value(Text) | — |
| — | `--include` | Value(Pattern) | filter(GrepInclude, ci=false) |
| — | `--exclude` | Value(Pattern) | — |
| — | `--exclude-dir` | Value(Pattern) | — |
| — | `--label` | Value(Text) | — |
| — | `--color` | Value(Text) | — |
| — | `--colour` | Value(Text) | — |
| — | `--binary-files` | Value(Text) | — |
| — | `--group-separator` | Value(Text) | — |

### `zgrep` (56 flags, ListedOnly)

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
| `-r` | `--recursive` | Bool | recursive:+; links(root==, below=-) |
| `-R` | `--dereference-recursive` | Bool | recursive:+; links(root=+, below=+) |
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
| `-d` | `--directories` | Value(Text) | recursive:+ |
| `-D` | `--devices` | Value(Text) | — |
| — | `--include` | Value(Pattern) | filter(GrepInclude, ci=false) |
| — | `--exclude` | Value(Pattern) | — |
| — | `--exclude-dir` | Value(Pattern) | — |
| — | `--label` | Value(Text) | — |
| — | `--color` | Value(Text) | — |
| — | `--colour` | Value(Text) | — |
| — | `--binary-files` | Value(Text) | — |
| — | `--group-separator` | Value(Text) | — |

### `ggrep` (56 flags, ListedOnly)

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
| `-r` | `--recursive` | Bool | recursive:+; links(root==, below=-) |
| `-R` | `--dereference-recursive` | Bool | recursive:+; links(root=+, below=+) |
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
| `-d` | `--directories` | Value(Text) | recursive:+ |
| `-D` | `--devices` | Value(Text) | — |
| — | `--include` | Value(Pattern) | filter(GrepInclude, ci=false) |
| — | `--exclude` | Value(Pattern) | — |
| — | `--exclude-dir` | Value(Pattern) | — |
| — | `--label` | Value(Text) | — |
| — | `--color` | Value(Text) | — |
| — | `--colour` | Value(Text) | — |
| — | `--binary-files` | Value(Text) | — |
| — | `--group-separator` | Value(Text) | — |

### `rgrep` (56 flags, ListedOnly)

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
| `-r` | `--recursive` | Bool | recursive:+; links(root==, below=-) |
| `-R` | `--dereference-recursive` | Bool | recursive:+; links(root=+, below=+) |
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
| `-d` | `--directories` | Value(Text) | recursive:+ |
| `-D` | `--devices` | Value(Text) | — |
| — | `--include` | Value(Pattern) | filter(GrepInclude, ci=false) |
| — | `--exclude` | Value(Pattern) | — |
| — | `--exclude-dir` | Value(Pattern) | — |
| — | `--label` | Value(Text) | — |
| — | `--color` | Value(Text) | — |
| — | `--colour` | Value(Text) | — |
| — | `--binary-files` | Value(Text) | — |
| — | `--group-separator` | Value(Text) | — |

### `ag` (79 flags, ListedOnly)

| Short | Long | Kind | Effects |
|---|---|---|---|
| `-a` | `--all-types` | Bool | — |
| `-D` | `--debug` | Bool | — |
| `-c` | `--count` | Bool | — |
| `-F` | `--fixed-strings` | Bool | — |
| `-f` | `--follow` | Bool | links(root=+, below=+) |
| `-H` | `--heading` | Bool | — |
| `-h` | — | Bool | — |
| `-i` | `--ignore-case` | Bool | — |
| `-L` | `--files-without-matches` | Bool | — |
| `-l` | `--files-with-matches` | Bool | — |
| `-n` | `--norecurse` | Bool | recursive:- |
| `-o` | `--only-matching` | Bool | — |
| `-Q` | `--literal` | Bool | — |
| `-R` | `--recurse` | Bool | recursive:+ |
| `-r` | — | Bool | recursive:+ |
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
| — | `--nofollow` | Bool | links(root=-, below=-) |
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

### `ack` (41 flags, ListedOnly)

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
| `-n` | `--no-recurse` | Bool | recursive:- |
| `-r` | `--recurse` | Bool | recursive:+ |
| `-R` | — | Bool | recursive:+ |
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
| — | `--follow` | Bool | links(root=+, below=+) |
| — | `--nofollow` | Bool | links(root=-, below=-) |
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

### `git` (8 flags, AnyExceptForbidden)

| Short | Long | Kind | Effects |
|---|---|---|---|
| `-C` | — | Value(Path) | — |
| `-c` | — | Value(Code) | runs-code |
| — | `--config-env` | Value(Code) | runs-code |
| — | `--exec-path` | Value(Code) | runs-code |
| — | `--git-dir` | Value(Path) | — |
| — | `--work-tree` | Value(Path) | — |
| — | `--namespace` | Value(Text) | — |
| — | `--super-prefix` | Value(Text) | — |

### `git log` (2 flags, AnyExceptForbidden)

| Short | Long | Kind | Effects |
|---|---|---|---|
| `-p` | — | Bool | — |
| `-L` | `--line-range` | Value(Text) | — |

### `git diff` (2 flags, AnyExceptForbidden)

| Short | Long | Kind | Effects |
|---|---|---|---|
| — | `--output` | Value(Path) | writes:+ |
| — | `--ext-diff` | Bool | runs-code |

### `git branch` (14 flags, AnyExceptForbidden)

| Short | Long | Kind | Effects |
|---|---|---|---|
| `-a` | `--all` | Bool | — |
| `-r` | `--remotes` | Bool | — |
| `-v` | `--verbose` | Bool | — |
| `-l` | `--list` | Bool | writes:- |
| — | `--show-current` | Bool | — |
| `-d` | `--delete` | Bool | writes:+ |
| `-D` | — | Bool | writes:+ |
| `-m` | `--move` | Bool | writes:+ |
| `-M` | — | Bool | writes:+ |
| `-c` | `--copy` | Bool | writes:+ |
| `-C` | — | Bool | writes:+ |
| `-u` | `--set-upstream-to` | Value(Ref) | writes:+ |
| — | `--unset-upstream` | Bool | writes:+ |
| — | `--edit-description` | Bool | writes:+ |

### `git push` (11 flags, AnyExceptForbidden)

| Short | Long | Kind | Effects |
|---|---|---|---|
| `-f` | `--force` | Bool | — |
| — | `--force-with-lease` | Bool | — |
| — | `--force-if-includes` | Bool | — |
| — | `--mirror` | Bool | — |
| — | `--all` | Bool | — |
| — | `--tags` | Bool | — |
| — | `--branches` | Bool | — |
| — | `--prune` | Bool | — |
| `-o` | `--push-option` | Value(Text) | — |
| — | `--receive-pack` | Value(Code) | runs-code |
| — | `--exec` | Value(Code) | runs-code |

### `git checkout` (3 flags, AnyExceptForbidden)

| Short | Long | Kind | Effects |
|---|---|---|---|
| `-b` | — | Value(Ref) | — |
| `-B` | — | Value(Ref) | — |
| — | `--orphan` | Value(Ref) | — |

### `git rebase` (1 flags, AnyExceptForbidden)

| Short | Long | Kind | Effects |
|---|---|---|---|
| `-x` | `--exec` | Value(Code) | runs-code |

### `git difftool` (1 flags, AnyExceptForbidden)

| Short | Long | Kind | Effects |
|---|---|---|---|
| `-x` | `--tool-cmd` | Value(Code) | runs-code |

### `git mergetool` (1 flags, AnyExceptForbidden)

| Short | Long | Kind | Effects |
|---|---|---|---|
| `-x` | `--tool-cmd` | Value(Code) | runs-code |

### `cd` (4 flags, ListedOnly)

| Short | Long | Kind | Effects |
|---|---|---|---|
| `-L` | — | Bool | — |
| `-P` | — | Bool | — |
| `-e` | — | Bool | — |
| `-@` | — | Bool | — |

### `find` (22 flags, AnyExceptForbidden)

| Short | Long | Kind | Effects |
|---|---|---|---|
| `-H` | — | Bool | links(root=+, below=-) |
| `-L` | `-follow` | Bool | links(root=+, below=+) |
| `-P` | — | Bool | links(root=-, below=-) |
| `-E` | — | Bool | — |
| `-X` | — | Bool | — |
| `-d` | — | Bool | — |
| `-s` | — | Bool | — |
| `-x` | — | Bool | — |
| `-D` | — | Value(Text) | — |
| `-f` | — | Value(Path) | — |
| — | `-delete` | Bool | writes:+ |
| — | `-depth` | Bool | — |
| — | `-maxdepth` | Value(Text) | — |
| — | `-mindepth` | Value(Text) | — |
| — | `-exec` | Value(Code) | writes:+; runs-code |
| — | `-execdir` | Value(Code) | writes:+; runs-code |
| — | `-ok` | Value(Code) | writes:+; runs-code |
| — | `-okdir` | Value(Code) | writes:+; runs-code |
| — | `-fprint` | Value(Path) | writes:+ |
| — | `-fprint0` | Value(Path) | writes:+ |
| — | `-fprintf` | Value(Path) | writes:+ |
| — | `-fls` | Value(Path) | writes:+ |

### `ln` (1 flags, AnyExceptForbidden)

| Short | Long | Kind | Effects |
|---|---|---|---|
| `-s` | — | Bool | — |

### `cp` (6 flags, AnyExceptForbidden)

| Short | Long | Kind | Effects |
|---|---|---|---|
| `-r` | — | Bool | derived-dest:+; recursive:+ |
| `-R` | `--recursive` | Bool | derived-dest:+; recursive:+ |
| `-a` | `--archive` | Bool | derived-dest:+; recursive:+; links(root=-, below=-) |
| `-L` | `--dereference` | Bool | links(root=+, below=+) |
| `-P` | — | Bool | links(root=-, below=-) |
| `-d` | — | Bool | links(root=-, below=-) |

### `rsync` (3 flags, AnyExceptForbidden)

| Short | Long | Kind | Effects |
|---|---|---|---|
| `-L` | `--copy-links` | Bool | links(root=+, below=+) |
| `-k` | `--copy-dirlinks` | Bool | links(root=+, below=+) |
| — | `--copy-unsafe-links` | Bool | links(root=+, below=+) |

### `tar` (8 flags, AnyExceptForbidden)

| Short | Long | Kind | Effects |
|---|---|---|---|
| `-c` | `--create` | Bool | writes:+; positional-role(Path) |
| `-r` | `--append` | Bool | writes:+; positional-role(Path) |
| `-u` | `--update` | Bool | writes:+; positional-role(Path) |
| `-x` | `--extract` | Bool | writes:+; derived-dest:+; positional-role(ArchiveMember) |
| `-t` | `--list` | Bool | positional-role(ArchiveMember) |
| `-h` | `--dereference` | Bool | links(root=+, below=+) |
| `-f` | `--file` | Value(Path) | — |
| `-C` | `--directory` | Value(DestinationRoot) | — |

### `unzip` (1 flags, AnyExceptForbidden)

| Short | Long | Kind | Effects |
|---|---|---|---|
| `-d` | — | Value(DestinationRoot) | writes:+ |

### `rm` (2 flags, AnyExceptForbidden)

| Short | Long | Kind | Effects |
|---|---|---|---|
| `-r` | `--recursive` | Bool | recursive:+ |
| `-R` | — | Bool | recursive:+ |

### `sort` (1 flags, AnyExceptForbidden)

| Short | Long | Kind | Effects |
|---|---|---|---|
| `-o` | `--output` | Value(Path) | writes:+ |

### `echo` (3 flags, AnyExceptForbidden)

| Short | Long | Kind | Effects |
|---|---|---|---|
| `-n` | — | Bool | — |
| `-e` | — | Bool | — |
| `-E` | — | Bool | — |

### `printf` (1 flags, AnyExceptForbidden)

| Short | Long | Kind | Effects |
|---|---|---|---|
| `-v` | — | Value(Text) | — |

### `codesign` (5 flags, AnyExceptForbidden)

| Short | Long | Kind | Effects |
|---|---|---|---|
| `-s` | `--sign` | Value(Text) | writes:+ |
| `-f` | `--force` | Bool | writes:+ |
| — | `--remove-signature` | Bool | writes:+ |
| `-d` | `--display` | Bool | — |
| `-v` | `--verify` | Bool | — |

### `awk` (1 flags, AnyExceptForbidden)

| Short | Long | Kind | Effects |
|---|---|---|---|
| `-i` | `--include` | Value(Code) | writes:+; runs-code |

### `gawk` (1 flags, AnyExceptForbidden)

| Short | Long | Kind | Effects |
|---|---|---|---|
| `-i` | `--include` | Value(Code) | writes:+; runs-code |

### `mawk` (1 flags, AnyExceptForbidden)

| Short | Long | Kind | Effects |
|---|---|---|---|
| `-i` | `--include` | Value(Code) | writes:+; runs-code |

### `sed` (3 flags, AnyExceptForbidden)

| Short | Long | Kind | Effects |
|---|---|---|---|
| `-i` | `--in-place` | Bool | writes:+ |
| `-e` | `--expression` | Value(Code) | provides-script |
| `-f` | `--file` | Value(ProgramFile) | writes:+; provides-script; runs-code |

### `perl` (3 flags, AnyExceptForbidden)

| Short | Long | Kind | Effects |
|---|---|---|---|
| `-i` | — | Bool | writes:+ |
| `-e` | — | Value(Code) | runs-code |
| `-E` | — | Value(Code) | runs-code |

### `python` (2 flags, AnyExceptForbidden)

| Short | Long | Kind | Effects |
|---|---|---|---|
| `-c` | — | Value(Code) | runs-code |
| `-m` | — | Value(Code) | runs-code |

### `python3` (2 flags, AnyExceptForbidden)

| Short | Long | Kind | Effects |
|---|---|---|---|
| `-c` | — | Value(Code) | runs-code |
| `-m` | — | Value(Code) | runs-code |

### `node` (2 flags, AnyExceptForbidden)

| Short | Long | Kind | Effects |
|---|---|---|---|
| `-e` | `--eval` | Value(Code) | runs-code |
| `-p` | `--print` | Value(Code) | runs-code |

### `ruby` (1 flags, AnyExceptForbidden)

| Short | Long | Kind | Effects |
|---|---|---|---|
| `-e` | — | Value(Code) | runs-code |

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
| `pushd` | Linux, macOS | No | §5 | — | Mutates shell directory stack; makes cwd unknown (§5) |
| `popd` | Linux, macOS | No | §5 | — | Mutates shell directory stack; makes cwd unknown (§5) |

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
| `rg` | §11 item 9; §11 item 15 (proposed; §10 row 15) |
| `grep` | §11 item 9; §11 item 15 (proposed; §10 row 15) |
| `egrep` | §11 item 9 |
| `fgrep` | §11 item 9 |
| `zgrep` | §11 item 9 |
| `ggrep` | §11 item 9 |
| `rgrep` | §11 item 9 |
| `ag` | §11 item 4; §11 item 9 |
| `ack` | §11 item 4; §11 item 9 |
| `git` | §11 item 4 |
| `git status` | §11 item 10 |
| `git log` | §11 item 9; §11 item 10 |
| `git diff` | §11 item 4; §11 item 9; §11 item 10 |
| `git show` | §11 item 9; §11 item 10 |
| `git branch` | §11 item 9 |
| `git rev-parse` | §11 item 9 |
| `git ls-files` | §11 item 10 |
| `git cat-file` | §11 item 9 |
| `git push` | §11 item 4; §11 item 9 |
| `git clone` | §11 item 9; §11 item 10 |
| `git checkout` | §11 item 9; §11 item 10 |
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
| `true` | §11 item 9 |
| `false` | §11 item 9 |
| `id` | §11 item 9 |
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
