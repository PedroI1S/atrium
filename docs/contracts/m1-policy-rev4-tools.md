# Policy engine rev 4: normative tool table (annex to §6)

Generated from `crates/atrium-core/src/policy/tools.rs`. Do not edit by hand; regenerate with `env UPDATE_TOOL_TABLE=1 cargo test -p atrium-core tools`.

**Unknown tools, Home-eligibility restrictions, and excluded forms (§4, §6):**
- An **unknown tool** (absent from both **Modeled tools** and **Outside the model** below) stays in the model for hard-deny and store checks (§4): every word counts as a candidate `Path` with conservative `ReadWrite` access, the unit counts as a possible writer (§5), and it is never eligible for Home's read-only list. Under a covering prefix/exact Allow rule meeting §7's simple-command conditions, Clear targets remain Clear.
- **`AnyExceptForbidden`** applies **only** to `HOME_READONLY_COMMANDS` rows without complete `dev` flag tables. Listed flags (and long abbreviations matching a listed writing flag) keep their stated effects; any unlisted flag keeps `dev`'s conservative treatment (`ReadWrite` candidate-path check on every unclassified word and attached `--opt=value`).
- **Home-eligibility restrictions (`home_readonly.forbidden` / `home_readonly.only`):** forbidden flags mirror `dev`'s `engine::forbidden_flag` (short clusters and long abbreviations of `>= 3` chars including `--`). On an `AnyExceptForbidden` row, a Home-forbidden flag **only removes Home read-only eligibility** (`home_readonly_eligible = false`, so Home asks as on `dev`); it does **not** make the unit `Unread`. The unit stays in the model with that flag's declared effect (e.g. `tree -o out.txt` writes `out.txt`, `git diff --output out.patch` writes `out.patch`, `git diff --ext-diff` runs inline code → `Unread`), so a covering prefix/exact Allow rule can still cover it unless its declared effect triggers `Unread`.
- **Rows outside `HOME_READONLY_COMMANDS`** (and search tools with complete `dev` flag tables) use **`ListedOnly`**: an unlisted flag makes the unit `Unread(UnknownFlag)` and falls back to conservative `ReadWrite` candidate-path checks on unclassified words.
- An **explicitly excluded form** makes its unit **Unread** under every setting (including prefix/exact rules): every entry in **Outside the model**, any unlisted flag on a `ListedOnly` row, or any modeled flag/form triggering an explicit `Unread` effect (§3–§6).

## Modeled tools

| Tool | Platforms | Flags | Operands | Effects & pinning | Home read-only | Changes default (§11) |
|---|---|---|---|---|---|---|
| `command` | Linux, macOS | ListedOnly: none (any flag → Unread) | — | access: AllRead; links(root=Y, below=N); wrapper: Command | No | §11 item 2; §11 item 10 |
| `exec` | Linux, macOS | ListedOnly: none (any flag → Unread) | — | access: AllRead; links(root=Y, below=N); wrapper: Exec | No | §11 item 2; §11 item 10 |
| `nice` | Linux, macOS | ListedOnly: `-n` (Text) | — | access: AllRead; links(root=Y, below=N); wrapper: Nice | No | §11 item 2; §11 item 9; §11 item 10 |
| `timeout` | Linux, macOS | ListedOnly: `-s` (Text), `-k` (Text) | [0] Text | access: AllRead; links(root=Y, below=N); wrapper: Timeout | No | §11 item 2; §11 item 9; §11 item 10 |
| `time` | Linux, macOS | ListedOnly: none (any flag → Unread) | — | access: AllRead; links(root=Y, below=N); wrapper: Time | No | §11 item 2; §11 item 10 |
| `env` | Linux, macOS | ListedOnly: none (any flag → Unread) | — | access: AllRead; links(root=Y, below=N); wrapper: Env | No | §11 item 2; §11 item 10 |
| `sh` | Linux, macOS | ListedOnly: `-e`, `-u`, `-x`, `-c` (Code) [runs-code] | [0] ProgramFile, rest: Path | access: AllRead; writes; links(root=Y, below=N); code: program/inline (pin: FirstOperand); unpinned-prefix-invalid; nested-shell | No | §11 item 2; §11 item 4; §11 item 8 |
| `bash` | Linux, macOS | ListedOnly: `-e`, `-u`, `-x`, `-c` (Code) [runs-code] | [0] ProgramFile, rest: Path | access: AllRead; writes; links(root=Y, below=N); code: program/inline (pin: FirstOperand); unpinned-prefix-invalid; nested-shell | No | §11 item 2; §11 item 4; §11 item 8 |
| `dash` | Linux, macOS | ListedOnly: `-e`, `-u`, `-x`, `-c` (Code) [runs-code] | [0] ProgramFile, rest: Path | access: AllRead; writes; links(root=Y, below=N); code: program/inline (pin: FirstOperand); unpinned-prefix-invalid; nested-shell | No | §11 item 2; §11 item 4; §11 item 8 |
| `ls` | Linux, macOS | AnyExceptForbidden: any flag (Home forbids listed flags) | rest: Path | access: AllRead; links(root=Y, below=N) | Yes | — |
| `pwd` | Linux, macOS | AnyExceptForbidden: any flag (Home forbids listed flags) | rest: Path | access: AllRead; links(root=Y, below=N) | Yes | — |
| `cat` | Linux, macOS | AnyExceptForbidden: any flag (Home forbids listed flags) | rest: Path | access: AllRead; links(root=Y, below=N) | Yes | — |
| `head` | Linux, macOS | AnyExceptForbidden: any flag (Home forbids listed flags) | rest: Path | access: AllRead; links(root=Y, below=N) | Yes | — |
| `tail` | Linux, macOS | AnyExceptForbidden: any flag (Home forbids listed flags) | rest: Path | access: AllRead; links(root=Y, below=N) | Yes | — |
| `wc` | Linux, macOS | AnyExceptForbidden: any flag (Home forbids listed flags) | rest: Path | access: AllRead; links(root=Y, below=N) | Yes | — |
| `file` | Linux, macOS | AnyExceptForbidden: any flag (Home forbids listed flags) | rest: Path | access: AllRead; links(root=Y, below=N) | Yes (forbids -C, --compile, -m, --magic-file, -f, --files-from) | — |
| `stat` | Linux, macOS | AnyExceptForbidden: any flag (Home forbids listed flags) | rest: Path | access: AllRead; links(root=N, below=N) | Yes | — |
| `du` | Linux, macOS | AnyExceptForbidden: any flag (Home forbids listed flags) | rest: Path | access: AllRead; links(root=Y, below=N) | Yes | — |
| `df` | Linux, macOS | AnyExceptForbidden: any flag (Home forbids listed flags) | rest: Path | access: AllRead; links(root=Y, below=N) | Yes | — |
| `which` | Linux, macOS | AnyExceptForbidden: any flag (Home forbids listed flags) | rest: Text | access: AllRead; links(root=Y, below=N) | Yes | §11 item 9 |
| `uname` | Linux, macOS | AnyExceptForbidden: any flag (Home forbids listed flags) | rest: Path | access: AllRead; links(root=Y, below=N) | Yes | — |
| `date` | Linux, macOS | AnyExceptForbidden: any flag (Home forbids listed flags) | rest: Path | access: AllRead; links(root=Y, below=N) | Yes (forbids -s, --set, -f, --file) | — |
| `whoami` | Linux, macOS | AnyExceptForbidden: any flag (Home forbids listed flags) | rest: Path | access: AllRead; links(root=Y, below=N) | Yes | — |
| `tree` | Linux, macOS | AnyExceptForbidden: `-o/--output` (Path) [writes:+; val-access:Write] | rest: Path | access: AllRead; writes (flag); links(root=Y, below=N) | Yes (forbids -o, --output, -R) | — |
| `rg` | Linux, macOS | ListedOnly: 83 listed flags (53 bool, 30 value; see flag table below) | [0] Pattern, rest: Path | access: AllRead; recursive; links(root=Y, below=N); filter: RgGlob | Yes (forbids --pre, --pre-glob, -z, --search-zip, --hostname-bin, -f, --file) [recursive] [allows followed search (§11 item 15)] | §11 item 2; §11 item 9; §11 item 15 |
| `grep` | Linux, macOS | ListedOnly: 56 listed flags (40 bool, 16 value; see flag table below) | [0] Pattern, rest: Path | access: AllRead; recursive (flag); links(root=Y, below=N); filter: GrepInclude | Yes (forbids -f, --file) [recursive w/ flags] [allows followed search (§11 item 15)] | §11 item 2; §11 item 9; §11 item 15 |
| `egrep` | Linux, macOS | ListedOnly: 56 listed flags (40 bool, 16 value; see flag table below) | [0] Pattern, rest: Path | access: AllRead; recursive (flag); links(root=Y, below=N); filter: GrepInclude | No | §11 item 2; §11 item 9 |
| `fgrep` | Linux, macOS | ListedOnly: 56 listed flags (40 bool, 16 value; see flag table below) | [0] Pattern, rest: Path | access: AllRead; recursive (flag); links(root=Y, below=N); filter: GrepInclude | No | §11 item 2; §11 item 9 |
| `zgrep` | Linux, macOS | ListedOnly: 56 listed flags (40 bool, 16 value; see flag table below) | [0] Pattern, rest: Path | access: AllRead; recursive (flag); links(root=Y, below=N); filter: GrepInclude | No | §11 item 2; §11 item 9 |
| `ggrep` | Linux, macOS | ListedOnly: 56 listed flags (40 bool, 16 value; see flag table below) | [0] Pattern, rest: Path | access: AllRead; recursive (flag); links(root=Y, below=N); filter: GrepInclude | No | §11 item 2; §11 item 9 |
| `rgrep` | Linux, macOS | ListedOnly: 56 listed flags (40 bool, 16 value; see flag table below) | [0] Pattern, rest: Path | access: AllRead; recursive; links(root=Y, below=N); filter: GrepInclude | No | §11 item 2; §11 item 9 |
| `ag` | Linux, macOS | ListedOnly: 79 listed flags (62 bool, 17 value; see flag table below) | [0] Pattern, rest: Path | access: AllRead; recursive; links(root=Y, below=N); code: flag; unpinned-prefix-invalid | No | §11 item 2; §11 item 4; §11 item 9 |
| `ack` | Linux, macOS | ListedOnly: 41 listed flags (31 bool, 10 value; see flag table below) | [0] Pattern, rest: Path | access: AllRead; recursive; links(root=Y, below=N); code: flag; unpinned-prefix-invalid | No | §11 item 2; §11 item 4; §11 item 9 |
| `git` | Linux, macOS | ListedOnly: `-C` (Path) [val-access:Read; unread(BaseDirChange)], `-c` (Code) [runs-code], `--config-env` (Code) [runs-code], `--exec-path` (Code) [runs-code], `--git-dir` (Path) [val-access:Read; unread(BaseDirChange)], `--work-tree` (Path) [val-access:Read; unread(BaseDirChange)], `--namespace` (Text), `--super-prefix` (Text) [unread(BaseDirChange)] | — | access: AllRead; links(root=Y, below=N); code: flag; unpinned-prefix-invalid | No | §11 item 2; §11 item 4; §11 item 9 |
| `cargo` | Linux, macOS | ListedOnly: none (any flag → Unread) | — | access: AllRead; writes; links(root=Y, below=N); code: build (pin: CommandAndSubcommand); unpinned-prefix-invalid | No | §11 item 2; §11 item 4; §11 item 8 |
| `pipx` | Linux, macOS | ListedOnly: none (any flag → Unread) | — | access: AllRead; writes; links(root=Y, below=N); code: subarg run; unpinned-prefix-invalid | No | §11 item 2; §11 item 4; §11 item 8 |
| `cd` | Linux, macOS | ListedOnly: `-L`, `-P`, `-e`, `-@` | [0] Path | access: AllRead; links(root=Y, below=N) | No | §11 item 2 |
| `find` | Linux, macOS | ListedOnly: 22 listed flags (10 bool, 12 value; see flag table below) | rest: Path | access: InPlaceWithFlag; writes (flag); derived-dest (flag); recursive; links(root=N, below=N); code: flag; unpinned-prefix-invalid | No | §11 item 2; §11 item 4; §11 item 9; §11 item 10 |
| `ln` | Linux, macOS | ListedOnly: `-s` | [0] Path, [1] Path | access: SourcesReadLastWrite; writes; links(root=N, below=N) | No | §11 item 2 |
| `cp` | Linux, macOS | ListedOnly: `-r` [derived-dest:+; recursive:+], `-R/--recursive` [derived-dest:+; recursive:+], `-a/--archive` [derived-dest:+; recursive:+; links(root=-, below=-)], `-L/--dereference` [links(root=+, below=+)], `-P` [links(root=-, below=-)], `-d` [links(root=-, below=-)] | rest: Path, last: DestinationRoot | access: SourcesReadLastWrite; writes; derived-dest (flag); recursive (flag); links(root=Y, below=N) | No | §11 item 2; §11 item 10 |
| `mv` | Linux, macOS | ListedOnly: none (any flag → Unread) | rest: Path, last: DestinationRoot | access: AllWrite; writes; derived-dest (dir src); links(root=N, below=N) | No | §11 item 2; §11 item 10 |
| `rsync` | Linux, macOS | ListedOnly: `-L/--copy-links` [links(root=+, below=+)], `-k/--copy-dirlinks` [links(root=+, below=+)], `--copy-unsafe-links` [links(root=+, below=+)] | rest: Path, last: DestinationRoot | access: SourcesReadLastWrite; writes; derived-dest; recursive; links(root=Y, below=N) | No | §11 item 2; §11 item 10 |
| `tar` | Linux, macOS | ListedOnly: `-c/--create` [writes:+; op:Write; positional-role(Path)], `-r/--append` [writes:+; op:Write; positional-role(Path)], `-u/--update` [writes:+; op:Write; positional-role(Path)], `-x/--extract` [writes:+; op:Extract; derived-dest:+; positional-role(ArchiveMember)], `-t/--list` [op:List; positional-role(ArchiveMember)], `-h/--dereference` [links(root=+, below=+)], `-f/--file` (Path) [val-access:TarArchive], `-C/--directory` (Path) [val-access:TarDirectory; unread(BaseDirChange)] | rest: Path | access: AllRead; writes (flag); derived-dest (flag); recursive; links(root=N, below=N) | No | §11 item 2; §11 item 9; §11 item 10 |
| `unzip` | Linux, macOS | ListedOnly: `-d` (DestinationRoot) [writes:+; val-access:Write] | [0] Path, rest: ArchiveMember | access: AllRead; writes; derived-dest; links(root=Y, below=N) | No | §11 item 2; §11 item 9; §11 item 10 |
| `rm` | Linux, macOS | ListedOnly: `-r/--recursive` [recursive:+], `-R` [recursive:+] | rest: Path | access: AllWrite; writes; recursive (flag); links(root=N, below=N) | No | §11 item 2 |
| `tee` | Linux, macOS | ListedOnly: none (any flag → Unread) | rest: Path | access: AllWrite; writes; links(root=Y, below=N) | No | §11 item 2 |
| `touch` | Linux, macOS | ListedOnly: none (any flag → Unread) | rest: Path | access: AllWrite; writes; links(root=Y, below=N) | No | §11 item 2 |
| `chmod` | Linux, macOS | ListedOnly: none (any flag → Unread) | [0] Text, rest: Path | access: AllWrite; writes; links(root=Y, below=N) | No | §11 item 2; §11 item 9 |
| `chown` | Linux, macOS | ListedOnly: none (any flag → Unread) | [0] Text, rest: Path | access: AllWrite; writes; links(root=Y, below=N) | No | §11 item 2; §11 item 9 |
| `chgrp` | Linux, macOS | ListedOnly: none (any flag → Unread) | [0] Text, rest: Path | access: AllWrite; writes; links(root=Y, below=N) | No | §11 item 2; §11 item 9 |
| `truncate` | Linux, macOS | ListedOnly: none (any flag → Unread) | rest: Path | access: AllWrite; writes; links(root=Y, below=N) | No | §11 item 2 |
| `install` | Linux, macOS | ListedOnly: none (any flag → Unread) | rest: Path | access: SourcesReadLastWrite; writes; links(root=Y, below=N) | No | §11 item 2 |
| `mkdir` | Linux, macOS | ListedOnly: none (any flag → Unread) | rest: Path | access: AllWrite; writes; links(root=Y, below=N) | No | §11 item 2 |
| `rmdir` | Linux, macOS | ListedOnly: none (any flag → Unread) | rest: Path | access: AllWrite; writes; links(root=N, below=N) | No | §11 item 2 |
| `unlink` | Linux, macOS | ListedOnly: none (any flag → Unread) | rest: Path | access: AllWrite; writes; links(root=N, below=N) | No | §11 item 2 |
| `shred` | Linux, macOS | ListedOnly: none (any flag → Unread) | rest: Path | access: AllWrite; writes; links(root=Y, below=N) | No | §11 item 2 |
| `patch` | Linux, macOS | ListedOnly: none (any flag → Unread) | rest: Path | access: AllWrite; writes; links(root=Y, below=N) | No | §11 item 2 |
| `wipefs` | Linux | ListedOnly: none (any flag → Unread) | rest: Path | access: AllWrite; writes; links(root=Y, below=N) | No | §11 item 2 |
| `blkdiscard` | Linux | ListedOnly: none (any flag → Unread) | rest: Path | access: AllWrite; writes; links(root=Y, below=N) | No | §11 item 2 |
| `ed` | Linux, macOS | ListedOnly: none (any flag → Unread) | rest: Path | access: AllWrite; writes; links(root=Y, below=N) | No | §11 item 2 |
| `ex` | Linux, macOS | ListedOnly: none (any flag → Unread) | rest: Path | access: AllWrite; writes; links(root=Y, below=N) | No | §11 item 2 |
| `diff` | Linux, macOS | ListedOnly: none (any flag → Unread) | rest: Path | access: AllRead; links(root=Y, below=N) | No | §11 item 2 |
| `cmp` | Linux, macOS | ListedOnly: none (any flag → Unread) | rest: Path | access: AllRead; links(root=Y, below=N) | No | §11 item 2 |
| `md5` | macOS | ListedOnly: none (any flag → Unread) | rest: Path | access: AllRead; links(root=Y, below=N) | No | §11 item 2 |
| `md5sum` | Linux | ListedOnly: none (any flag → Unread) | rest: Path | access: AllRead; links(root=Y, below=N) | No | §11 item 2 |
| `shasum` | Linux, macOS | ListedOnly: none (any flag → Unread) | rest: Path | access: AllRead; links(root=Y, below=N) | No | §11 item 2 |
| `sha1sum` | Linux, macOS | ListedOnly: none (any flag → Unread) | rest: Path | access: AllRead; links(root=Y, below=N) | No | §11 item 2 |
| `sha256sum` | Linux, macOS | ListedOnly: none (any flag → Unread) | rest: Path | access: AllRead; links(root=Y, below=N) | No | §11 item 2 |
| `readlink` | Linux, macOS | ListedOnly: none (any flag → Unread) | rest: Path | access: AllRead; links(root=N, below=N) | No | §11 item 2 |
| `realpath` | Linux, macOS | ListedOnly: none (any flag → Unread) | rest: Path | access: AllRead; links(root=Y, below=Y) | No | §11 item 2 |
| `basename` | Linux, macOS | ListedOnly: none (any flag → Unread) | rest: Path | access: AllRead; links(root=N, below=N) | No | §11 item 2 |
| `dirname` | Linux, macOS | ListedOnly: none (any flag → Unread) | rest: Path | access: AllRead; links(root=N, below=N) | No | §11 item 2 |
| `nl` | Linux, macOS | ListedOnly: none (any flag → Unread) | rest: Path | access: AllRead; links(root=Y, below=N) | No | §11 item 2 |
| `od` | Linux, macOS | ListedOnly: none (any flag → Unread) | rest: Path | access: AllRead; links(root=Y, below=N) | No | §11 item 2 |
| `hexdump` | Linux, macOS | ListedOnly: none (any flag → Unread) | rest: Path | access: AllRead; links(root=Y, below=N) | No | §11 item 2 |
| `strings` | Linux, macOS | ListedOnly: none (any flag → Unread) | rest: Path | access: AllRead; links(root=Y, below=N) | No | §11 item 2 |
| `bat` | Linux, macOS | ListedOnly: none (any flag → Unread) | rest: Path | access: AllRead; links(root=Y, below=N) | No | §11 item 2 |
| `column` | Linux, macOS | ListedOnly: none (any flag → Unread) | rest: Path | access: AllRead; links(root=Y, below=N) | No | §11 item 2 |
| `fold` | Linux, macOS | ListedOnly: none (any flag → Unread) | rest: Path | access: AllRead; links(root=Y, below=N) | No | §11 item 2 |
| `expand` | Linux, macOS | ListedOnly: none (any flag → Unread) | rest: Path | access: AllRead; links(root=Y, below=N) | No | §11 item 2 |
| `less` | Linux, macOS | ListedOnly: none (any flag → Unread) | rest: Path | access: AllRead; links(root=Y, below=N) | No | §11 item 2 |
| `more` | Linux, macOS | ListedOnly: none (any flag → Unread) | rest: Path | access: AllRead; links(root=Y, below=N) | No | §11 item 2 |
| `cut` | Linux, macOS | ListedOnly: none (any flag → Unread) | rest: Path | access: AllRead; links(root=Y, below=N) | No | §11 item 2 |
| `sort` | Linux, macOS | ListedOnly: `-o/--output` (Path) [writes:+; val-access:Write] | rest: Path | access: AllRead; writes (flag); links(root=Y, below=N) | No | §11 item 2 |
| `uniq` | Linux, macOS | ListedOnly: none (any flag → Unread) | [0] Path, [1] Path | access: SourcesReadLastWrite; writes (>= 2 args); links(root=Y, below=N) | No | §11 item 2 |
| `xxd` | Linux, macOS | ListedOnly: none (any flag → Unread) | [0] Path, [1] Path | access: SourcesReadLastWrite; writes (>= 2 args); links(root=Y, below=N) | No | §11 item 2 |
| `echo` | Linux, macOS | ListedOnly: `-n`, `-e`, `-E` | rest: Text | access: AllRead; links(root=Y, below=N) | No | §11 item 2; §11 item 9 |
| `printf` | Linux, macOS | ListedOnly: `-v` (Text) [unread(FlagEffect)] | rest: Text | access: AllRead; links(root=Y, below=N) | No | §11 item 2; §11 item 9 |
| `whereis` | Linux, macOS | ListedOnly: none (any flag → Unread) | rest: Text | access: AllRead; links(root=Y, below=N) | No | §11 item 2; §11 item 9 |
| `type` | Linux, macOS | ListedOnly: none (any flag → Unread) | rest: Text | access: AllRead; links(root=Y, below=N) | No | §11 item 2; §11 item 9 |
| `tr` | Linux, macOS | ListedOnly: none (any flag → Unread) | rest: Text | access: AllRead; links(root=Y, below=N) | No | §11 item 2; §11 item 9 |
| `true` | Linux, macOS | ListedOnly: none (any flag → Unread) | rest: Text | access: AllRead; links(root=Y, below=N) | No | §11 item 2; §11 item 9 |
| `false` | Linux, macOS | ListedOnly: none (any flag → Unread) | rest: Text | access: AllRead; links(root=Y, below=N) | No | §11 item 2; §11 item 9 |
| `id` | Linux, macOS | ListedOnly: none (any flag → Unread) | rest: Text | access: AllRead; links(root=Y, below=N) | No | §11 item 2; §11 item 9 |
| `test` | Linux, macOS | ListedOnly: none (any flag → Unread) | rest: Path | access: AllRead; links(root=Y, below=N) | No | §11 item 2 |
| `[` | Linux, macOS | ListedOnly: none (any flag → Unread) | rest: Path | access: AllRead; links(root=Y, below=N) | No | §11 item 2 |
| `jq` | Linux, macOS | ListedOnly: none (any flag → Unread) | [0] Pattern, rest: Path | access: AllRead; links(root=Y, below=N) | No | §11 item 2; §11 item 9 |
| `mdls` | macOS | ListedOnly: none (any flag → Unread) | rest: Path | access: AllRead; links(root=Y, below=N) | No | §11 item 2 |
| `open` | macOS | ListedOnly: none (any flag → Unread) | rest: Path | access: AllRead; links(root=Y, below=N) | No | §11 item 2 |
| `spctl` | macOS | ListedOnly: none (any flag → Unread) | rest: Path | access: AllRead; links(root=Y, below=N) | No | §11 item 2 |
| `mdfind` | macOS | ListedOnly: none (any flag → Unread) | rest: Path | access: AllRead; links(root=Y, below=N) | No | §11 item 2 |
| `codesign` | macOS | ListedOnly: `-s/--sign` (Text) [writes:+; writes-inputs:+], `-f/--force` [writes:+; writes-inputs:+], `--remove-signature` [writes:+; writes-inputs:+], `-d/--display`, `-v/--verify` | rest: Path | access: InPlaceWithFlag; writes (flag); links(root=Y, below=N) | No | §11 item 2; §11 item 9 |
| `awk` | Linux, macOS | ListedOnly: `-i/--include` (Code) [writes:+; runs-code] | [0] Code, rest: Path | access: AllRead; writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 2; §11 item 4 |
| `gawk` | Linux, macOS | ListedOnly: `-i/--include` (Code) [writes:+; runs-code] | [0] Code, rest: Path | access: AllRead; writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 2; §11 item 4 |
| `mawk` | Linux, macOS | ListedOnly: `-i/--include` (Code) [writes:+; runs-code] | [0] Code, rest: Path | access: AllRead; writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 2; §11 item 4 |
| `sed` | Linux, macOS | ListedOnly: `-i/--in-place` [writes:+; writes-inputs:+], `-e/--expression` (Code) [provides-script], `-f/--file` (ProgramFile) [writes:+; val-access:Read; provides-script; runs-program(FlagValue)] | [0] Code, rest: Path | access: InPlaceWithFlag; writes (flag); links(root=Y, below=N); code: script commands (e/w/r); unpinned-prefix-invalid | No | §11 item 2; §11 item 4; §11 item 8 |
| `perl` | Linux, macOS | ListedOnly: `-i` [writes-inputs:+], `-e` (Code) [provides-script; runs-code], `-E` (Code) [provides-script; runs-code] | [0] ProgramFile, rest: Path | access: InPlaceWithFlag; writes; links(root=Y, below=N); code: program/inline (pin: FirstOperand); unpinned-prefix-invalid | No | §11 item 2; §11 item 4; §11 item 8 |
| `python` | Linux, macOS | ListedOnly: `-c` (Code) [runs-code], `-m` (Code) [runs-code] | [0] ProgramFile, rest: Path | access: AllRead; writes; links(root=Y, below=N); code: program/inline (pin: FirstOperand); unpinned-prefix-invalid | No | §11 item 2; §11 item 4; §11 item 8 |
| `python3` | Linux, macOS | ListedOnly: `-c` (Code) [runs-code], `-m` (Code) [runs-code] | [0] ProgramFile, rest: Path | access: AllRead; writes; links(root=Y, below=N); code: program/inline (pin: FirstOperand); unpinned-prefix-invalid | No | §11 item 2; §11 item 4; §11 item 8 |
| `node` | Linux, macOS | ListedOnly: `-e/--eval` (Code) [runs-code], `-p/--print` (Code) [runs-code] | [0] ProgramFile, rest: Path | access: AllRead; writes; links(root=Y, below=N); code: program/inline (pin: FirstOperand); unpinned-prefix-invalid | No | §11 item 2; §11 item 4; §11 item 8 |
| `ruby` | Linux, macOS | ListedOnly: `-e` (Code) [runs-code] | [0] ProgramFile, rest: Path | access: AllRead; writes; links(root=Y, below=N); code: program/inline (pin: FirstOperand); unpinned-prefix-invalid | No | §11 item 2; §11 item 4; §11 item 8 |
| `eval` | Linux, macOS | ListedOnly: none (any flag → Unread) | rest: Code | access: AllRead; writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 2; §11 item 4 |
| `source` | Linux, macOS | ListedOnly: none (any flag → Unread) | [0] ProgramFile, rest: Path | access: AllRead; writes; links(root=Y, below=N); code: program/inline (pin: FirstOperand); unpinned-prefix-invalid | No | §11 item 2; §11 item 4; §11 item 8 |
| `.` | Linux, macOS | ListedOnly: none (any flag → Unread) | [0] ProgramFile, rest: Path | access: AllRead; writes; links(root=Y, below=N); code: program/inline (pin: FirstOperand); unpinned-prefix-invalid | No | §11 item 2; §11 item 4; §11 item 8 |
| `npx` | Linux, macOS | ListedOnly: none (any flag → Unread) | [0] ProgramFile, rest: Path | access: AllRead; writes; links(root=Y, below=N); code: program/inline (pin: FirstOperand); unpinned-prefix-invalid | No | §11 item 2; §11 item 4; §11 item 8 |
| `xargs` | Linux, macOS | ListedOnly: none (any flag → Unread) | rest: Code | access: AllRead; writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 2; §11 item 4 |
| `ssh` | Linux, macOS | ListedOnly: none (any flag → Unread) | [0] Text, rest: Code | access: AllRead; writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 2; §11 item 4; §11 item 9 |
| `mosh` | Linux, macOS | ListedOnly: none (any flag → Unread) | [0] Text, rest: Code | access: AllRead; writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 2; §11 item 4; §11 item 9 |
| `autossh` | Linux, macOS | ListedOnly: none (any flag → Unread) | [0] Text, rest: Code | access: AllRead; writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 2; §11 item 4; §11 item 9 |
| `nsenter` | Linux | ListedOnly: none (any flag → Unread) | rest: Code | access: AllRead; writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 2; §11 item 4 |
| `unshare` | Linux | ListedOnly: none (any flag → Unread) | rest: Code | access: AllRead; writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 2; §11 item 4 |
| `docker` | Linux, macOS | ListedOnly: none (any flag → Unread) | [0] Text, rest: Code | access: AllRead; writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 2; §11 item 4; §11 item 9 |
| `podman` | Linux, macOS | ListedOnly: none (any flag → Unread) | [0] Text, rest: Code | access: AllRead; writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 2; §11 item 4; §11 item 9 |
| `nerdctl` | Linux, macOS | ListedOnly: none (any flag → Unread) | [0] Text, rest: Code | access: AllRead; writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 2; §11 item 4; §11 item 9 |
| `kubectl` | Linux, macOS | ListedOnly: none (any flag → Unread) | [0] Text, rest: Code | access: AllRead; writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 2; §11 item 4; §11 item 9 |
| `oc` | Linux, macOS | ListedOnly: none (any flag → Unread) | [0] Text, rest: Code | access: AllRead; writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 2; §11 item 4; §11 item 9 |
| `lxc` | Linux, macOS | ListedOnly: none (any flag → Unread) | [0] Text, rest: Code | access: AllRead; writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 2; §11 item 4; §11 item 9 |
| `incus` | Linux, macOS | ListedOnly: none (any flag → Unread) | [0] Text, rest: Code | access: AllRead; writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 2; §11 item 4; §11 item 9 |
| `machinectl` | Linux | ListedOnly: none (any flag → Unread) | [0] Text, rest: Code | access: AllRead; writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 2; §11 item 4; §11 item 9 |
| `systemd-run` | Linux | ListedOnly: none (any flag → Unread) | rest: Code | access: AllRead; writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 2; §11 item 4 |
| `systemd-nspawn` | Linux | ListedOnly: none (any flag → Unread) | rest: Code | access: AllRead; writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 2; §11 item 4 |
| `firejail` | Linux | ListedOnly: none (any flag → Unread) | rest: Code | access: AllRead; writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 2; §11 item 4 |
| `bwrap` | Linux | ListedOnly: none (any flag → Unread) | rest: Code | access: AllRead; writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 2; §11 item 4 |
| `flatpak-spawn` | Linux | ListedOnly: none (any flag → Unread) | rest: Code | access: AllRead; writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 2; §11 item 4 |
| `toolbox` | Linux, macOS | ListedOnly: none (any flag → Unread) | [0] Text, rest: Code | access: AllRead; writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 2; §11 item 4; §11 item 9 |
| `distrobox` | Linux, macOS | ListedOnly: none (any flag → Unread) | [0] Text, rest: Code | access: AllRead; writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 2; §11 item 4; §11 item 9 |
| `distrobox-enter` | Linux, macOS | ListedOnly: none (any flag → Unread) | [0] Text, rest: Code | access: AllRead; writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 2; §11 item 4; §11 item 9 |
| `multipass` | Linux, macOS | ListedOnly: none (any flag → Unread) | [0] Text, rest: Code | access: AllRead; writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 2; §11 item 4; §11 item 9 |
| `vagrant` | Linux, macOS | ListedOnly: none (any flag → Unread) | [0] Text, rest: Code | access: AllRead; writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 2; §11 item 4; §11 item 9 |
| `limactl` | Linux, macOS | ListedOnly: none (any flag → Unread) | [0] Text, rest: Code | access: AllRead; writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 2; §11 item 4; §11 item 9 |
| `colima` | Linux, macOS | ListedOnly: none (any flag → Unread) | [0] Text, rest: Code | access: AllRead; writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 2; §11 item 4; §11 item 9 |
| `runuser` | Linux | ListedOnly: none (any flag → Unread) | [0] Text, rest: Code | access: AllRead; writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 2; §11 item 4; §11 item 9 |
| `setpriv` | Linux | ListedOnly: none (any flag → Unread) | rest: Code | access: AllRead; writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 2; §11 item 4 |
| `parallel` | Linux, macOS | ListedOnly: none (any flag → Unread) | rest: Code | access: AllRead; writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 2; §11 item 4 |
| `sudo` | Linux, macOS | ListedOnly: none (any flag → Unread) | rest: Code | access: AllRead; writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid; hard-deny: `privilege-escalation` | No | — |
| `sudoedit` | Linux, macOS | ListedOnly: none (any flag → Unread) | rest: Path | access: AllWrite; writes; links(root=Y, below=N); hard-deny: `privilege-escalation` | No | — |
| `su` | Linux, macOS | ListedOnly: none (any flag → Unread) | rest: Code | access: AllRead; writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid; hard-deny: `privilege-escalation` | No | — |
| `doas` | Linux, macOS | ListedOnly: none (any flag → Unread) | rest: Code | access: AllRead; writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid; hard-deny: `privilege-escalation` | No | — |
| `pkexec` | Linux | ListedOnly: none (any flag → Unread) | rest: Code | access: AllRead; writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid; hard-deny: `privilege-escalation` | No | — |
| `run0` | Linux | ListedOnly: none (any flag → Unread) | rest: Code | access: AllRead; writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid; hard-deny: `privilege-escalation` | No | — |
| `curl` | Linux, macOS | ListedOnly: none (any flag → Unread) | rest: Path | access: AllWrite; writes; links(root=Y, below=N); downloader | No | §11 item 2 |
| `wget` | Linux, macOS | ListedOnly: none (any flag → Unread) | rest: Path | access: AllWrite; writes; links(root=Y, below=N); downloader | No | §11 item 2 |
| `fetch` | Linux, macOS | ListedOnly: none (any flag → Unread) | rest: Path | access: AllWrite; writes; links(root=Y, below=N); downloader | No | §11 item 2 |

## Subcommands

### `git`

| Subcommand | Flags | Operands | Effects & pinning | Home read-only | Changes default (§11) |
|---|---|---|---|---|---|
| `git status` | AnyExceptForbidden: any flag (Home forbids listed flags) | rest: Path | access: AllRead; links(root=Y, below=N) | Yes (forbids --output, --ext-diff, --exec-path, --config-env, -c) | §11 item 10 |
| `git log` | AnyExceptForbidden: `-p`, `-L/--line-range` (Text) | rest: Ref | access: AllRead; links(root=Y, below=N); git-cred-select | Yes (forbids --output, --ext-diff, --exec-path, --config-env, -c) | §11 item 9; §11 item 10 |
| `git diff` | AnyExceptForbidden: `--output` (Path) [writes:+; val-access:Write], `--ext-diff` [runs-code] | rest: Ref | access: AllRead; writes (flag); links(root=Y, below=N); code: flag; unpinned-prefix-invalid; git-cred-select | Yes (forbids --output, --ext-diff, --exec-path, --config-env, -c) | §11 item 4; §11 item 9; §11 item 10 |
| `git show` | AnyExceptForbidden: any flag (Home forbids listed flags) | rest: Ref | access: AllRead; links(root=Y, below=N); git-cred-select | Yes (forbids --output, --ext-diff, --exec-path, --config-env, -c) | §11 item 9; §11 item 10 |
| `git branch` | AnyExceptForbidden: 14 listed flags (13 bool, 1 value; see flag table below) | rest: Ref | access: AllRead; writes (flag or >= 1 args unless disabled); links(root=Y, below=N) | Yes (forbids --output, --ext-diff, --exec-path, --config-env, -c) (only -a, --all, -r, --remotes, -v, -vv, --verbose, --list, -l, --show-current) | §11 item 9 |
| `git rev-parse` | AnyExceptForbidden: any flag (Home forbids listed flags) | rest: Ref | access: AllRead; links(root=Y, below=N) | Yes (forbids --output, --ext-diff, --exec-path, --config-env, -c) | §11 item 9 |
| `git ls-files` | AnyExceptForbidden: any flag (Home forbids listed flags) | rest: Path | access: AllRead; links(root=Y, below=N) | Yes (forbids --output, --ext-diff, --exec-path, --config-env, -c) | §11 item 10 |
| `git cat-file` | ListedOnly: none (any flag → Unread) | rest: Ref | access: AllRead; links(root=Y, below=N); git-cred-select | No | §11 item 2; §11 item 9 |
| `git push` | ListedOnly: `-f/--force` [git-push(Force)], `--force-with-lease` [git-push(Force)], `--force-if-includes` [git-push(Force)], `--mirror` [git-push(Force)], `--all` [git-push(ImplicitTargets)], `--tags` [git-push(ImplicitTargets)], `--branches` [git-push(ImplicitTargets)], `--prune` [git-push(ImplicitTargets)], `-o/--push-option` (Text), `--receive-pack` (Code) [runs-code], `--exec` (Code) [runs-code] | rest: Ref | access: AllRead; writes; links(root=Y, below=N); code: flag; unpinned-prefix-invalid | No | §11 item 2; §11 item 4; §11 item 9 |
| `git clone` | ListedOnly: none (any flag → Unread) | [0] Ref, [1] DestinationRoot | access: SourcesReadLastWrite; writes; derived-dest; links(root=Y, below=N) | No | §11 item 2; §11 item 9; §11 item 10 |
| `git checkout` | ListedOnly: `-b` (Ref) [writes:+], `-B` (Ref) [writes:+], `--orphan` (Ref) [writes:+] | [0] Ref, rest: DestinationRoot, after `--`: DestinationRoot | access: AllWrite; writes; derived-dest; links(root=Y, below=N) | No | §11 item 2; §11 item 9; §11 item 10 |
| `git merge-index` | ListedOnly: none (any flag → Unread) | rest: Code | access: AllRead; writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 2; §11 item 4 |
| `git filter-branch` | ListedOnly: none (any flag → Unread) | rest: Code | access: AllRead; writes; links(root=Y, below=N); code: inline; unpinned-prefix-invalid | No | §11 item 2; §11 item 4 |
| `git bisect` | ListedOnly: none (any flag → Unread) | [0] Text, rest: Code | access: AllRead; writes; links(root=Y, below=N); code: subarg run; unpinned-prefix-invalid | No | §11 item 2; §11 item 4; §11 item 9 |
| `git rebase` | ListedOnly: `-x/--exec` (Code) [runs-code] | rest: Ref | access: AllRead; writes; links(root=Y, below=N); code: flag; unpinned-prefix-invalid | No | §11 item 2; §11 item 4; §11 item 9 |
| `git submodule` | ListedOnly: none (any flag → Unread) | [0] Text, rest: Code | access: AllRead; writes; links(root=Y, below=N); code: subarg foreach; unpinned-prefix-invalid | No | §11 item 2; §11 item 4; §11 item 9 |
| `git difftool` | ListedOnly: `-x/--tool-cmd` (Code) [runs-code] | rest: Ref | access: AllRead; links(root=Y, below=N); code: flag; unpinned-prefix-invalid | No | §11 item 2; §11 item 4; §11 item 9 |
| `git mergetool` | ListedOnly: `-x/--tool-cmd` (Code) [runs-code] | rest: Path | access: AllWrite; writes; links(root=Y, below=N); code: flag; unpinned-prefix-invalid | No | §11 item 2; §11 item 4 |

### `cargo`

| Subcommand | Flags | Operands | Effects & pinning | Home read-only | Changes default (§11) |
|---|---|---|---|---|---|
| `cargo test` | ListedOnly: none (any flag → Unread) | rest: Text | access: AllRead; writes; links(root=Y, below=N); code: build (pin: CommandAndSubcommand) | No | §11 item 2; §11 item 8; §11 item 9 |
| `cargo build` | ListedOnly: none (any flag → Unread) | rest: Text | access: AllRead; writes; links(root=Y, below=N); code: build (pin: CommandAndSubcommand) | No | §11 item 2; §11 item 8; §11 item 9 |
| `cargo run` | ListedOnly: none (any flag → Unread) | rest: Text | access: AllRead; writes; links(root=Y, below=N); code: build (pin: CommandAndSubcommand) | No | §11 item 2; §11 item 8; §11 item 9 |
| `cargo check` | ListedOnly: none (any flag → Unread) | rest: Text | access: AllRead; writes; links(root=Y, below=N); code: build (pin: CommandAndSubcommand) | No | §11 item 2; §11 item 8; §11 item 9 |
| `cargo bench` | ListedOnly: none (any flag → Unread) | rest: Text | access: AllRead; writes; links(root=Y, below=N); code: build (pin: CommandAndSubcommand) | No | §11 item 2; §11 item 8; §11 item 9 |
| `cargo doc` | ListedOnly: none (any flag → Unread) | rest: Text | access: AllRead; writes; links(root=Y, below=N); code: build (pin: CommandAndSubcommand) | No | §11 item 2; §11 item 8; §11 item 9 |
| `cargo clippy` | ListedOnly: none (any flag → Unread) | rest: Text | access: AllRead; writes; links(root=Y, below=N); code: build (pin: CommandAndSubcommand) | No | §11 item 2; §11 item 8; §11 item 9 |

### `pipx`

| Subcommand | Flags | Operands | Effects & pinning | Home read-only | Changes default (§11) |
|---|---|---|---|---|---|
| `pipx run` | ListedOnly: none (any flag → Unread) | [0] ProgramFile, rest: Path | access: AllRead; writes; links(root=Y, below=N); code: program/inline (pin: FirstOperand); unpinned-prefix-invalid | No | §11 item 2; §11 item 4; §11 item 8 |

## Detailed flag tables

### `nice` (1 flags, ListedOnly)

| Short | Long | Kind | Effects |
|---|---|---|---|
| `-n` | — | Value(Text) | none (scheduling priority increment; allowed wrapper option (§4)) |

### `timeout` (2 flags, ListedOnly)

| Short | Long | Kind | Effects |
|---|---|---|---|
| `-s` | — | Value(Text) | none (termination signal name/number; allowed wrapper option (§4)) |
| `-k` | — | Value(Text) | none (kill-after duration; allowed wrapper option (§4)) |

### `sh` (4 flags, ListedOnly)

| Short | Long | Kind | Effects |
|---|---|---|---|
| `-e` | — | Bool | none (shell errexit option; allowed before -c (§4)) |
| `-u` | — | Bool | none (shell nounset option; allowed before -c (§4)) |
| `-x` | — | Bool | none (shell xtrace option; allowed before -c (§4)) |
| `-c` | — | Value(Code) | runs-code |

### `bash` (4 flags, ListedOnly)

| Short | Long | Kind | Effects |
|---|---|---|---|
| `-e` | — | Bool | none (shell errexit option; allowed before -c (§4)) |
| `-u` | — | Bool | none (shell nounset option; allowed before -c (§4)) |
| `-x` | — | Bool | none (shell xtrace option; allowed before -c (§4)) |
| `-c` | — | Value(Code) | runs-code |

### `dash` (4 flags, ListedOnly)

| Short | Long | Kind | Effects |
|---|---|---|---|
| `-e` | — | Bool | none (shell errexit option; allowed before -c (§4)) |
| `-u` | — | Bool | none (shell nounset option; allowed before -c (§4)) |
| `-x` | — | Bool | none (shell xtrace option; allowed before -c (§4)) |
| `-c` | — | Value(Code) | runs-code |

### `tree` (1 flags, AnyExceptForbidden)

| Short | Long | Kind | Effects |
|---|---|---|---|
| `-o` | `--output` | Value(Path) | writes:+; val-access:Write |

### `rg` (83 flags, ListedOnly)

| Short | Long | Kind | Effects |
|---|---|---|---|
| `-i` | `--ignore-case` | Bool | none (content match case-insensitivity only) |
| `-S` | `--smart-case` | Bool | none (content match smart-case only) |
| `-s` | `--case-sensitive` | Bool | none (content match case-sensitivity only) |
| `-n` | `--line-number` | Bool | none (output formatting: print line numbers) |
| `-N` | `--no-line-number` | Bool | none (output formatting: omit line numbers) |
| `-H` | `--with-filename` | Bool | none (output formatting: print filename) |
| `-I` | `--no-filename` | Bool | none (output formatting: omit filename) |
| `-l` | `--files-with-matches` | Bool | none (output formatting: print matching filenames only) |
| `-c` | `--count` | Bool | none (output formatting: print match count) |
| `-o` | `--only-matching` | Bool | none (output formatting: print matched text only) |
| `-v` | `--invert-match` | Bool | none (content match inversion only) |
| `-w` | `--word-regexp` | Bool | none (content match word boundaries only) |
| `-x` | `--line-regexp` | Bool | none (content match whole lines only) |
| `-u` | `--unrestricted` | Bool | none (disables default ignore/hidden skipping; model checks all entries) |
| `-a` | `--text` | Bool | none (treat binary files as text during match) |
| `-U` | `--multiline` | Bool | none (multiline content match only) |
| `-F` | `--fixed-strings` | Bool | none (literal content match only) |
| `-P` | `--pcre2` | Bool | none (PCRE2 content regex engine only) |
| `-q` | `--quiet` | Bool | none (suppress output) |
| `-0` | `--null` | Bool | none (output formatting: NUL filename separator) |
| `-p` | `--pretty` | Bool | none (output formatting: pretty display) |
| `-b` | `--byte-offset` | Bool | none (output formatting: print byte offset) |
| `-L` | `--follow` | Bool | links(root=+, below=+) |
| — | `--hidden` | Bool | none (searches dotfiles; model already checks all entries) |
| — | `--no-ignore` | Bool | none (disables ignore files; model already checks all entries) |
| — | `--no-ignore-vcs` | Bool | none (disables VCS ignore files; model already checks all entries) |
| — | `--no-ignore-dot` | Bool | none (disables .ignore files; model already checks all entries) |
| — | `--no-ignore-parent` | Bool | none (disables parent ignore files; model already checks all entries) |
| — | `--no-ignore-global` | Bool | none (disables global ignore files; model already checks all entries) |
| — | `--no-ignore-files` | Bool | none (disables --ignore-file entries; model already checks all entries) |
| — | `--no-messages` | Bool | none (suppress error messages) |
| — | `--no-heading` | Bool | none (output formatting: omit file headings) |
| — | `--heading` | Bool | none (output formatting: print file headings) |
| — | `--count-matches` | Bool | none (output formatting: print total match count) |
| — | `--files-without-match` | Bool | none (output formatting: print non-matching filenames) |
| — | `--multiline-dotall` | Bool | none (multiline regex dotall mode only) |
| — | `--binary` | Bool | none (search binary files for content matches) |
| — | `--files` | Bool | provides-pattern |
| — | `--type-list` | Bool | provides-pattern |
| — | `--json` | Bool | none (output formatting: JSON lines) |
| — | `--column` | Bool | none (output formatting: print column number) |
| — | `--no-column` | Bool | none (output formatting: omit column number) |
| — | `--vimgrep` | Bool | none (output formatting: vimgrep format) |
| — | `--trim` | Bool | none (output formatting: trim leading whitespace) |
| — | `--stats` | Bool | none (output formatting: print search statistics) |
| — | `--no-config` | Bool | none (ignore ripgrep config file) |
| — | `--version` | Bool | none (print version and exit) |
| — | `--help` | Bool | none (print help and exit) |
| — | `--no-pcre2` | Bool | none (disable PCRE2 content regex engine) |
| — | `--crlf` | Bool | none (CRLF line terminator handling only) |
| — | `--include-zero` | Bool | none (output formatting: include zero counts) |
| — | `--glob-case-insensitive` | Bool | filter-ci:+ |
| — | `--no-glob-case-insensitive` | Bool | filter-ci:- |
| `-e` | `--regexp` | Value(Pattern) | provides-pattern |
| `-g` | `--glob` | Value(Pattern) | filter(RgGlob, global-toggle) |
| — | `--iglob` | Value(Pattern) | filter(RgGlob, insensitive) |
| `-f` | `--file` | Value(Path) | val-access:Read; provides-pattern |
| `-t` | `--type` | Value(Pattern) | unread(FilterForm) |
| `-T` | `--type-not` | Value(Pattern) | unread(FilterForm) |
| `-m` | `--max-count` | Value(Text) | none (limits matches printed per file) |
| `-A` | `--after-context` | Value(Text) | none (prints trailing context lines) |
| `-B` | `--before-context` | Value(Text) | none (prints leading context lines) |
| `-C` | `--context` | Value(Text) | none (prints surrounding context lines) |
| `-d` | `--max-depth` | Value(Text) | unread(FilterForm) |
| — | `--maxdepth` | Value(Text) | unread(FilterForm) |
| `-j` | `--threads` | Value(Text) | none (sets worker thread count) |
| `-M` | `--max-columns` | Value(Text) | none (truncates long output lines) |
| `-E` | `--encoding` | Value(Text) | none (selects text decoding for content match) |
| `-r` | `--replace` | Value(Text) | none (formats matched output text) |
| — | `--colors` | Value(Text) | none (output color specification) |
| — | `--color` | Value(Text) | none (output color mode) |
| — | `--sort` | Value(Text) | none (sorts output order) |
| — | `--sortr` | Value(Text) | none (reverse-sorts output order) |
| — | `--max-filesize` | Value(Text) | unread(FilterForm) |
| — | `--path-separator` | Value(Text) | none (output path display separator only) |
| — | `--engine` | Value(Text) | none (selects regex engine) |
| — | `--context-separator` | Value(Text) | none (output context separator string) |
| — | `--type-add` | Value(Pattern) | unread(FilterForm) |
| — | `--type-clear` | Value(Pattern) | unread(FilterForm) |
| — | `--field-context-separator` | Value(Text) | none (output field separator string) |
| — | `--field-match-separator` | Value(Text) | none (output field separator string) |
| — | `--dfa-size-limit` | Value(Text) | none (regex DFA memory limit) |
| — | `--regex-size-limit` | Value(Text) | none (regex compilation memory limit) |

### `grep` (56 flags, ListedOnly)

| Short | Long | Kind | Effects |
|---|---|---|---|
| `-i` | `--ignore-case` | Bool | none (content match case-insensitivity only) |
| — | `--no-ignore-case` | Bool | none (content match case-sensitivity only) |
| `-v` | `--invert-match` | Bool | none (content match inversion only) |
| `-n` | `--line-number` | Bool | none (output formatting: print line numbers) |
| `-c` | `--count` | Bool | none (output formatting: print match count) |
| `-l` | `--files-with-matches` | Bool | none (output formatting: print matching filenames only) |
| `-L` | `--files-without-match` | Bool | none (output formatting: print non-matching filenames) |
| `-o` | `--only-matching` | Bool | none (output formatting: print matched text only) |
| `-q` | `--quiet` | Bool | none (suppress output) |
| — | `--silent` | Bool | none (suppress output) |
| `-s` | `--no-messages` | Bool | none (suppress error messages) |
| `-w` | `--word-regexp` | Bool | none (content match word boundaries only) |
| `-x` | `--line-regexp` | Bool | none (content match whole lines only) |
| `-h` | `--no-filename` | Bool | none (output formatting: omit filename) |
| `-H` | `--with-filename` | Bool | none (output formatting: print filename) |
| `-E` | `--extended-regexp` | Bool | none (ERE content regex dialect only) |
| `-F` | `--fixed-strings` | Bool | none (literal content match only) |
| `-G` | `--basic-regexp` | Bool | none (BRE content regex dialect only) |
| `-P` | `--perl-regexp` | Bool | none (PCRE content regex dialect only) |
| `-a` | `--text` | Bool | none (treat binary files as text during match) |
| `-z` | `--null-data` | Bool | none (NUL-delimited input lines only) |
| `-Z` | `--null` | Bool | none (output formatting: NUL filename separator) |
| `-b` | `--byte-offset` | Bool | none (output formatting: print byte offset) |
| `-T` | `--initial-tab` | Bool | none (output formatting: align tab) |
| `-r` | `--recursive` | Bool | recursive:+; links(root==, below=-) |
| `-R` | `--dereference-recursive` | Bool | recursive:+; links(root=+, below=+) |
| — | `--no-group-separator` | Bool | none (output formatting: omit context group separator) |
| `-I` | — | Bool | none (skip binary files in content match only) |
| `-U` | — | Bool | none (binary CR handling on MS-DOS/Windows only) |
| `-y` | — | Bool | none (obsolete alias for -i ignore-case) |
| `-0` | — | Bool | none (digit context-line count shorthand) |
| `-1` | — | Bool | none (digit context-line count shorthand) |
| `-2` | — | Bool | none (digit context-line count shorthand) |
| `-3` | — | Bool | none (digit context-line count shorthand) |
| `-4` | — | Bool | none (digit context-line count shorthand) |
| `-5` | — | Bool | none (digit context-line count shorthand) |
| `-6` | — | Bool | none (digit context-line count shorthand) |
| `-7` | — | Bool | none (digit context-line count shorthand) |
| `-8` | — | Bool | none (digit context-line count shorthand) |
| `-9` | — | Bool | none (digit context-line count shorthand) |
| `-e` | `--regexp` | Value(Pattern) | provides-pattern |
| `-f` | `--file` | Value(Path) | val-access:Read; provides-pattern |
| `-m` | `--max-count` | Value(Text) | none (limits matches printed per file) |
| `-A` | `--after-context` | Value(Text) | none (prints trailing context lines) |
| `-B` | `--before-context` | Value(Text) | none (prints leading context lines) |
| `-C` | `--context` | Value(Text) | none (prints surrounding context lines) |
| `-d` | `--directories` | Value(Text) | recursive:+; unread(FlagEffect) |
| `-D` | `--devices` | Value(Text) | none (device/FIFO handling only; does not change file or directory selection) |
| — | `--include` | Value(Pattern) | filter(GrepInclude, sensitive) |
| — | `--exclude` | Value(Pattern) | unread(FilterForm) |
| — | `--exclude-dir` | Value(Pattern) | unread(FilterForm) |
| — | `--label` | Value(Text) | none (stdin display label in output) |
| — | `--color` | Value(Text) | none (output color mode) |
| — | `--colour` | Value(Text) | none (output color mode) |
| — | `--binary-files` | Value(Text) | none (binary file content match mode only) |
| — | `--group-separator` | Value(Text) | none (output context group separator string) |

### `egrep` (56 flags, ListedOnly)

| Short | Long | Kind | Effects |
|---|---|---|---|
| `-i` | `--ignore-case` | Bool | none (content match case-insensitivity only) |
| — | `--no-ignore-case` | Bool | none (content match case-sensitivity only) |
| `-v` | `--invert-match` | Bool | none (content match inversion only) |
| `-n` | `--line-number` | Bool | none (output formatting: print line numbers) |
| `-c` | `--count` | Bool | none (output formatting: print match count) |
| `-l` | `--files-with-matches` | Bool | none (output formatting: print matching filenames only) |
| `-L` | `--files-without-match` | Bool | none (output formatting: print non-matching filenames) |
| `-o` | `--only-matching` | Bool | none (output formatting: print matched text only) |
| `-q` | `--quiet` | Bool | none (suppress output) |
| — | `--silent` | Bool | none (suppress output) |
| `-s` | `--no-messages` | Bool | none (suppress error messages) |
| `-w` | `--word-regexp` | Bool | none (content match word boundaries only) |
| `-x` | `--line-regexp` | Bool | none (content match whole lines only) |
| `-h` | `--no-filename` | Bool | none (output formatting: omit filename) |
| `-H` | `--with-filename` | Bool | none (output formatting: print filename) |
| `-E` | `--extended-regexp` | Bool | none (ERE content regex dialect only) |
| `-F` | `--fixed-strings` | Bool | none (literal content match only) |
| `-G` | `--basic-regexp` | Bool | none (BRE content regex dialect only) |
| `-P` | `--perl-regexp` | Bool | none (PCRE content regex dialect only) |
| `-a` | `--text` | Bool | none (treat binary files as text during match) |
| `-z` | `--null-data` | Bool | none (NUL-delimited input lines only) |
| `-Z` | `--null` | Bool | none (output formatting: NUL filename separator) |
| `-b` | `--byte-offset` | Bool | none (output formatting: print byte offset) |
| `-T` | `--initial-tab` | Bool | none (output formatting: align tab) |
| `-r` | `--recursive` | Bool | recursive:+; links(root==, below=-) |
| `-R` | `--dereference-recursive` | Bool | recursive:+; links(root=+, below=+) |
| — | `--no-group-separator` | Bool | none (output formatting: omit context group separator) |
| `-I` | — | Bool | none (skip binary files in content match only) |
| `-U` | — | Bool | none (binary CR handling on MS-DOS/Windows only) |
| `-y` | — | Bool | none (obsolete alias for -i ignore-case) |
| `-0` | — | Bool | none (digit context-line count shorthand) |
| `-1` | — | Bool | none (digit context-line count shorthand) |
| `-2` | — | Bool | none (digit context-line count shorthand) |
| `-3` | — | Bool | none (digit context-line count shorthand) |
| `-4` | — | Bool | none (digit context-line count shorthand) |
| `-5` | — | Bool | none (digit context-line count shorthand) |
| `-6` | — | Bool | none (digit context-line count shorthand) |
| `-7` | — | Bool | none (digit context-line count shorthand) |
| `-8` | — | Bool | none (digit context-line count shorthand) |
| `-9` | — | Bool | none (digit context-line count shorthand) |
| `-e` | `--regexp` | Value(Pattern) | provides-pattern |
| `-f` | `--file` | Value(Path) | val-access:Read; provides-pattern |
| `-m` | `--max-count` | Value(Text) | none (limits matches printed per file) |
| `-A` | `--after-context` | Value(Text) | none (prints trailing context lines) |
| `-B` | `--before-context` | Value(Text) | none (prints leading context lines) |
| `-C` | `--context` | Value(Text) | none (prints surrounding context lines) |
| `-d` | `--directories` | Value(Text) | recursive:+; unread(FlagEffect) |
| `-D` | `--devices` | Value(Text) | none (device/FIFO handling only; does not change file or directory selection) |
| — | `--include` | Value(Pattern) | filter(GrepInclude, sensitive) |
| — | `--exclude` | Value(Pattern) | unread(FilterForm) |
| — | `--exclude-dir` | Value(Pattern) | unread(FilterForm) |
| — | `--label` | Value(Text) | none (stdin display label in output) |
| — | `--color` | Value(Text) | none (output color mode) |
| — | `--colour` | Value(Text) | none (output color mode) |
| — | `--binary-files` | Value(Text) | none (binary file content match mode only) |
| — | `--group-separator` | Value(Text) | none (output context group separator string) |

### `fgrep` (56 flags, ListedOnly)

| Short | Long | Kind | Effects |
|---|---|---|---|
| `-i` | `--ignore-case` | Bool | none (content match case-insensitivity only) |
| — | `--no-ignore-case` | Bool | none (content match case-sensitivity only) |
| `-v` | `--invert-match` | Bool | none (content match inversion only) |
| `-n` | `--line-number` | Bool | none (output formatting: print line numbers) |
| `-c` | `--count` | Bool | none (output formatting: print match count) |
| `-l` | `--files-with-matches` | Bool | none (output formatting: print matching filenames only) |
| `-L` | `--files-without-match` | Bool | none (output formatting: print non-matching filenames) |
| `-o` | `--only-matching` | Bool | none (output formatting: print matched text only) |
| `-q` | `--quiet` | Bool | none (suppress output) |
| — | `--silent` | Bool | none (suppress output) |
| `-s` | `--no-messages` | Bool | none (suppress error messages) |
| `-w` | `--word-regexp` | Bool | none (content match word boundaries only) |
| `-x` | `--line-regexp` | Bool | none (content match whole lines only) |
| `-h` | `--no-filename` | Bool | none (output formatting: omit filename) |
| `-H` | `--with-filename` | Bool | none (output formatting: print filename) |
| `-E` | `--extended-regexp` | Bool | none (ERE content regex dialect only) |
| `-F` | `--fixed-strings` | Bool | none (literal content match only) |
| `-G` | `--basic-regexp` | Bool | none (BRE content regex dialect only) |
| `-P` | `--perl-regexp` | Bool | none (PCRE content regex dialect only) |
| `-a` | `--text` | Bool | none (treat binary files as text during match) |
| `-z` | `--null-data` | Bool | none (NUL-delimited input lines only) |
| `-Z` | `--null` | Bool | none (output formatting: NUL filename separator) |
| `-b` | `--byte-offset` | Bool | none (output formatting: print byte offset) |
| `-T` | `--initial-tab` | Bool | none (output formatting: align tab) |
| `-r` | `--recursive` | Bool | recursive:+; links(root==, below=-) |
| `-R` | `--dereference-recursive` | Bool | recursive:+; links(root=+, below=+) |
| — | `--no-group-separator` | Bool | none (output formatting: omit context group separator) |
| `-I` | — | Bool | none (skip binary files in content match only) |
| `-U` | — | Bool | none (binary CR handling on MS-DOS/Windows only) |
| `-y` | — | Bool | none (obsolete alias for -i ignore-case) |
| `-0` | — | Bool | none (digit context-line count shorthand) |
| `-1` | — | Bool | none (digit context-line count shorthand) |
| `-2` | — | Bool | none (digit context-line count shorthand) |
| `-3` | — | Bool | none (digit context-line count shorthand) |
| `-4` | — | Bool | none (digit context-line count shorthand) |
| `-5` | — | Bool | none (digit context-line count shorthand) |
| `-6` | — | Bool | none (digit context-line count shorthand) |
| `-7` | — | Bool | none (digit context-line count shorthand) |
| `-8` | — | Bool | none (digit context-line count shorthand) |
| `-9` | — | Bool | none (digit context-line count shorthand) |
| `-e` | `--regexp` | Value(Pattern) | provides-pattern |
| `-f` | `--file` | Value(Path) | val-access:Read; provides-pattern |
| `-m` | `--max-count` | Value(Text) | none (limits matches printed per file) |
| `-A` | `--after-context` | Value(Text) | none (prints trailing context lines) |
| `-B` | `--before-context` | Value(Text) | none (prints leading context lines) |
| `-C` | `--context` | Value(Text) | none (prints surrounding context lines) |
| `-d` | `--directories` | Value(Text) | recursive:+; unread(FlagEffect) |
| `-D` | `--devices` | Value(Text) | none (device/FIFO handling only; does not change file or directory selection) |
| — | `--include` | Value(Pattern) | filter(GrepInclude, sensitive) |
| — | `--exclude` | Value(Pattern) | unread(FilterForm) |
| — | `--exclude-dir` | Value(Pattern) | unread(FilterForm) |
| — | `--label` | Value(Text) | none (stdin display label in output) |
| — | `--color` | Value(Text) | none (output color mode) |
| — | `--colour` | Value(Text) | none (output color mode) |
| — | `--binary-files` | Value(Text) | none (binary file content match mode only) |
| — | `--group-separator` | Value(Text) | none (output context group separator string) |

### `zgrep` (56 flags, ListedOnly)

| Short | Long | Kind | Effects |
|---|---|---|---|
| `-i` | `--ignore-case` | Bool | none (content match case-insensitivity only) |
| — | `--no-ignore-case` | Bool | none (content match case-sensitivity only) |
| `-v` | `--invert-match` | Bool | none (content match inversion only) |
| `-n` | `--line-number` | Bool | none (output formatting: print line numbers) |
| `-c` | `--count` | Bool | none (output formatting: print match count) |
| `-l` | `--files-with-matches` | Bool | none (output formatting: print matching filenames only) |
| `-L` | `--files-without-match` | Bool | none (output formatting: print non-matching filenames) |
| `-o` | `--only-matching` | Bool | none (output formatting: print matched text only) |
| `-q` | `--quiet` | Bool | none (suppress output) |
| — | `--silent` | Bool | none (suppress output) |
| `-s` | `--no-messages` | Bool | none (suppress error messages) |
| `-w` | `--word-regexp` | Bool | none (content match word boundaries only) |
| `-x` | `--line-regexp` | Bool | none (content match whole lines only) |
| `-h` | `--no-filename` | Bool | none (output formatting: omit filename) |
| `-H` | `--with-filename` | Bool | none (output formatting: print filename) |
| `-E` | `--extended-regexp` | Bool | none (ERE content regex dialect only) |
| `-F` | `--fixed-strings` | Bool | none (literal content match only) |
| `-G` | `--basic-regexp` | Bool | none (BRE content regex dialect only) |
| `-P` | `--perl-regexp` | Bool | none (PCRE content regex dialect only) |
| `-a` | `--text` | Bool | none (treat binary files as text during match) |
| `-z` | `--null-data` | Bool | none (NUL-delimited input lines only) |
| `-Z` | `--null` | Bool | none (output formatting: NUL filename separator) |
| `-b` | `--byte-offset` | Bool | none (output formatting: print byte offset) |
| `-T` | `--initial-tab` | Bool | none (output formatting: align tab) |
| `-r` | `--recursive` | Bool | recursive:+; links(root==, below=-) |
| `-R` | `--dereference-recursive` | Bool | recursive:+; links(root=+, below=+) |
| — | `--no-group-separator` | Bool | none (output formatting: omit context group separator) |
| `-I` | — | Bool | none (skip binary files in content match only) |
| `-U` | — | Bool | none (binary CR handling on MS-DOS/Windows only) |
| `-y` | — | Bool | none (obsolete alias for -i ignore-case) |
| `-0` | — | Bool | none (digit context-line count shorthand) |
| `-1` | — | Bool | none (digit context-line count shorthand) |
| `-2` | — | Bool | none (digit context-line count shorthand) |
| `-3` | — | Bool | none (digit context-line count shorthand) |
| `-4` | — | Bool | none (digit context-line count shorthand) |
| `-5` | — | Bool | none (digit context-line count shorthand) |
| `-6` | — | Bool | none (digit context-line count shorthand) |
| `-7` | — | Bool | none (digit context-line count shorthand) |
| `-8` | — | Bool | none (digit context-line count shorthand) |
| `-9` | — | Bool | none (digit context-line count shorthand) |
| `-e` | `--regexp` | Value(Pattern) | provides-pattern |
| `-f` | `--file` | Value(Path) | val-access:Read; provides-pattern |
| `-m` | `--max-count` | Value(Text) | none (limits matches printed per file) |
| `-A` | `--after-context` | Value(Text) | none (prints trailing context lines) |
| `-B` | `--before-context` | Value(Text) | none (prints leading context lines) |
| `-C` | `--context` | Value(Text) | none (prints surrounding context lines) |
| `-d` | `--directories` | Value(Text) | recursive:+; unread(FlagEffect) |
| `-D` | `--devices` | Value(Text) | none (device/FIFO handling only; does not change file or directory selection) |
| — | `--include` | Value(Pattern) | filter(GrepInclude, sensitive) |
| — | `--exclude` | Value(Pattern) | unread(FilterForm) |
| — | `--exclude-dir` | Value(Pattern) | unread(FilterForm) |
| — | `--label` | Value(Text) | none (stdin display label in output) |
| — | `--color` | Value(Text) | none (output color mode) |
| — | `--colour` | Value(Text) | none (output color mode) |
| — | `--binary-files` | Value(Text) | none (binary file content match mode only) |
| — | `--group-separator` | Value(Text) | none (output context group separator string) |

### `ggrep` (56 flags, ListedOnly)

| Short | Long | Kind | Effects |
|---|---|---|---|
| `-i` | `--ignore-case` | Bool | none (content match case-insensitivity only) |
| — | `--no-ignore-case` | Bool | none (content match case-sensitivity only) |
| `-v` | `--invert-match` | Bool | none (content match inversion only) |
| `-n` | `--line-number` | Bool | none (output formatting: print line numbers) |
| `-c` | `--count` | Bool | none (output formatting: print match count) |
| `-l` | `--files-with-matches` | Bool | none (output formatting: print matching filenames only) |
| `-L` | `--files-without-match` | Bool | none (output formatting: print non-matching filenames) |
| `-o` | `--only-matching` | Bool | none (output formatting: print matched text only) |
| `-q` | `--quiet` | Bool | none (suppress output) |
| — | `--silent` | Bool | none (suppress output) |
| `-s` | `--no-messages` | Bool | none (suppress error messages) |
| `-w` | `--word-regexp` | Bool | none (content match word boundaries only) |
| `-x` | `--line-regexp` | Bool | none (content match whole lines only) |
| `-h` | `--no-filename` | Bool | none (output formatting: omit filename) |
| `-H` | `--with-filename` | Bool | none (output formatting: print filename) |
| `-E` | `--extended-regexp` | Bool | none (ERE content regex dialect only) |
| `-F` | `--fixed-strings` | Bool | none (literal content match only) |
| `-G` | `--basic-regexp` | Bool | none (BRE content regex dialect only) |
| `-P` | `--perl-regexp` | Bool | none (PCRE content regex dialect only) |
| `-a` | `--text` | Bool | none (treat binary files as text during match) |
| `-z` | `--null-data` | Bool | none (NUL-delimited input lines only) |
| `-Z` | `--null` | Bool | none (output formatting: NUL filename separator) |
| `-b` | `--byte-offset` | Bool | none (output formatting: print byte offset) |
| `-T` | `--initial-tab` | Bool | none (output formatting: align tab) |
| `-r` | `--recursive` | Bool | recursive:+; links(root==, below=-) |
| `-R` | `--dereference-recursive` | Bool | recursive:+; links(root=+, below=+) |
| — | `--no-group-separator` | Bool | none (output formatting: omit context group separator) |
| `-I` | — | Bool | none (skip binary files in content match only) |
| `-U` | — | Bool | none (binary CR handling on MS-DOS/Windows only) |
| `-y` | — | Bool | none (obsolete alias for -i ignore-case) |
| `-0` | — | Bool | none (digit context-line count shorthand) |
| `-1` | — | Bool | none (digit context-line count shorthand) |
| `-2` | — | Bool | none (digit context-line count shorthand) |
| `-3` | — | Bool | none (digit context-line count shorthand) |
| `-4` | — | Bool | none (digit context-line count shorthand) |
| `-5` | — | Bool | none (digit context-line count shorthand) |
| `-6` | — | Bool | none (digit context-line count shorthand) |
| `-7` | — | Bool | none (digit context-line count shorthand) |
| `-8` | — | Bool | none (digit context-line count shorthand) |
| `-9` | — | Bool | none (digit context-line count shorthand) |
| `-e` | `--regexp` | Value(Pattern) | provides-pattern |
| `-f` | `--file` | Value(Path) | val-access:Read; provides-pattern |
| `-m` | `--max-count` | Value(Text) | none (limits matches printed per file) |
| `-A` | `--after-context` | Value(Text) | none (prints trailing context lines) |
| `-B` | `--before-context` | Value(Text) | none (prints leading context lines) |
| `-C` | `--context` | Value(Text) | none (prints surrounding context lines) |
| `-d` | `--directories` | Value(Text) | recursive:+; unread(FlagEffect) |
| `-D` | `--devices` | Value(Text) | none (device/FIFO handling only; does not change file or directory selection) |
| — | `--include` | Value(Pattern) | filter(GrepInclude, sensitive) |
| — | `--exclude` | Value(Pattern) | unread(FilterForm) |
| — | `--exclude-dir` | Value(Pattern) | unread(FilterForm) |
| — | `--label` | Value(Text) | none (stdin display label in output) |
| — | `--color` | Value(Text) | none (output color mode) |
| — | `--colour` | Value(Text) | none (output color mode) |
| — | `--binary-files` | Value(Text) | none (binary file content match mode only) |
| — | `--group-separator` | Value(Text) | none (output context group separator string) |

### `rgrep` (56 flags, ListedOnly)

| Short | Long | Kind | Effects |
|---|---|---|---|
| `-i` | `--ignore-case` | Bool | none (content match case-insensitivity only) |
| — | `--no-ignore-case` | Bool | none (content match case-sensitivity only) |
| `-v` | `--invert-match` | Bool | none (content match inversion only) |
| `-n` | `--line-number` | Bool | none (output formatting: print line numbers) |
| `-c` | `--count` | Bool | none (output formatting: print match count) |
| `-l` | `--files-with-matches` | Bool | none (output formatting: print matching filenames only) |
| `-L` | `--files-without-match` | Bool | none (output formatting: print non-matching filenames) |
| `-o` | `--only-matching` | Bool | none (output formatting: print matched text only) |
| `-q` | `--quiet` | Bool | none (suppress output) |
| — | `--silent` | Bool | none (suppress output) |
| `-s` | `--no-messages` | Bool | none (suppress error messages) |
| `-w` | `--word-regexp` | Bool | none (content match word boundaries only) |
| `-x` | `--line-regexp` | Bool | none (content match whole lines only) |
| `-h` | `--no-filename` | Bool | none (output formatting: omit filename) |
| `-H` | `--with-filename` | Bool | none (output formatting: print filename) |
| `-E` | `--extended-regexp` | Bool | none (ERE content regex dialect only) |
| `-F` | `--fixed-strings` | Bool | none (literal content match only) |
| `-G` | `--basic-regexp` | Bool | none (BRE content regex dialect only) |
| `-P` | `--perl-regexp` | Bool | none (PCRE content regex dialect only) |
| `-a` | `--text` | Bool | none (treat binary files as text during match) |
| `-z` | `--null-data` | Bool | none (NUL-delimited input lines only) |
| `-Z` | `--null` | Bool | none (output formatting: NUL filename separator) |
| `-b` | `--byte-offset` | Bool | none (output formatting: print byte offset) |
| `-T` | `--initial-tab` | Bool | none (output formatting: align tab) |
| `-r` | `--recursive` | Bool | recursive:+; links(root==, below=-) |
| `-R` | `--dereference-recursive` | Bool | recursive:+; links(root=+, below=+) |
| — | `--no-group-separator` | Bool | none (output formatting: omit context group separator) |
| `-I` | — | Bool | none (skip binary files in content match only) |
| `-U` | — | Bool | none (binary CR handling on MS-DOS/Windows only) |
| `-y` | — | Bool | none (obsolete alias for -i ignore-case) |
| `-0` | — | Bool | none (digit context-line count shorthand) |
| `-1` | — | Bool | none (digit context-line count shorthand) |
| `-2` | — | Bool | none (digit context-line count shorthand) |
| `-3` | — | Bool | none (digit context-line count shorthand) |
| `-4` | — | Bool | none (digit context-line count shorthand) |
| `-5` | — | Bool | none (digit context-line count shorthand) |
| `-6` | — | Bool | none (digit context-line count shorthand) |
| `-7` | — | Bool | none (digit context-line count shorthand) |
| `-8` | — | Bool | none (digit context-line count shorthand) |
| `-9` | — | Bool | none (digit context-line count shorthand) |
| `-e` | `--regexp` | Value(Pattern) | provides-pattern |
| `-f` | `--file` | Value(Path) | val-access:Read; provides-pattern |
| `-m` | `--max-count` | Value(Text) | none (limits matches printed per file) |
| `-A` | `--after-context` | Value(Text) | none (prints trailing context lines) |
| `-B` | `--before-context` | Value(Text) | none (prints leading context lines) |
| `-C` | `--context` | Value(Text) | none (prints surrounding context lines) |
| `-d` | `--directories` | Value(Text) | recursive:+; unread(FlagEffect) |
| `-D` | `--devices` | Value(Text) | none (device/FIFO handling only; does not change file or directory selection) |
| — | `--include` | Value(Pattern) | filter(GrepInclude, sensitive) |
| — | `--exclude` | Value(Pattern) | unread(FilterForm) |
| — | `--exclude-dir` | Value(Pattern) | unread(FilterForm) |
| — | `--label` | Value(Text) | none (stdin display label in output) |
| — | `--color` | Value(Text) | none (output color mode) |
| — | `--colour` | Value(Text) | none (output color mode) |
| — | `--binary-files` | Value(Text) | none (binary file content match mode only) |
| — | `--group-separator` | Value(Text) | none (output context group separator string) |

### `ag` (79 flags, ListedOnly)

| Short | Long | Kind | Effects |
|---|---|---|---|
| `-a` | `--all-types` | Bool | none (disables default type skipping; model already checks all entries) |
| `-D` | `--debug` | Bool | none (prints debug diagnostics only) |
| `-c` | `--count` | Bool | none (output formatting: print match count) |
| `-F` | `--fixed-strings` | Bool | none (literal content match only) |
| `-f` | `--follow` | Bool | links(root=+, below=+) |
| `-H` | `--heading` | Bool | none (output formatting: print file headings) |
| `-h` | — | Bool | none (print help and exit) |
| `-i` | `--ignore-case` | Bool | none (content match case-insensitivity only) |
| `-L` | `--files-without-matches` | Bool | none (output formatting: print non-matching filenames) |
| `-l` | `--files-with-matches` | Bool | none (output formatting: print matching filenames only) |
| `-n` | `--norecurse` | Bool | recursive:- |
| `-o` | `--only-matching` | Bool | none (output formatting: print matched text only) |
| `-Q` | `--literal` | Bool | none (literal content match only) |
| `-R` | `--recurse` | Bool | recursive:+ |
| `-r` | — | Bool | recursive:+ |
| `-S` | `--smart-case` | Bool | none (content match smart-case only) |
| `-s` | `--case-sensitive` | Bool | none (content match case-sensitivity only) |
| `-v` | `--invert-match` | Bool | none (content match inversion only) |
| `-V` | — | Bool | none (print version and exit) |
| `-t` | `--all-text` | Bool | none (disables ignore skipping for text files; model checks all entries) |
| `-u` | `--unrestricted` | Bool | none (disables ignore/hidden skipping; model checks all entries) |
| `-U` | `--skip-vcs-ignores` | Bool | none (disables VCS ignore files; model checks all entries) |
| `-w` | `--word-regexp` | Bool | none (content match word boundaries only) |
| `-z` | `--search-zip` | Bool | unread(FlagEffect) |
| `-0` | `--null` | Bool | none (output formatting: NUL filename separator) |
| — | `--ackmate` | Bool | none (output formatting: AckMate format) |
| — | `--affinity` | Bool | none (CPU thread affinity setting only) |
| — | `--noaffinity` | Bool | none (disables CPU thread affinity only) |
| — | `--hidden` | Bool | none (searches dotfiles; model already checks all entries) |
| — | `--column` | Bool | none (output formatting: print column number) |
| — | `--nocolor` | Bool | none (output formatting: disable color) |
| — | `--color` | Bool | none (output formatting: enable color) |
| — | `--color-win-ansi` | Bool | none (output formatting: ANSI colors on Windows) |
| — | `--noheading` | Bool | none (output formatting: omit file headings) |
| — | `--nobreak` | Bool | none (output formatting: omit blank lines between files) |
| — | `--break` | Bool | none (output formatting: print blank lines between files) |
| — | `--nofollow` | Bool | links(root=-, below=-) |
| — | `--search-binary` | Bool | none (search binary files; model already checks all entries) |
| — | `--silent` | Bool | none (suppress error messages) |
| — | `--stats` | Bool | none (output formatting: print search statistics) |
| — | `--stats-only` | Bool | none (output formatting: print search statistics only) |
| — | `--vimgrep` | Bool | none (output formatting: vimgrep format) |
| — | `--print0` | Bool | none (output formatting: NUL filename separator) |
| — | `--one-device` | Bool | unread(FilterForm) |
| — | `--filename` | Bool | none (output formatting: print filename) |
| — | `--nofilename` | Bool | none (output formatting: omit filename) |
| — | `--numbers` | Bool | none (output formatting: print line numbers) |
| — | `--nonumbers` | Bool | none (output formatting: omit line numbers) |
| — | `--line-numbers` | Bool | none (output formatting: print line numbers) |
| — | `--group` | Bool | none (output formatting: group matches by file) |
| — | `--nogroup` | Bool | none (output formatting: do not group matches by file) |
| — | `--match` | Bool | none (disables filename-only search mode) |
| — | `--mmap` | Bool | none (use mmap for file reading only) |
| — | `--nommap` | Bool | none (disable mmap for file reading only) |
| — | `--multiline` | Bool | none (multiline content match only) |
| — | `--nomultiline` | Bool | none (disable multiline content match) |
| — | `--nopager` | Bool | none (do not pipe output to pager) |
| — | `--parallel` | Bool | none (enable parallel search threads only) |
| — | `--passthrough` | Bool | none (output formatting: print all lines of file) |
| — | `--passthru` | Bool | none (output formatting: print all lines of file) |
| — | `--print-all-files` | Bool | none (output formatting: print all file headers) |
| — | `--print-long-lines` | Bool | none (output formatting: do not truncate lines) |
| `-G` | `--file-search-regex` | Value(Pattern) | unread(FilterForm) |
| `-g` | `--filename-pattern` | Value(Pattern) | provides-pattern; unread(FilterForm) |
| `-m` | `--max-count` | Value(Text) | none (limits matches printed per file) |
| `-p` | `--path-to-ignore` | Value(Path) | val-access:Read; unread(FilterForm) |
| `-W` | `--width` | Value(Text) | none (output formatting: truncate lines to width) |
| — | `--ackmate-dir-filter` | Value(Pattern) | unread(FilterForm) |
| — | `--depth` | Value(Text) | unread(FilterForm) |
| — | `--ignore` | Value(Pattern) | unread(FilterForm) |
| — | `--ignore-dir` | Value(Pattern) | unread(FilterForm) |
| — | `--pager` | Value(Code) | runs-code |
| — | `--workers` | Value(Text) | none (sets worker thread count) |
| — | `--color-line-number` | Value(Text) | none (output color for line numbers) |
| — | `--color-match` | Value(Text) | none (output color for matches) |
| — | `--color-path` | Value(Text) | none (output color for paths) |
| `-A` | `--after` | OptionalValue(Text) | none (prints trailing context lines) |
| `-B` | `--before` | OptionalValue(Text) | none (prints leading context lines) |
| `-C` | `--context` | OptionalValue(Text) | none (prints surrounding context lines) |

### `ack` (41 flags, ListedOnly)

| Short | Long | Kind | Effects |
|---|---|---|---|
| `-i` | `--ignore-case` | Bool | none (content match case-insensitivity only) |
| `-I` | — | Bool | none (content match case-sensitivity only) |
| `-S` | `--smart-case` | Bool | none (content match smart-case only) |
| `-v` | `--invert-match` | Bool | none (content match inversion only) |
| `-w` | `--word-regexp` | Bool | none (content match word boundaries only) |
| `-Q` | `--literal` | Bool | none (literal content match only) |
| `-l` | `--files-with-matches` | Bool | none (output formatting: print matching filenames only) |
| `-L` | `--files-without-matches` | Bool | none (output formatting: print non-matching filenames) |
| `-c` | `--count` | Bool | none (output formatting: print match count) |
| `-h` | `--no-filename` | Bool | none (output formatting: omit filename) |
| `-H` | `--with-filename` | Bool | none (output formatting: print filename) |
| `-o` | — | Bool | none (output formatting: print matched text only) |
| `-1` | — | Bool | none (stop after first match) |
| `-n` | `--no-recurse` | Bool | recursive:- |
| `-r` | `--recurse` | Bool | recursive:+ |
| `-R` | — | Bool | recursive:+ |
| `-s` | — | Bool | none (suppress error messages) |
| `-x` | — | Bool | unread(FlagEffect) |
| `-f` | — | Bool | provides-pattern |
| `-k` | — | Bool | unread(FilterForm) |
| — | `--column` | Bool | none (output formatting: print column number) |
| — | `--nocolor` | Bool | none (output formatting: disable color) |
| — | `--color` | Bool | none (output formatting: enable color) |
| — | `--noheading` | Bool | none (output formatting: omit file headings) |
| — | `--heading` | Bool | none (output formatting: print file headings) |
| — | `--nobreak` | Bool | none (output formatting: omit blank lines between files) |
| — | `--break` | Bool | none (output formatting: print blank lines between files) |
| — | `--follow` | Bool | links(root=+, below=+) |
| — | `--nofollow` | Bool | links(root=-, below=-) |
| — | `--sort-files` | Bool | none (sorts output file order only) |
| — | `--nopager` | Bool | none (do not pipe output to pager) |
| `-A` | `--after-context` | Value(Text) | none (prints trailing context lines) |
| `-B` | `--before-context` | Value(Text) | none (prints leading context lines) |
| `-m` | `--max-count` | Value(Text) | none (limits matches printed per file) |
| `-g` | — | Value(Pattern) | provides-pattern; unread(FilterForm) |
| `-t` | `--type` | Value(Pattern) | unread(FilterForm) |
| — | `--ignore-dir` | Value(Pattern) | unread(FilterForm) |
| — | `--noignore-dir` | Value(Pattern) | unread(FilterForm) |
| — | `--match` | Value(Pattern) | provides-pattern |
| — | `--output` | Value(Code) | runs-code |
| `-C` | `--context` | OptionalValue(Text) | none (prints surrounding context lines) |

### `git` (8 flags, ListedOnly)

| Short | Long | Kind | Effects |
|---|---|---|---|
| `-C` | — | Value(Path) | val-access:Read; unread(BaseDirChange) |
| `-c` | — | Value(Code) | runs-code |
| — | `--config-env` | Value(Code) | runs-code |
| — | `--exec-path` | Value(Code) | runs-code |
| — | `--git-dir` | Value(Path) | val-access:Read; unread(BaseDirChange) |
| — | `--work-tree` | Value(Path) | val-access:Read; unread(BaseDirChange) |
| — | `--namespace` | Value(Text) | none (selects git ref namespace only; does not change host filesystem paths) |
| — | `--super-prefix` | Value(Text) | unread(BaseDirChange) |

### `git log` (2 flags, AnyExceptForbidden)

| Short | Long | Kind | Effects |
|---|---|---|---|
| `-p` | — | Bool | none (shows patch diffs in git log output; CR-12 git selection still checks operands) |
| `-L` | `--line-range` | Value(Text) | none (selects line range in git log; file part checked by CR-12 git selection (§6)) |

### `git diff` (2 flags, AnyExceptForbidden)

| Short | Long | Kind | Effects |
|---|---|---|---|
| — | `--output` | Value(Path) | writes:+; val-access:Write |
| — | `--ext-diff` | Bool | runs-code |

### `git branch` (14 flags, AnyExceptForbidden)

| Short | Long | Kind | Effects |
|---|---|---|---|
| `-a` | `--all` | Bool | none (lists both local and remote-tracking branches (read-only)) |
| `-r` | `--remotes` | Bool | none (lists remote-tracking branches (read-only)) |
| `-v` | `--verbose` | Bool | none (prints commit hash and subject in branch listing (read-only)) |
| `-l` | `--list` | Bool | writes:- |
| — | `--show-current` | Bool | none (prints current branch name only (read-only)) |
| `-d` | `--delete` | Bool | writes:+ |
| `-D` | — | Bool | writes:+ |
| `-m` | `--move` | Bool | writes:+ |
| `-M` | — | Bool | writes:+ |
| `-c` | `--copy` | Bool | writes:+ |
| `-C` | — | Bool | writes:+ |
| `-u` | `--set-upstream-to` | Value(Ref) | writes:+ |
| — | `--unset-upstream` | Bool | writes:+ |
| — | `--edit-description` | Bool | writes:+ |

### `git push` (11 flags, ListedOnly)

| Short | Long | Kind | Effects |
|---|---|---|---|
| `-f` | `--force` | Bool | git-push(Force) |
| — | `--force-with-lease` | Bool | git-push(Force) |
| — | `--force-if-includes` | Bool | git-push(Force) |
| — | `--mirror` | Bool | git-push(Force) |
| — | `--all` | Bool | git-push(ImplicitTargets) |
| — | `--tags` | Bool | git-push(ImplicitTargets) |
| — | `--branches` | Bool | git-push(ImplicitTargets) |
| — | `--prune` | Bool | git-push(ImplicitTargets) |
| `-o` | `--push-option` | Value(Text) | none (transmits string option to remote server only; does not touch host paths) |
| — | `--receive-pack` | Value(Code) | runs-code |
| — | `--exec` | Value(Code) | runs-code |

### `git checkout` (3 flags, ListedOnly)

| Short | Long | Kind | Effects |
|---|---|---|---|
| `-b` | — | Value(Ref) | writes:+ |
| `-B` | — | Value(Ref) | writes:+ |
| — | `--orphan` | Value(Ref) | writes:+ |

### `git rebase` (1 flags, ListedOnly)

| Short | Long | Kind | Effects |
|---|---|---|---|
| `-x` | `--exec` | Value(Code) | runs-code |

### `git difftool` (1 flags, ListedOnly)

| Short | Long | Kind | Effects |
|---|---|---|---|
| `-x` | `--tool-cmd` | Value(Code) | runs-code |

### `git mergetool` (1 flags, ListedOnly)

| Short | Long | Kind | Effects |
|---|---|---|---|
| `-x` | `--tool-cmd` | Value(Code) | runs-code |

### `cd` (4 flags, ListedOnly)

| Short | Long | Kind | Effects |
|---|---|---|---|
| `-L` | — | Bool | none (logical symlink handling for cd; target resolved by P2 (§5)) |
| `-P` | — | Bool | none (physical symlink handling for cd; target resolved by P2 (§5)) |
| `-e` | — | Bool | none (exit non-zero if physical cwd cannot be determined) |
| `-@` | — | Bool | none (extended-attribute directory flag accepted by dev's cd parser) |

### `find` (22 flags, ListedOnly)

| Short | Long | Kind | Effects |
|---|---|---|---|
| `-H` | — | Bool | links(root=+, below=-) |
| `-L` | `-follow` | Bool | links(root=+, below=+) |
| `-P` | — | Bool | links(root=-, below=-) |
| `-E` | — | Bool | none (extended regex syntax for -regex predicate only) |
| `-X` | — | Bool | none (xargs-safe path warning mode only) |
| `-d` | — | Bool | none (BSD depth-first traversal order only (same files visited)) |
| `-s` | — | Bool | none (BSD lexicographical traversal order only (same files visited)) |
| `-x` | — | Bool | unread(FilterForm) |
| `-D` | — | Value(Text) | none (GNU find debug output category only) |
| `-f` | — | Value(Path) | val-access:Read |
| — | `-delete` | Bool | writes:+; writes-inputs:+; derived-dest:+ |
| — | `-depth` | Bool | none (depth-first traversal order only (same files visited)) |
| — | `-maxdepth` | Value(Text) | unread(FilterForm) |
| — | `-mindepth` | Value(Text) | unread(FilterForm) |
| — | `-exec` | TerminatedValues(Code, end: ; / +) | writes:+; runs-code |
| — | `-execdir` | TerminatedValues(Code, end: ; / +) | writes:+; runs-code |
| — | `-ok` | TerminatedValues(Code, end: ;) | writes:+; runs-code |
| — | `-okdir` | TerminatedValues(Code, end: ;) | writes:+; runs-code |
| — | `-fprint` | Value(Path) | writes:+; val-access:Write |
| — | `-fprint0` | Value(Path) | writes:+; val-access:Write |
| — | `-fprintf` | MultiValue(Path:Write, Text) | writes:+ |
| — | `-fls` | Value(Path) | writes:+; val-access:Write |

### `ln` (1 flags, ListedOnly)

| Short | Long | Kind | Effects |
|---|---|---|---|
| `-s` | — | Bool | none (creates symbolic link instead of hard link; target stays Read and link stays Write (§6)) |

### `cp` (6 flags, ListedOnly)

| Short | Long | Kind | Effects |
|---|---|---|---|
| `-r` | — | Bool | derived-dest:+; recursive:+ |
| `-R` | `--recursive` | Bool | derived-dest:+; recursive:+ |
| `-a` | `--archive` | Bool | derived-dest:+; recursive:+; links(root=-, below=-) |
| `-L` | `--dereference` | Bool | links(root=+, below=+) |
| `-P` | — | Bool | links(root=-, below=-) |
| `-d` | — | Bool | links(root=-, below=-) |

### `rsync` (3 flags, ListedOnly)

| Short | Long | Kind | Effects |
|---|---|---|---|
| `-L` | `--copy-links` | Bool | links(root=+, below=+) |
| `-k` | `--copy-dirlinks` | Bool | links(root=+, below=+) |
| — | `--copy-unsafe-links` | Bool | links(root=+, below=+) |

### `tar` (8 flags, ListedOnly)

| Short | Long | Kind | Effects |
|---|---|---|---|
| `-c` | `--create` | Bool | writes:+; op:Write; positional-role(Path) |
| `-r` | `--append` | Bool | writes:+; op:Write; positional-role(Path) |
| `-u` | `--update` | Bool | writes:+; op:Write; positional-role(Path) |
| `-x` | `--extract` | Bool | writes:+; op:Extract; derived-dest:+; positional-role(ArchiveMember) |
| `-t` | `--list` | Bool | op:List; positional-role(ArchiveMember) |
| `-h` | `--dereference` | Bool | links(root=+, below=+) |
| `-f` | `--file` | Value(Path) | val-access:TarArchive |
| `-C` | `--directory` | Value(Path) | val-access:TarDirectory; unread(BaseDirChange) |

### `unzip` (1 flags, ListedOnly)

| Short | Long | Kind | Effects |
|---|---|---|---|
| `-d` | — | Value(DestinationRoot) | writes:+; val-access:Write |

### `rm` (2 flags, ListedOnly)

| Short | Long | Kind | Effects |
|---|---|---|---|
| `-r` | `--recursive` | Bool | recursive:+ |
| `-R` | — | Bool | recursive:+ |

### `sort` (1 flags, ListedOnly)

| Short | Long | Kind | Effects |
|---|---|---|---|
| `-o` | `--output` | Value(Path) | writes:+; val-access:Write |

### `echo` (3 flags, ListedOnly)

| Short | Long | Kind | Effects |
|---|---|---|---|
| `-n` | — | Bool | none (omits trailing newline in stdout output only) |
| `-e` | — | Bool | none (interprets backslash escapes in stdout output only) |
| `-E` | — | Bool | none (disables backslash escape interpretation in stdout output only) |

### `printf` (1 flags, ListedOnly)

| Short | Long | Kind | Effects |
|---|---|---|---|
| `-v` | — | Value(Text) | unread(FlagEffect) |

### `codesign` (5 flags, ListedOnly)

| Short | Long | Kind | Effects |
|---|---|---|---|
| `-s` | `--sign` | Value(Text) | writes:+; writes-inputs:+ |
| `-f` | `--force` | Bool | writes:+; writes-inputs:+ |
| — | `--remove-signature` | Bool | writes:+; writes-inputs:+ |
| `-d` | `--display` | Bool | none (displays code signature metadata (read-only)) |
| `-v` | `--verify` | Bool | none (verifies code signature (read-only)) |

### `awk` (1 flags, ListedOnly)

| Short | Long | Kind | Effects |
|---|---|---|---|
| `-i` | `--include` | Value(Code) | writes:+; runs-code |

### `gawk` (1 flags, ListedOnly)

| Short | Long | Kind | Effects |
|---|---|---|---|
| `-i` | `--include` | Value(Code) | writes:+; runs-code |

### `mawk` (1 flags, ListedOnly)

| Short | Long | Kind | Effects |
|---|---|---|---|
| `-i` | `--include` | Value(Code) | writes:+; runs-code |

### `sed` (3 flags, ListedOnly)

| Short | Long | Kind | Effects |
|---|---|---|---|
| `-i` | `--in-place` | Bool | writes:+; writes-inputs:+ |
| `-e` | `--expression` | Value(Code) | provides-script |
| `-f` | `--file` | Value(ProgramFile) | writes:+; val-access:Read; provides-script; runs-program(FlagValue) |

### `perl` (3 flags, ListedOnly)

| Short | Long | Kind | Effects |
|---|---|---|---|
| `-i` | — | Bool | writes-inputs:+ |
| `-e` | — | Value(Code) | provides-script; runs-code |
| `-E` | — | Value(Code) | provides-script; runs-code |

### `python` (2 flags, ListedOnly)

| Short | Long | Kind | Effects |
|---|---|---|---|
| `-c` | — | Value(Code) | runs-code |
| `-m` | — | Value(Code) | runs-code |

### `python3` (2 flags, ListedOnly)

| Short | Long | Kind | Effects |
|---|---|---|---|
| `-c` | — | Value(Code) | runs-code |
| `-m` | — | Value(Code) | runs-code |

### `node` (2 flags, ListedOnly)

| Short | Long | Kind | Effects |
|---|---|---|---|
| `-e` | `--eval` | Value(Code) | runs-code |
| `-p` | `--print` | Value(Code) | runs-code |

### `ruby` (1 flags, ListedOnly)

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
| `command` | §11 item 2; §11 item 10 |
| `exec` | §11 item 2; §11 item 10 |
| `nice` | §11 item 2; §11 item 9; §11 item 10 |
| `timeout` | §11 item 2; §11 item 9; §11 item 10 |
| `time` | §11 item 2; §11 item 10 |
| `env` | §11 item 2; §11 item 10 |
| `sh` | §11 item 2; §11 item 4; §11 item 8 |
| `bash` | §11 item 2; §11 item 4; §11 item 8 |
| `dash` | §11 item 2; §11 item 4; §11 item 8 |
| `which` | §11 item 9 |
| `rg` | §11 item 2; §11 item 9; §11 item 15 |
| `grep` | §11 item 2; §11 item 9; §11 item 15 |
| `egrep` | §11 item 2; §11 item 9 |
| `fgrep` | §11 item 2; §11 item 9 |
| `zgrep` | §11 item 2; §11 item 9 |
| `ggrep` | §11 item 2; §11 item 9 |
| `rgrep` | §11 item 2; §11 item 9 |
| `ag` | §11 item 2; §11 item 4; §11 item 9 |
| `ack` | §11 item 2; §11 item 4; §11 item 9 |
| `git` | §11 item 2; §11 item 4; §11 item 9 |
| `git status` | §11 item 10 |
| `git log` | §11 item 9; §11 item 10 |
| `git diff` | §11 item 4; §11 item 9; §11 item 10 |
| `git show` | §11 item 9; §11 item 10 |
| `git branch` | §11 item 9 |
| `git rev-parse` | §11 item 9 |
| `git ls-files` | §11 item 10 |
| `git cat-file` | §11 item 2; §11 item 9 |
| `git push` | §11 item 2; §11 item 4; §11 item 9 |
| `git clone` | §11 item 2; §11 item 9; §11 item 10 |
| `git checkout` | §11 item 2; §11 item 9; §11 item 10 |
| `git merge-index` | §11 item 2; §11 item 4 |
| `git filter-branch` | §11 item 2; §11 item 4 |
| `git bisect` | §11 item 2; §11 item 4; §11 item 9 |
| `git rebase` | §11 item 2; §11 item 4; §11 item 9 |
| `git submodule` | §11 item 2; §11 item 4; §11 item 9 |
| `git difftool` | §11 item 2; §11 item 4; §11 item 9 |
| `git mergetool` | §11 item 2; §11 item 4 |
| `cargo` | §11 item 2; §11 item 4; §11 item 8 |
| `cargo test` | §11 item 2; §11 item 8; §11 item 9 |
| `cargo build` | §11 item 2; §11 item 8; §11 item 9 |
| `cargo run` | §11 item 2; §11 item 8; §11 item 9 |
| `cargo check` | §11 item 2; §11 item 8; §11 item 9 |
| `cargo bench` | §11 item 2; §11 item 8; §11 item 9 |
| `cargo doc` | §11 item 2; §11 item 8; §11 item 9 |
| `cargo clippy` | §11 item 2; §11 item 8; §11 item 9 |
| `pipx` | §11 item 2; §11 item 4; §11 item 8 |
| `pipx run` | §11 item 2; §11 item 4; §11 item 8 |
| `cd` | §11 item 2 |
| `find` | §11 item 2; §11 item 4; §11 item 9; §11 item 10 |
| `ln` | §11 item 2 |
| `cp` | §11 item 2; §11 item 10 |
| `mv` | §11 item 2; §11 item 10 |
| `rsync` | §11 item 2; §11 item 10 |
| `tar` | §11 item 2; §11 item 9; §11 item 10 |
| `unzip` | §11 item 2; §11 item 9; §11 item 10 |
| `rm` | §11 item 2 |
| `tee` | §11 item 2 |
| `touch` | §11 item 2 |
| `chmod` | §11 item 2; §11 item 9 |
| `chown` | §11 item 2; §11 item 9 |
| `chgrp` | §11 item 2; §11 item 9 |
| `truncate` | §11 item 2 |
| `install` | §11 item 2 |
| `mkdir` | §11 item 2 |
| `rmdir` | §11 item 2 |
| `unlink` | §11 item 2 |
| `shred` | §11 item 2 |
| `patch` | §11 item 2 |
| `wipefs` | §11 item 2 |
| `blkdiscard` | §11 item 2 |
| `ed` | §11 item 2 |
| `ex` | §11 item 2 |
| `diff` | §11 item 2 |
| `cmp` | §11 item 2 |
| `md5` | §11 item 2 |
| `md5sum` | §11 item 2 |
| `shasum` | §11 item 2 |
| `sha1sum` | §11 item 2 |
| `sha256sum` | §11 item 2 |
| `readlink` | §11 item 2 |
| `realpath` | §11 item 2 |
| `basename` | §11 item 2 |
| `dirname` | §11 item 2 |
| `nl` | §11 item 2 |
| `od` | §11 item 2 |
| `hexdump` | §11 item 2 |
| `strings` | §11 item 2 |
| `bat` | §11 item 2 |
| `column` | §11 item 2 |
| `fold` | §11 item 2 |
| `expand` | §11 item 2 |
| `less` | §11 item 2 |
| `more` | §11 item 2 |
| `cut` | §11 item 2 |
| `sort` | §11 item 2 |
| `uniq` | §11 item 2 |
| `xxd` | §11 item 2 |
| `echo` | §11 item 2; §11 item 9 |
| `printf` | §11 item 2; §11 item 9 |
| `whereis` | §11 item 2; §11 item 9 |
| `type` | §11 item 2; §11 item 9 |
| `tr` | §11 item 2; §11 item 9 |
| `true` | §11 item 2; §11 item 9 |
| `false` | §11 item 2; §11 item 9 |
| `id` | §11 item 2; §11 item 9 |
| `test` | §11 item 2 |
| `[` | §11 item 2 |
| `jq` | §11 item 2; §11 item 9 |
| `mdls` | §11 item 2 |
| `open` | §11 item 2 |
| `spctl` | §11 item 2 |
| `mdfind` | §11 item 2 |
| `codesign` | §11 item 2; §11 item 9 |
| `awk` | §11 item 2; §11 item 4 |
| `gawk` | §11 item 2; §11 item 4 |
| `mawk` | §11 item 2; §11 item 4 |
| `sed` | §11 item 2; §11 item 4; §11 item 8 |
| `perl` | §11 item 2; §11 item 4; §11 item 8 |
| `python` | §11 item 2; §11 item 4; §11 item 8 |
| `python3` | §11 item 2; §11 item 4; §11 item 8 |
| `node` | §11 item 2; §11 item 4; §11 item 8 |
| `ruby` | §11 item 2; §11 item 4; §11 item 8 |
| `eval` | §11 item 2; §11 item 4 |
| `source` | §11 item 2; §11 item 4; §11 item 8 |
| `.` | §11 item 2; §11 item 4; §11 item 8 |
| `npx` | §11 item 2; §11 item 4; §11 item 8 |
| `xargs` | §11 item 2; §11 item 4 |
| `ssh` | §11 item 2; §11 item 4; §11 item 9 |
| `mosh` | §11 item 2; §11 item 4; §11 item 9 |
| `autossh` | §11 item 2; §11 item 4; §11 item 9 |
| `nsenter` | §11 item 2; §11 item 4 |
| `unshare` | §11 item 2; §11 item 4 |
| `docker` | §11 item 2; §11 item 4; §11 item 9 |
| `podman` | §11 item 2; §11 item 4; §11 item 9 |
| `nerdctl` | §11 item 2; §11 item 4; §11 item 9 |
| `kubectl` | §11 item 2; §11 item 4; §11 item 9 |
| `oc` | §11 item 2; §11 item 4; §11 item 9 |
| `lxc` | §11 item 2; §11 item 4; §11 item 9 |
| `incus` | §11 item 2; §11 item 4; §11 item 9 |
| `machinectl` | §11 item 2; §11 item 4; §11 item 9 |
| `systemd-run` | §11 item 2; §11 item 4 |
| `systemd-nspawn` | §11 item 2; §11 item 4 |
| `firejail` | §11 item 2; §11 item 4 |
| `bwrap` | §11 item 2; §11 item 4 |
| `flatpak-spawn` | §11 item 2; §11 item 4 |
| `toolbox` | §11 item 2; §11 item 4; §11 item 9 |
| `distrobox` | §11 item 2; §11 item 4; §11 item 9 |
| `distrobox-enter` | §11 item 2; §11 item 4; §11 item 9 |
| `multipass` | §11 item 2; §11 item 4; §11 item 9 |
| `vagrant` | §11 item 2; §11 item 4; §11 item 9 |
| `limactl` | §11 item 2; §11 item 4; §11 item 9 |
| `colima` | §11 item 2; §11 item 4; §11 item 9 |
| `runuser` | §11 item 2; §11 item 4; §11 item 9 |
| `setpriv` | §11 item 2; §11 item 4 |
| `parallel` | §11 item 2; §11 item 4 |
| `curl` | §11 item 2 |
| `wget` | §11 item 2 |
| `fetch` | §11 item 2 |
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
