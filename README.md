# sure-rm

[![build](https://github.com/ChunzhengLab/sure-rm/actions/workflows/rust.yml/badge.svg)](https://github.com/ChunzhengLab/sure-rm/actions/workflows/rust.yml) [![version](https://img.shields.io/github/v/tag/ChunzhengLab/sure-rm?label=version)](https://github.com/ChunzhengLab/sure-rm/releases) [![license](https://img.shields.io/github/license/ChunzhengLab/sure-rm)](LICENSE)

English | [中文](README.zh-cn.md)

`sure-rm` is a command-line tool written in Rust. It works just like the system `rm`.

Instead of permanently deleting files from disk, it moves them into a recoverable trash store. Files are only permanently deleted when you explicitly ask for it.

## Install

```sh
brew tap ChunzhengLab/tap
brew install sure-rm
```

See [homebrew-tap](https://github.com/ChunzhengLab/homebrew-tap) for details.

## Modes

`--mode` and the `SURE_RM_MODE` environment variable support three modes:

- `auto`: detect based on TTY
- `interactive`: prompt once for riskier operations
- `batch`: no extra prompts, execute directly

**The recommended setup is `interactive` mode with a shell alias:**

```sh
alias rm='sure-rm --mode interactive'
```

Or set the default mode via environment variable:

```sh
export SURE_RM_MODE=interactive
alias rm='sure-rm'
```

**This makes `rm` behave as sure-rm. When you need a real delete, use `rm -s ...` (or `rm --sure ...`) to fall back to the system command.**

> **Note:** Shell aliases only apply in interactive terminals. In scripts, `rm` remains `/bin/rm` and existing automation is unaffected.

## Features

Safely delete files, symlinks, and directories — moved to trash by default instead of permanent deletion.

### Subcommands

- `list` — view all entries in the trash
- `restore` — restore an entry by id or by original path
- `purge` — permanently clean the trash, by id, by path, or `--all`
- `unlink` — single-file delete entry point: `sure-rm unlink [--] <path>`

### Options

- Supports nearly all `rm` options: `-d`, `-f`, `-i`, `-I`, `-p`, `-r/-R`, `-v`, `-x`
- `-p` permanently delete, skip the trash
- `-s` / `--sure` bypass sure-rm, exec system `/bin/rm` or `/bin/unlink`
- `--mode auto|interactive|batch` control prompt behavior, also configurable via `SURE_RM_MODE`

### Safety

- Automatically blocks dangerous targets like `/`, `.`, `..`, current directory, and `HOME`

## Examples

```sh
# Set up alias
alias rm='sure-rm --mode interactive'

rm -rv build                           # move build/ to trash, verbose output
rm -sf build                           # bypass sure-rm, exec /bin/rm -f build
rm list                                # list all entries in the trash
rm restore 1774864212-68302-250054000  # restore a specific entry by id
rm restore ./notes.txt                 # restore by relative path
rm restore ../docs/notes.txt           # cross-directory relative path works too
rm restore /home/user/notes.txt        # restore by absolute path
rm -pv old.log                         # permanently delete, skip the trash
rm unlink -- -file                     # unlink a file named "-file"
```

## Trash

The trash is located at `~/.sure-rm` by default.

```sh
rm list                                # view trash contents
rm restore ./notes.txt                 # restore a file
rm purge 1774864212-68302-250054000    # permanently delete one entry by id
rm purge ./notes.txt                   # permanently delete one entry by path
rm purge --all                         # empty the trash
```

You can override the trash path via environment variable for testing or sandboxed execution:

```sh
SURE_RM_ROOT=/tmp/sure-rm sure-rm -rv some-directory
```

### Auto-expiry (TTL)

Set `SURE_RM_TTL` to automatically purge entries older than the specified duration. Expired entries are cleaned up when running `list`.

```sh
export SURE_RM_TTL=30d   # 30 days (default unit is days)
export SURE_RM_TTL=12h   # 12 hours
export SURE_RM_TTL=3600s # 3600 seconds
```

Set to `0` or leave unset to disable auto-expiry.

## Inspiration

Inspired by [jwanLab](https://github.com/jwanLab), who spent months building an awesome project, then deleted it in less than a second.
