# Policy engine rev 4: normative tool table (annex to §6, D-141)

Generated from `crates/atrium-core/src/policy/tools.rs`. Do not edit by hand; regenerate with `env UPDATE_TOOL_TABLE=1 cargo test -p atrium-core tools`.

**D-141 structure — six row kinds plus unknown-tool fallback (§4, §6):**
- **`HomeReadOnly`:** tools and `git` subcommands in `HOME_READONLY_COMMANDS` (without a search flag table). Uses `AnyExceptForbidden` mirroring `dev`'s `forbidden_flag` matching (short clusters and long abbreviations of `>= 3` chars including `--`). Every word is a candidate `Path` (`ReadWrite`, no operand relaxations), and a forbidden flag **only** removes Home read-only eligibility (never makes the unit `Unread` on its own).
- **`Search`:** `rg` and the `grep` family, keeping `dev`'s flag tables (listed flags only), the pattern operand (§11 item 9), modeled filter dialects and case rules, unsupported filters as `Unread`, and recursion / link-following flags with the §11 item 15 toggle.
- **`Wrapper` and `NestedShell`:** §4's closed wrapper forms and `sh`/`bash`/`dash` nested shells.
- **`CodeRunner`:** always `Unread`, naming its inline-code location or pinned program (`Unread (program)`, §11 item 8), with `code_runner_no_pin` per §7 / §11 item 4.
- **`ConditionalRunner`:** unknown-tool treatment unless a listed trigger fires (`find`, `git`, `sed`), making the unit `Unread` (or pinnable `Unread (program)` for `sed -f`/`--file`). No flag arities or operand roles.
- **`DerivedDestinationWriter`:** `cp` (`-R`/`-r`/`-a`), `mv`, `rsync`, `tar` (`-x`), `unzip`, `git clone`, `git checkout`. When triggered, the writer itself is `Unread(DerivedDestinations)` (§11 item 10) while every word stays a candidate `Path` (`ReadWrite`). Without a trigger, the unit receives unknown-tool treatment.
- **Every other tool** (including **Unknown tools by design** below) is an **unknown tool** (§4): every word is a candidate `Path` (`ReadWrite`) for hard-deny, store, and self-protection checks, the unit is a possible writer (§5), and it is never Home-eligible.
- **Outside the model (`OUTSIDE_MODEL`):** non-§4 shells, non-§4 wrappers, shell reserved words, and `pushd`/`popd` always make their unit `Unread`.

## 1. `HomeReadOnly` rows

| Command | Kind | Platforms | Forbidden flags (Home only) | Only args (Home only) | Git checks | Changes default (§11) |
|---|---|---|---|---|---|---|
| `ls` | `HomeReadOnly` | Linux, macOS | — | — | — | — |
| `pwd` | `HomeReadOnly` | Linux, macOS | — | — | — | — |
| `cat` | `HomeReadOnly` | Linux, macOS | — | — | — | — |
| `head` | `HomeReadOnly` | Linux, macOS | — | — | — | — |
| `tail` | `HomeReadOnly` | Linux, macOS | — | — | — | — |
| `wc` | `HomeReadOnly` | Linux, macOS | — | — | — | — |
| `file` | `HomeReadOnly` | Linux, macOS | `-C`, `--compile`, `-m`, `--magic-file`, `-f`, `--files-from` | — | — | — |
| `stat` | `HomeReadOnly` | Linux, macOS | — | — | — | — |
| `du` | `HomeReadOnly` | Linux, macOS | — | — | — | — |
| `df` | `HomeReadOnly` | Linux, macOS | — | — | — | — |
| `which` | `HomeReadOnly` | Linux, macOS | — | — | — | — |
| `uname` | `HomeReadOnly` | Linux, macOS | — | — | — | — |
| `date` | `HomeReadOnly` | Linux, macOS | `-s`, `--set`, `-f`, `--file` | — | — | — |
| `whoami` | `HomeReadOnly` | Linux, macOS | — | — | — | — |
| `tree` | `HomeReadOnly` | Linux, macOS | `-o`, `--output`, `-R` | — | — | — |
| `git status` | `HomeReadOnly` | Linux, macOS | `--output`, `--ext-diff`, `--exec-path`, `--config-env`, `-c` | — | wildcard pathspec → Unread | §11 item 10 |
| `git log` | `HomeReadOnly` | Linux, macOS | `--output`, `--ext-diff`, `--exec-path`, `--config-env`, `-c` | — | CR-12 git-cred-select; wildcard pathspec → Unread | §11 item 10 |
| `git diff` | `HomeReadOnly` | Linux, macOS | `--output`, `--ext-diff`, `--exec-path`, `--config-env`, `-c` | — | CR-12 git-cred-select; wildcard pathspec → Unread | §11 item 10 |
| `git show` | `HomeReadOnly` | Linux, macOS | `--output`, `--ext-diff`, `--exec-path`, `--config-env`, `-c` | — | CR-12 git-cred-select; wildcard pathspec → Unread | §11 item 10 |
| `git branch` | `HomeReadOnly` | Linux, macOS | `--output`, `--ext-diff`, `--exec-path`, `--config-env`, `-c` | `-a`, `--all`, `-r`, `--remotes`, `-v`, `-vv`, `--verbose`, `--list`, `-l`, `--show-current` | — | — |
| `git rev-parse` | `HomeReadOnly` | Linux, macOS | `--output`, `--ext-diff`, `--exec-path`, `--config-env`, `-c` | — | — | — |
| `git ls-files` | `HomeReadOnly` | Linux, macOS | `--output`, `--ext-diff`, `--exec-path`, `--config-env`, `-c` | — | wildcard pathspec → Unread | §11 item 10 |

## 2. `Search` rows

| Tool | Kind | Platforms | Flags | Filter dialect | Recursion | Default links | Home read-only | Changes default (§11) |
|---|---|---|---|---|---|---|---|---|
| `rg` | `Search` | Linux, macOS | 83 listed flags (53 bool, 30 value; see flag table below) | `RgGlob` | always | root=Y, below=N | Yes (forbids --pre, --pre-glob, -z, --search-zip, --hostname-bin, -f, --file) [recursive] [allows followed search (§11 item 15)] | §11 item 2; §11 item 9; §11 item 15 |
| `grep` | `Search` | Linux, macOS | 56 listed flags (40 bool, 16 value; see flag table below) | `GrepInclude` | with flags (`-r`/`-R`/`-d`) | root=Y, below=N | Yes (forbids -f, --file) [recursive w/ flags] [allows followed search (§11 item 15)] | §11 item 2; §11 item 9; §11 item 15 |
| `egrep` | `Search` | Linux, macOS | 56 listed flags (40 bool, 16 value; see flag table below) | `GrepInclude` | with flags (`-r`/`-R`/`-d`) | root=Y, below=N | No | §11 item 2; §11 item 9 |
| `fgrep` | `Search` | Linux, macOS | 56 listed flags (40 bool, 16 value; see flag table below) | `GrepInclude` | with flags (`-r`/`-R`/`-d`) | root=Y, below=N | No | §11 item 2; §11 item 9 |
| `zgrep` | `Search` | Linux, macOS | 56 listed flags (40 bool, 16 value; see flag table below) | `GrepInclude` | with flags (`-r`/`-R`/`-d`) | root=Y, below=N | No | §11 item 2; §11 item 9 |
| `ggrep` | `Search` | Linux, macOS | 56 listed flags (40 bool, 16 value; see flag table below) | `GrepInclude` | with flags (`-r`/`-R`/`-d`) | root=Y, below=N | No | §11 item 2; §11 item 9 |
| `rgrep` | `Search` | Linux, macOS | 56 listed flags (40 bool, 16 value; see flag table below) | `GrepInclude` | always | root=Y, below=N | No | §11 item 2; §11 item 9 |

## 3. `Wrapper` and `NestedShell` rows

### `Wrapper` (§4)

| Tool | Kind | Platforms | Wrapper | Allowed flags | `--` allowed | Wrapper operands | Changes default (§11) |
|---|---|---|---|---|---|---|---|
| `command` | `Wrapper` | Linux, macOS | `Command` | none (any flag → Unread) | Yes | — | §11 item 10 |
| `exec` | `Wrapper` | Linux, macOS | `Exec` | none (any flag → Unread) | No | — | §11 item 10 |
| `nice` | `Wrapper` | Linux, macOS | `Nice` | `-n` (Text) | No | — | §11 item 9; §11 item 10 |
| `timeout` | `Wrapper` | Linux, macOS | `Timeout` | `-s` (Text), `-k` (Text) | No | [0] Text | §11 item 9; §11 item 10 |
| `time` | `Wrapper` | Linux, macOS | `Time` | none (any flag → Unread) | No | — | §11 item 10 |
| `env` | `Wrapper` | Linux, macOS | `Env` | none (any flag → Unread) | No | — | §11 item 2; §11 item 10 |

### `NestedShell` (§4)

| Tool | Kind | Platforms | Allowed flags | Script pin (§11 item 8) | Unpinned prefix invalid (§11 item 4) | Changes default (§11) |
|---|---|---|---|---|---|---|
| `sh` | `NestedShell` | Linux, macOS | `-e`, `-u`, `-x`, `-c` (Code) [runs-code] | `FirstOperand` | Yes | §11 item 2; §11 item 4; §11 item 8 |
| `bash` | `NestedShell` | Linux, macOS | `-e`, `-u`, `-x`, `-c` (Code) [runs-code] | `FirstOperand` | Yes | §11 item 2; §11 item 4; §11 item 8 |
| `dash` | `NestedShell` | Linux, macOS | `-e`, `-u`, `-x`, `-c` (Code) [runs-code] | `FirstOperand` | Yes | §11 item 2; §11 item 4; §11 item 8 |

## 4. `CodeRunner` rows

| Command | Kind | Platforms | Inline code | Program pin (§11 item 8) | Unpinned prefix invalid (§11 item 4) | Changes default (§11) |
|---|---|---|---|---|---|---|
| `awk` | `CodeRunner` | Linux, macOS | operand `[0]` | FlagValue (`-f`/`--file`) | Yes | §11 item 4; §11 item 8 |
| `gawk` | `CodeRunner` | Linux, macOS | operand `[0]` | FlagValue (`-f`/`--file`) | Yes | §11 item 4; §11 item 8 |
| `mawk` | `CodeRunner` | Linux, macOS | operand `[0]` | FlagValue (`-f`/`--file`) | Yes | §11 item 4; §11 item 8 |
| `perl` | `CodeRunner` | Linux, macOS | `-e`, `-E` | FirstOperand | Yes | §11 item 4; §11 item 8 |
| `python` | `CodeRunner` | Linux, macOS | `-c`, `-m` | FirstOperand | Yes | §11 item 4; §11 item 8 |
| `python3` | `CodeRunner` | Linux, macOS | `-c`, `-m` | FirstOperand | Yes | §11 item 4; §11 item 8 |
| `node` | `CodeRunner` | Linux, macOS | `-e`, `--eval`, `-p`, `--print` | FirstOperand | Yes | §11 item 4; §11 item 8 |
| `ruby` | `CodeRunner` | Linux, macOS | `-e` | FirstOperand | Yes | §11 item 4; §11 item 8 |
| `eval` | `CodeRunner` | Linux, macOS | operands `[0..]` | — | Yes | §11 item 4 |
| `xargs` | `CodeRunner` | Linux, macOS | operands `[0..]` | — | Yes | §11 item 4 |
| `source` | `CodeRunner` | Linux, macOS | — | FirstOperand | Yes | §11 item 4; §11 item 8 |
| `.` | `CodeRunner` | Linux, macOS | — | FirstOperand | Yes | §11 item 4; §11 item 8 |
| `npx` | `CodeRunner` | Linux, macOS | — | FirstOperand | Yes | §11 item 4; §11 item 8 |
| `cargo` | `CodeRunner` | Linux, macOS | — | CommandAndSubcommand | Yes | §11 item 4; §11 item 8 |
| `cargo test` | `CodeRunner` | Linux, macOS | — | CommandAndSubcommand | No | §11 item 8 |
| `cargo build` | `CodeRunner` | Linux, macOS | — | CommandAndSubcommand | No | §11 item 8 |
| `cargo run` | `CodeRunner` | Linux, macOS | — | CommandAndSubcommand | No | §11 item 8 |
| `cargo check` | `CodeRunner` | Linux, macOS | — | CommandAndSubcommand | No | §11 item 8 |
| `cargo bench` | `CodeRunner` | Linux, macOS | — | CommandAndSubcommand | No | §11 item 8 |
| `cargo doc` | `CodeRunner` | Linux, macOS | — | CommandAndSubcommand | No | §11 item 8 |
| `cargo clippy` | `CodeRunner` | Linux, macOS | — | CommandAndSubcommand | No | §11 item 8 |
| `pipx` | `CodeRunner` | Linux, macOS | — | — | Yes | §11 item 4 |
| `pipx run` | `CodeRunner` | Linux, macOS | — | FirstOperand | Yes | §11 item 4; §11 item 8 |
| `ssh` | `CodeRunner` | Linux, macOS | operands `[1..]` | — | Yes | §11 item 4 |
| `mosh` | `CodeRunner` | Linux, macOS | operands `[1..]` | — | Yes | §11 item 4 |
| `autossh` | `CodeRunner` | Linux, macOS | operands `[1..]` | — | Yes | §11 item 4 |
| `nsenter` | `CodeRunner` | Linux | operands `[0..]` | — | Yes | §11 item 4 |
| `unshare` | `CodeRunner` | Linux | operands `[0..]` | — | Yes | §11 item 4 |
| `docker` | `CodeRunner` | Linux, macOS | operands `[1..]` | — | Yes | §11 item 4 |
| `podman` | `CodeRunner` | Linux, macOS | operands `[1..]` | — | Yes | §11 item 4 |
| `nerdctl` | `CodeRunner` | Linux, macOS | operands `[1..]` | — | Yes | §11 item 4 |
| `kubectl` | `CodeRunner` | Linux, macOS | operands `[1..]` | — | Yes | §11 item 4 |
| `oc` | `CodeRunner` | Linux, macOS | operands `[1..]` | — | Yes | §11 item 4 |
| `lxc` | `CodeRunner` | Linux, macOS | operands `[1..]` | — | Yes | §11 item 4 |
| `incus` | `CodeRunner` | Linux, macOS | operands `[1..]` | — | Yes | §11 item 4 |
| `machinectl` | `CodeRunner` | Linux | operands `[1..]` | — | Yes | §11 item 4 |
| `systemd-run` | `CodeRunner` | Linux | operands `[0..]` | — | Yes | §11 item 4 |
| `systemd-nspawn` | `CodeRunner` | Linux | operands `[0..]` | — | Yes | §11 item 4 |
| `firejail` | `CodeRunner` | Linux | operands `[0..]` | — | Yes | §11 item 4 |
| `bwrap` | `CodeRunner` | Linux | operands `[0..]` | — | Yes | §11 item 4 |
| `flatpak-spawn` | `CodeRunner` | Linux | operands `[0..]` | — | Yes | §11 item 4 |
| `toolbox` | `CodeRunner` | Linux, macOS | operands `[1..]` | — | Yes | §11 item 4 |
| `distrobox` | `CodeRunner` | Linux, macOS | operands `[1..]` | — | Yes | §11 item 4 |
| `distrobox-enter` | `CodeRunner` | Linux, macOS | operands `[1..]` | — | Yes | §11 item 4 |
| `multipass` | `CodeRunner` | Linux, macOS | operands `[1..]` | — | Yes | §11 item 4 |
| `vagrant` | `CodeRunner` | Linux, macOS | operands `[1..]` | — | Yes | §11 item 4 |
| `limactl` | `CodeRunner` | Linux, macOS | operands `[1..]` | — | Yes | §11 item 4 |
| `colima` | `CodeRunner` | Linux, macOS | operands `[1..]` | — | Yes | §11 item 4 |
| `su` | `CodeRunner` | Linux, macOS | operands `[0..]` | — | Yes | — |
| `runuser` | `CodeRunner` | Linux | operands `[1..]` | — | Yes | §11 item 4 |
| `setpriv` | `CodeRunner` | Linux | operands `[0..]` | — | Yes | §11 item 4 |
| `parallel` | `CodeRunner` | Linux, macOS | operands `[0..]` | — | Yes | §11 item 4 |

## 5. `ConditionalRunner` rows

| Tool | Kind | Platforms | Triggers (make unit Unread) | Unpinned prefix invalid (§11 item 4) | Changes default (§11) |
|---|---|---|---|---|---|
| `find` | `ConditionalRunner` | Linux, macOS | flag `-exec`; flag `-execdir`; flag `-ok`; flag `-okdir` | Yes | §11 item 4 |
| `git` | `ConditionalRunner` | Linux, macOS | flag `-c`; flag `--config-env`; flag `--exec-path`; subcommand `merge-index`; subcommand `filter-branch`; `bisect run`; `rebase -x`; `rebase --exec`; `submodule foreach`; `difftool -x`; `difftool --tool-cmd`; `mergetool -x`; `mergetool --tool-cmd`; `diff --ext-diff`; `push --receive-pack`; `push --exec` | Yes | §11 item 4 |
| `sed` | `ConditionalRunner` | Linux, macOS | script commands `e`, `w`, `r`; program file `-f` (pin: `FlagValue`); program file `--file` (pin: `FlagValue`) | Yes | §11 item 4; §11 item 8 |

## 6. `DerivedDestinationWriter` rows

| Command | Kind | Platforms | Trigger | Changes default (§11) |
|---|---|---|---|---|
| `git clone` | `DerivedDestinationWriter` | Linux, macOS | always | §11 item 10 |
| `git checkout` | `DerivedDestinationWriter` | Linux, macOS | always | §11 item 10 |
| `cp` | `DerivedDestinationWriter` | Linux, macOS | `-R`, `-r`, `-a`, `--recursive`, `--archive` | §11 item 10 |
| `mv` | `DerivedDestinationWriter` | Linux, macOS | when source is a directory | §11 item 10 |
| `rsync` | `DerivedDestinationWriter` | Linux, macOS | always | §11 item 10 |
| `tar` | `DerivedDestinationWriter` | Linux, macOS | `-x`, `--extract`, `--get` | §11 item 10 |
| `unzip` | `DerivedDestinationWriter` | Linux, macOS | always | §11 item 10 |

## Detailed flag tables

### `rg` (83 listed flags)

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
| `-f` | `--file` | Value(Path) | provides-pattern |
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

### `grep / egrep / fgrep / zgrep / ggrep / rgrep` (56 listed flags)

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
| `-f` | `--file` | Value(Path) | provides-pattern |
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

### `nice` (1 listed flags)

| Short | Long | Kind | Effects |
|---|---|---|---|
| `-n` | — | Value(Text) | none (scheduling priority increment; allowed wrapper option (§4)) |

### `timeout` (2 listed flags)

| Short | Long | Kind | Effects |
|---|---|---|---|
| `-s` | — | Value(Text) | none (termination signal name/number; allowed wrapper option (§4)) |
| `-k` | — | Value(Text) | none (kill-after duration; allowed wrapper option (§4)) |

### `sh / bash / dash` (4 listed flags)

| Short | Long | Kind | Effects |
|---|---|---|---|
| `-e` | — | Bool | none (shell errexit option; allowed before -c (§4)) |
| `-u` | — | Bool | none (shell nounset option; allowed before -c (§4)) |
| `-x` | — | Bool | none (shell xtrace option; allowed before -c (§4)) |
| `-c` | — | Value(Code) | runs-code |

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
| `nohup` | Linux, macOS | Yes | §4 | §11 item 4; §11 item 10 | Wrapper outside §4's closed list; makes the unit Unread |
| `builtin` | Linux, macOS | Yes | §4 | §11 item 4; §11 item 10 | Wrapper outside §4's closed list; makes the unit Unread |
| `busybox` | Linux | Yes | §4 | §11 item 4; §11 item 10 | Wrapper outside §4's closed list; makes the unit Unread |
| `toybox` | Linux | Yes | §4 | §11 item 4; §11 item 10 | Wrapper outside §4's closed list; makes the unit Unread |
| `setsid` | Linux | Yes | §4 | §11 item 4; §11 item 10 | Wrapper outside §4's closed list; makes the unit Unread |
| `unbuffer` | Linux, macOS | Yes | §4 | §11 item 4; §11 item 10 | Wrapper outside §4's closed list; makes the unit Unread |
| `caffeinate` | macOS | Yes | §4 | §11 item 4; §11 item 10 | Wrapper outside §4's closed list; makes the unit Unread |
| `sandbox-exec` | macOS | Yes | §4 | §11 item 4; §11 item 10 | Wrapper outside §4's closed list; makes the unit Unread |
| `chroot` | Linux, macOS | Yes | §4 | §11 item 4; §11 item 10 | Wrapper outside §4's closed list; makes the unit Unread |
| `ionice` | Linux | Yes | §4 | §11 item 4; §11 item 10 | Wrapper outside §4's closed list; makes the unit Unread |
| `stdbuf` | Linux, macOS | Yes | §4 | §11 item 4; §11 item 10 | Wrapper outside §4's closed list; makes the unit Unread |
| `chrt` | Linux | Yes | §4 | §11 item 4; §11 item 10 | Wrapper outside §4's closed list; makes the unit Unread |
| `taskset` | Linux | Yes | §4 | §11 item 4; §11 item 10 | Wrapper outside §4's closed list; makes the unit Unread |
| `flock` | Linux | Yes | §4 | §11 item 4; §11 item 10 | Wrapper outside §4's closed list; makes the unit Unread |
| `script` | Linux, macOS | Yes | §4 | §11 item 4; §11 item 10 | Wrapper outside §4's closed list; makes the unit Unread |
| `arch` | macOS | Yes | §4 | §11 item 4; §11 item 10 | Wrapper outside §4's closed list; makes the unit Unread |
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

## Unknown tools by design (`UNKNOWN_BY_DESIGN`)

Every tool below leaves the table under D-141 and receives §4's unknown-tool treatment: every word is a candidate `Path` (`ReadWrite`), the unit is a possible writer (§5), and it is never Home-eligible.

| Tool | `dev` list | Reason |
|---|---|---|
| `rm` | `words::WRITERS` | Not Home-eligible; recursive delete checked by hard-deny pass, otherwise unknown-tool treatment (§4) |
| `tee` | `words::WRITERS` | Writer with no derived destinations or code triggers; unknown-tool treatment (§4) |
| `touch` | `words::WRITERS` | Writer with no derived destinations or code triggers; unknown-tool treatment (§4) |
| `chmod` | `words::WRITERS` | Writer with no derived destinations or code triggers; unknown-tool treatment (§4) |
| `chown` | `words::WRITERS` | Writer with no derived destinations or code triggers; unknown-tool treatment (§4) |
| `chgrp` | `words::WRITERS` | Writer with no derived destinations or code triggers; unknown-tool treatment (§4) |
| `ln` | `words::WRITERS` | Every word checked as ReadWrite candidate path (as on dev), so protected link targets remain hits (§4, §6) |
| `truncate` | `words::WRITERS` | Writer with no derived destinations or code triggers; unknown-tool treatment (§4) |
| `install` | `words::WRITERS` | Writer with no derived destinations or code triggers; unknown-tool treatment (§4) |
| `mkdir` | `words::WRITERS` | Writer with no derived destinations or code triggers; unknown-tool treatment (§4) |
| `rmdir` | `words::WRITERS` | Writer with no derived destinations or code triggers; unknown-tool treatment (§4) |
| `unlink` | `words::WRITERS` | Writer with no derived destinations or code triggers; unknown-tool treatment (§4) |
| `shred` | `words::WRITERS` | Writer with no derived destinations or code triggers; unknown-tool treatment (§4) |
| `patch` | `words::WRITERS` | Removed per D-141; unknown-tool treatment (possible writer, every word a ReadWrite candidate path) |
| `ed` | `words::WRITERS` | Removed per D-141; never Home-eligible, possible writer, every word a ReadWrite candidate path |
| `ex` | `words::WRITERS` | Removed per D-141; never Home-eligible, possible writer, every word a ReadWrite candidate path |
| `wipefs` | `words::WRITERS` | Checked by destructive-disk hard-deny pass; otherwise unknown-tool treatment (§4) |
| `blkdiscard` | `words::WRITERS` | Checked by destructive-disk hard-deny pass; otherwise unknown-tool treatment (§4) |
| `less` | `words::READERS` | Not in HOME_READONLY_COMMANDS; unknown-tool treatment (§4) |
| `more` | `words::READERS` | Not in HOME_READONLY_COMMANDS; unknown-tool treatment (§4) |
| `ag` | `words::READERS` | Outside rg/grep search family with no modeled filter dialect; unknown-tool treatment (D-141) |
| `ack` | `words::READERS` | Outside rg/grep search family with no modeled filter dialect; unknown-tool treatment (D-141) |
| `diff` | `words::READERS` | Not in HOME_READONLY_COMMANDS; unknown-tool treatment (§4) |
| `cmp` | `words::READERS` | Not in HOME_READONLY_COMMANDS; unknown-tool treatment (§4) |
| `md5` | `words::READERS` | Not in HOME_READONLY_COMMANDS; unknown-tool treatment (§4) |
| `md5sum` | `words::READERS` | Not in HOME_READONLY_COMMANDS; unknown-tool treatment (§4) |
| `shasum` | `words::READERS` | Not in HOME_READONLY_COMMANDS; unknown-tool treatment (§4) |
| `sha1sum` | `words::READERS` | Not in HOME_READONLY_COMMANDS; unknown-tool treatment (§4) |
| `sha256sum` | `words::READERS` | Not in HOME_READONLY_COMMANDS; unknown-tool treatment (§4) |
| `echo` | `words::READERS` | Not in HOME_READONLY_COMMANDS; unknown-tool treatment (§4) |
| `printf` | `words::READERS` | Not in HOME_READONLY_COMMANDS; unknown-tool treatment (§4) |
| `whereis` | `words::READERS` | Not in HOME_READONLY_COMMANDS; unknown-tool treatment (§4) |
| `type` | `words::READERS` | Not in HOME_READONLY_COMMANDS; unknown-tool treatment (§4) |
| `readlink` | `words::READERS` | Not in HOME_READONLY_COMMANDS; unknown-tool treatment (§4) |
| `realpath` | `words::READERS` | Not in HOME_READONLY_COMMANDS; unknown-tool treatment (§4) |
| `basename` | `words::READERS` | Not in HOME_READONLY_COMMANDS; unknown-tool treatment (§4) |
| `dirname` | `words::READERS` | Not in HOME_READONLY_COMMANDS; unknown-tool treatment (§4) |
| `test` | `words::READERS` | Not in HOME_READONLY_COMMANDS; unknown-tool treatment (§4) |
| `[` | `words::READERS` | Not in HOME_READONLY_COMMANDS; unknown-tool treatment (§4) |
| `sort` | `words::READERS` | Not in HOME_READONLY_COMMANDS; unknown-tool treatment (§4) |
| `uniq` | `words::READERS` | Not in HOME_READONLY_COMMANDS; unknown-tool treatment (§4) |
| `cut` | `words::READERS` | Not in HOME_READONLY_COMMANDS; unknown-tool treatment (§4) |
| `tr` | `words::READERS` | Not in HOME_READONLY_COMMANDS; unknown-tool treatment (§4) |
| `nl` | `words::READERS` | Not in HOME_READONLY_COMMANDS; unknown-tool treatment (§4) |
| `od` | `words::READERS` | Not in HOME_READONLY_COMMANDS; unknown-tool treatment (§4) |
| `xxd` | `words::READERS` | Not in HOME_READONLY_COMMANDS; unknown-tool treatment (§4) |
| `hexdump` | `words::READERS` | Not in HOME_READONLY_COMMANDS; unknown-tool treatment (§4) |
| `strings` | `words::READERS` | Not in HOME_READONLY_COMMANDS; unknown-tool treatment (§4) |
| `bat` | `words::READERS` | Not in HOME_READONLY_COMMANDS; unknown-tool treatment (§4) |
| `jq` | `words::READERS` | Not in HOME_READONLY_COMMANDS; unknown-tool treatment (§4) |
| `column` | `words::READERS` | Not in HOME_READONLY_COMMANDS; unknown-tool treatment (§4) |
| `fold` | `words::READERS` | Not in HOME_READONLY_COMMANDS; unknown-tool treatment (§4) |
| `expand` | `words::READERS` | Not in HOME_READONLY_COMMANDS; unknown-tool treatment (§4) |
| `true` | `words::READERS` | Not in HOME_READONLY_COMMANDS; unknown-tool treatment (§4) |
| `false` | `words::READERS` | Not in HOME_READONLY_COMMANDS; unknown-tool treatment (§4) |
| `id` | `words::READERS` | Not in HOME_READONLY_COMMANDS; unknown-tool treatment (§4) |
| `cd` | `words::READERS` | Handled directly by P2's §5 cwd tracker, not a D-141 tool-table row |
| `open` | `words::READERS` | Not in HOME_READONLY_COMMANDS; unknown-tool treatment (§4) |
| `spctl` | `words::READERS` | Not in HOME_READONLY_COMMANDS; unknown-tool treatment (§4) |
| `mdls` | `words::READERS` | Not in HOME_READONLY_COMMANDS; unknown-tool treatment (§4) |
| `mdfind` | `words::READERS` | Not in HOME_READONLY_COMMANDS; unknown-tool treatment (§4) |
| `codesign` | `words::READERS` | Removed per D-141; not in HOME_READONLY_COMMANDS, unknown-tool treatment (§4) |
| `curl` | `deny::DOWNLOADERS` | Checked by pipe-to-shell hard-deny pass; otherwise unknown-tool treatment (§4) |
| `wget` | `deny::DOWNLOADERS` | Checked by pipe-to-shell hard-deny pass; otherwise unknown-tool treatment (§4) |
| `fetch` | `deny::DOWNLOADERS` | Checked by pipe-to-shell hard-deny pass; otherwise unknown-tool treatment (§4) |
| `sudo` | `deny::PRIV` | Always HardDeny (privilege-escalation) before tool classification; unknown-tool treatment otherwise |
| `sudoedit` | `deny::PRIV` | Always HardDeny (privilege-escalation) before tool classification; unknown-tool treatment otherwise |
| `doas` | `deny::PRIV` | Always HardDeny (privilege-escalation) before tool classification; unknown-tool treatment otherwise |
| `pkexec` | `deny::PRIV` | Always HardDeny (privilege-escalation) before tool classification; unknown-tool treatment otherwise |
| `run0` | `deny::PRIV` | Always HardDeny (privilege-escalation) before tool classification; unknown-tool treatment otherwise |

## Summary of rows changing `dev` defaults (§11)

| Entry | Kind | §11 item(s) |
|---|---|---|
| `rg` | `Search` | §11 item 2; §11 item 9; §11 item 15 |
| `grep` | `Search` | §11 item 2; §11 item 9; §11 item 15 |
| `egrep` | `Search` | §11 item 2; §11 item 9 |
| `fgrep` | `Search` | §11 item 2; §11 item 9 |
| `zgrep` | `Search` | §11 item 2; §11 item 9 |
| `ggrep` | `Search` | §11 item 2; §11 item 9 |
| `rgrep` | `Search` | §11 item 2; §11 item 9 |
| `command` | `Wrapper` | §11 item 10 |
| `exec` | `Wrapper` | §11 item 10 |
| `nice` | `Wrapper` | §11 item 9; §11 item 10 |
| `timeout` | `Wrapper` | §11 item 9; §11 item 10 |
| `time` | `Wrapper` | §11 item 10 |
| `env` | `Wrapper` | §11 item 2; §11 item 10 |
| `sh` | `NestedShell` | §11 item 2; §11 item 4; §11 item 8 |
| `bash` | `NestedShell` | §11 item 2; §11 item 4; §11 item 8 |
| `dash` | `NestedShell` | §11 item 2; §11 item 4; §11 item 8 |
| `awk` | `CodeRunner` | §11 item 4; §11 item 8 |
| `gawk` | `CodeRunner` | §11 item 4; §11 item 8 |
| `mawk` | `CodeRunner` | §11 item 4; §11 item 8 |
| `perl` | `CodeRunner` | §11 item 4; §11 item 8 |
| `python` | `CodeRunner` | §11 item 4; §11 item 8 |
| `python3` | `CodeRunner` | §11 item 4; §11 item 8 |
| `node` | `CodeRunner` | §11 item 4; §11 item 8 |
| `ruby` | `CodeRunner` | §11 item 4; §11 item 8 |
| `eval` | `CodeRunner` | §11 item 4 |
| `xargs` | `CodeRunner` | §11 item 4 |
| `source` | `CodeRunner` | §11 item 4; §11 item 8 |
| `.` | `CodeRunner` | §11 item 4; §11 item 8 |
| `npx` | `CodeRunner` | §11 item 4; §11 item 8 |
| `cargo` | `CodeRunner` | §11 item 4; §11 item 8 |
| `cargo test` | `CodeRunner` | §11 item 8 |
| `cargo build` | `CodeRunner` | §11 item 8 |
| `cargo run` | `CodeRunner` | §11 item 8 |
| `cargo check` | `CodeRunner` | §11 item 8 |
| `cargo bench` | `CodeRunner` | §11 item 8 |
| `cargo doc` | `CodeRunner` | §11 item 8 |
| `cargo clippy` | `CodeRunner` | §11 item 8 |
| `pipx` | `CodeRunner` | §11 item 4 |
| `pipx run` | `CodeRunner` | §11 item 4; §11 item 8 |
| `ssh` | `CodeRunner` | §11 item 4 |
| `mosh` | `CodeRunner` | §11 item 4 |
| `autossh` | `CodeRunner` | §11 item 4 |
| `nsenter` | `CodeRunner` | §11 item 4 |
| `unshare` | `CodeRunner` | §11 item 4 |
| `docker` | `CodeRunner` | §11 item 4 |
| `podman` | `CodeRunner` | §11 item 4 |
| `nerdctl` | `CodeRunner` | §11 item 4 |
| `kubectl` | `CodeRunner` | §11 item 4 |
| `oc` | `CodeRunner` | §11 item 4 |
| `lxc` | `CodeRunner` | §11 item 4 |
| `incus` | `CodeRunner` | §11 item 4 |
| `machinectl` | `CodeRunner` | §11 item 4 |
| `systemd-run` | `CodeRunner` | §11 item 4 |
| `systemd-nspawn` | `CodeRunner` | §11 item 4 |
| `firejail` | `CodeRunner` | §11 item 4 |
| `bwrap` | `CodeRunner` | §11 item 4 |
| `flatpak-spawn` | `CodeRunner` | §11 item 4 |
| `toolbox` | `CodeRunner` | §11 item 4 |
| `distrobox` | `CodeRunner` | §11 item 4 |
| `distrobox-enter` | `CodeRunner` | §11 item 4 |
| `multipass` | `CodeRunner` | §11 item 4 |
| `vagrant` | `CodeRunner` | §11 item 4 |
| `limactl` | `CodeRunner` | §11 item 4 |
| `colima` | `CodeRunner` | §11 item 4 |
| `runuser` | `CodeRunner` | §11 item 4 |
| `setpriv` | `CodeRunner` | §11 item 4 |
| `parallel` | `CodeRunner` | §11 item 4 |
| `find` | `ConditionalRunner` | §11 item 4 |
| `git` | `ConditionalRunner` | §11 item 4 |
| `git status` | `HomeReadOnly` | §11 item 10 |
| `git log` | `HomeReadOnly` | §11 item 10 |
| `git diff` | `HomeReadOnly` | §11 item 10 |
| `git show` | `HomeReadOnly` | §11 item 10 |
| `git ls-files` | `HomeReadOnly` | §11 item 10 |
| `git clone` | `DerivedDestinationWriter` | §11 item 10 |
| `git checkout` | `DerivedDestinationWriter` | §11 item 10 |
| `sed` | `ConditionalRunner` | §11 item 4; §11 item 8 |
| `cp` | `DerivedDestinationWriter` | §11 item 10 |
| `mv` | `DerivedDestinationWriter` | §11 item 10 |
| `rsync` | `DerivedDestinationWriter` | §11 item 10 |
| `tar` | `DerivedDestinationWriter` | §11 item 10 |
| `unzip` | `DerivedDestinationWriter` | §11 item 10 |
| `zsh` | `OutsideModel` | §11 item 4 |
| `ksh` | `OutsideModel` | §11 item 4 |
| `fish` | `OutsideModel` | §11 item 4 |
| `ash` | `OutsideModel` | §11 item 4 |
| `csh` | `OutsideModel` | §11 item 4 |
| `tcsh` | `OutsideModel` | §11 item 4 |
| `mksh` | `OutsideModel` | §11 item 4 |
| `yash` | `OutsideModel` | §11 item 4 |
| `rbash` | `OutsideModel` | §11 item 4 |
| `posh` | `OutsideModel` | §11 item 4 |
| `elvish` | `OutsideModel` | §11 item 4 |
| `xonsh` | `OutsideModel` | §11 item 4 |
| `nohup` | `OutsideModel` | §11 item 4; §11 item 10 |
| `builtin` | `OutsideModel` | §11 item 4; §11 item 10 |
| `busybox` | `OutsideModel` | §11 item 4; §11 item 10 |
| `toybox` | `OutsideModel` | §11 item 4; §11 item 10 |
| `setsid` | `OutsideModel` | §11 item 4; §11 item 10 |
| `unbuffer` | `OutsideModel` | §11 item 4; §11 item 10 |
| `caffeinate` | `OutsideModel` | §11 item 4; §11 item 10 |
| `sandbox-exec` | `OutsideModel` | §11 item 4; §11 item 10 |
| `chroot` | `OutsideModel` | §11 item 4; §11 item 10 |
| `ionice` | `OutsideModel` | §11 item 4; §11 item 10 |
| `stdbuf` | `OutsideModel` | §11 item 4; §11 item 10 |
| `chrt` | `OutsideModel` | §11 item 4; §11 item 10 |
| `taskset` | `OutsideModel` | §11 item 4; §11 item 10 |
| `flock` | `OutsideModel` | §11 item 4; §11 item 10 |
| `script` | `OutsideModel` | §11 item 4; §11 item 10 |
| `arch` | `OutsideModel` | §11 item 4; §11 item 10 |
