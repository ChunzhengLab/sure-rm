# sure-rm

`sure-rm` is a safer `rm`-style tool written in Rust. It defaults to moving paths into a recoverable trash store instead of hard-deleting them.

## Current Scope

- Safer `rm`-style deletion for files, symlinks, and directories
- `list`, `restore`, and `purge` commands
- `-d`, `-f`, `-i`, `-I`, `-P`, `-r/-R`, `-v`, `-W`, and `-x`
- `--mode auto|interactive|batch`
- `--sure` to bypass sure-rm and run the system command
- `unlink`-style entry point:
  - `sure-rm unlink [--] <path>`
  - or invoke the binary via the name `unlink`

This project is intentionally not a bit-for-bit clone of system `rm`. The interface is familiar, but the default behavior is safer.

## Safety Model

- Default delete: move the target into sure-rm's trash
- `--sure`: bypass sure-rm completely and exec `/bin/rm` or `/bin/unlink`
- `-P`: permanently delete instead of using the trash
- `-W`: restore the latest trashed entry for a given original path
- Dangerous targets like `/`, `.`, `..`, the current working directory, and `HOME` are blocked

### Differences from BSD rm

| Flag | BSD rm | sure-rm |
|------|--------|---------|
| `-P` | No effect (kept for backwards compatibility) | Permanently delete, bypassing the trash |
| `-W` | Undelete via union filesystem whiteouts | Restore the latest trashed entry for the given path |

## Modes

`--mode` and `SURE_RM_MODE` support:

- `auto`: use TTY detection
- `interactive`: default to a one-time confirmation for riskier operations
- `batch`: do not add extra implicit confirmations

The `interactive` mode is intended for shell aliases such as:

```sh
alias rm='sure-rm --mode interactive'
```

In that setup, `rm --sure ...` becomes the escape hatch to the normal system command.

## Trash Root

By default the trash lives under `~/.sure-rm`.

For testing or sandboxed execution you can override it:

```sh
SURE_RM_ROOT=/tmp/sure-rm sure-rm -rv some-directory
```

## Examples

```sh
sure-rm -rv build
sure-rm --sure -rf build
sure-rm list
sure-rm restore 1774864212-68302-250054000
sure-rm -W ./notes.txt
sure-rm -Pv old.log
sure-rm unlink -- -file
```
