use std::env;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub enum Command {
    Delete(DeleteOptions),
    List,
    Restore(RestoreOptions),
    Purge(PurgeOptions),
    Unlink(UnlinkOptions),
    Help,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum RequestedMode {
    #[default]
    Auto,
    Interactive,
    Batch,
}

impl RequestedMode {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "auto" => Ok(Self::Auto),
            "interactive" => Ok(Self::Interactive),
            "batch" => Ok(Self::Batch),
            _ => Err(format!(
                "invalid mode: {value} (expected auto, interactive, or batch)"
            )),
        }
    }

    fn from_env() -> Result<Self, String> {
        match env::var("SURE_RM_MODE") {
            Ok(value) => Self::parse(&value).map_err(|_| {
                format!("invalid SURE_RM_MODE: {value} (expected auto, interactive, or batch)")
            }),
            Err(env::VarError::NotPresent) => Ok(Self::Auto),
            Err(env::VarError::NotUnicode(_)) => {
                Err("invalid SURE_RM_MODE: value must be valid UTF-8".to_string())
            }
        }
    }
}

#[derive(Debug, Default)]
pub struct DeleteOptions {
    pub allow_dir: bool,
    pub force: bool,
    pub interactive_each: bool,
    pub interactive_once: bool,
    pub mode: RequestedMode,
    pub one_file_system: bool,
    pub permanent: bool,
    pub recursive: bool,
    pub verbose: bool,
    pub paths: Vec<PathBuf>,
}

#[derive(Debug)]
pub struct RestoreOptions {
    pub id: String,
    pub destination: Option<PathBuf>,
}

#[derive(Debug, Default)]
pub struct PurgeOptions {
    pub all: bool,
    pub ids: Vec<String>,
}

#[derive(Debug)]
pub struct UnlinkOptions {
    pub path: PathBuf,
}

pub fn parse_args() -> Result<Command, String> {
    let mut args = env::args_os();
    let program = args.next();
    let rest: Vec<OsString> = args.collect();

    if invoked_as_unlink(program.as_ref()) {
        return parse_unlink(rest);
    }

    // Strip leading --mode/--mode=... so subcommands work with aliases
    // like `alias rm='sure-rm --mode interactive'`.
    // The mode value is preserved and re-injected for delete commands.
    let mut rest = rest;
    let mut leading_mode: Vec<OsString> = Vec::new();
    loop {
        let Some(first) = rest.first() else {
            return Ok(Command::Help);
        };
        if let Some(text) = first.to_str() {
            if text == "--mode" {
                leading_mode.extend(rest.drain(..2));
                continue;
            }
            if text.starts_with("--mode=") {
                leading_mode.push(rest.remove(0));
                continue;
            }
        }
        break;
    }

    let first = rest[0].clone();

    match first.to_str() {
        Some("help") => Ok(Command::Help),
        Some("--help") | Some("-h") => Ok(Command::Help),
        Some("unlink") => parse_unlink(rest[1..].to_vec()),
        Some("list") => {
            if rest.len() > 1 {
                Err("list does not accept positional arguments".to_string())
            } else {
                Ok(Command::List)
            }
        }
        Some("restore") => parse_restore(rest[1..].to_vec()),
        Some("purge") => parse_purge(rest[1..].to_vec()),
        _ => {
            // Re-inject --mode for delete parsing
            let mut delete_args = leading_mode;
            delete_args.push(first);
            delete_args.extend(rest[1..].iter().cloned());
            let first = delete_args.remove(0);
            parse_delete(first, delete_args)
        }
    }
}

pub fn usage() -> &'static str {
    "\
sure-rm 0.2.2

Usage:
  sure-rm [OPTIONS] <PATH>...
  sure-rm list
  sure-rm restore <ID> [--to <PATH>]
  sure-rm purge [--all] [ID...]
  sure-rm unlink [--] <PATH>

Delete options:
  -d          allow removing an empty directory without -r
  -r, -R      allow directory removal
  -f          ignore missing files and disable per-path prompts
  -i          ask before every removal
  -I          ask once before removing many paths or any directory
  -s, --sure  bypass sure-rm and exec the system rm/unlink command
  --mode      auto, interactive, or batch
  -x          refuse recursive operations that would cross filesystem boundaries
  -P          permanently delete instead of moving into sure-rm trash
  -v          print where the entry was moved
  -h, --help  show this help

By default sure-rm moves paths into its trash instead of hard-deleting them.
SURE_RM_MODE can also set auto, interactive, or batch.
If invoked as unlink, sure-rm accepts a single path and still performs safe removal.
"
}

pub fn invoked_as_unlink(program: Option<&OsString>) -> bool {
    let Some(program) = program else {
        return false;
    };

    Path::new(program)
        .file_name()
        .and_then(|name| name.to_str())
        == Some("unlink")
}

fn parse_restore(args: Vec<OsString>) -> Result<Command, String> {
    let mut id: Option<String> = None;
    let mut destination: Option<PathBuf> = None;
    let mut iter = args.into_iter();

    while let Some(arg) = iter.next() {
        match arg.to_str() {
            Some("--to") => {
                let Some(path) = iter.next() else {
                    return Err("restore requires a path after --to".to_string());
                };
                destination = Some(PathBuf::from(path));
            }
            Some("--help") | Some("-h") => return Ok(Command::Help),
            Some(value) if value.starts_with('-') => {
                return Err(format!("unknown restore option: {value}"));
            }
            Some(value) => {
                if id.is_some() {
                    return Err("restore accepts exactly one id".to_string());
                }
                id = Some(value.to_string());
            }
            None => return Err("restore arguments must be valid UTF-8".to_string()),
        }
    }

    let id = id.ok_or_else(|| "restore requires an id".to_string())?;
    Ok(Command::Restore(RestoreOptions { id, destination }))
}

fn parse_purge(args: Vec<OsString>) -> Result<Command, String> {
    let mut options = PurgeOptions::default();

    for arg in args {
        match arg.to_str() {
            Some("--all") => options.all = true,
            Some("--help") | Some("-h") => return Ok(Command::Help),
            Some(value) if value.starts_with('-') => {
                return Err(format!("unknown purge option: {value}"));
            }
            Some(value) => options.ids.push(value.to_string()),
            None => return Err("purge arguments must be valid UTF-8".to_string()),
        }
    }

    Ok(Command::Purge(options))
}

fn parse_unlink(args: Vec<OsString>) -> Result<Command, String> {
    let mut path: Option<PathBuf> = None;
    let mut parsing_options = true;

    for arg in args {
        if parsing_options && (arg == "--help" || arg == "-h") {
            return Ok(Command::Help);
        }

        if parsing_options && arg == "--" {
            parsing_options = false;
            continue;
        }

        if parsing_options
            && let Some(text) = arg.to_str()
            && text.starts_with('-')
        {
            return Err("unlink does not accept options other than --".to_string());
        }

        if path.is_some() {
            return Err("unlink accepts exactly one path".to_string());
        }

        path = Some(PathBuf::from(arg));
    }

    let path = path.ok_or_else(|| "unlink requires exactly one path".to_string())?;
    Ok(Command::Unlink(UnlinkOptions { path }))
}

fn parse_delete(first: OsString, rest: Vec<OsString>) -> Result<Command, String> {
    let mut options = DeleteOptions {
        mode: RequestedMode::from_env()?,
        ..DeleteOptions::default()
    };
    let mut parsing_options = true;

    let mut args = Vec::with_capacity(rest.len() + 1);
    args.push(first);
    args.extend(rest);

    let mut iter = args.into_iter();

    while let Some(arg) = iter.next() {
        if parsing_options && arg == "--help" {
            return Ok(Command::Help);
        }

        if parsing_options && arg == "--" {
            parsing_options = false;
            continue;
        }

        if parsing_options {
            if let Some(text) = arg.to_str() {
                if let Some(value) = text.strip_prefix("--mode=") {
                    options.mode = RequestedMode::parse(value)?;
                    continue;
                }

                if text == "--mode" {
                    let Some(value) = iter.next() else {
                        return Err("missing value after --mode".to_string());
                    };
                    let value = value
                        .to_str()
                        .ok_or_else(|| "mode must be valid UTF-8".to_string())?;
                    options.mode = RequestedMode::parse(value)?;
                    continue;
                }

                if text.starts_with("--") {
                    return Err(format!("unknown option: {text}"));
                }

                if text.starts_with('-') && text.len() > 1 {
                    for short in text[1..].chars() {
                        match short {
                            'd' => options.allow_dir = true,
                            'r' | 'R' => options.recursive = true,
                            'f' => {
                                options.force = true;
                                options.interactive_each = false;
                            }
                            'i' => {
                                options.interactive_each = true;
                                options.force = false;
                            }
                            'I' => options.interactive_once = true,
                            'x' => options.one_file_system = true,
                            'P' => options.permanent = true,
                            'v' => options.verbose = true,
                            'W' => return Err("sure-rm no longer supports -W; use `sure-rm restore` instead".to_string()),
                            'h' => return Ok(Command::Help),
                            _ => return Err(format!("unknown option: -{short}")),
                        }
                    }
                    continue;
                }
            }
        }

        options.paths.push(PathBuf::from(arg));
    }


    Ok(Command::Delete(options))
}

#[cfg(test)]
mod tests {
    use super::{Command, RequestedMode, invoked_as_unlink, parse_delete, parse_unlink};
    use std::ffi::OsString;
    use std::path::PathBuf;

    #[test]
    fn parses_combined_short_flags() {
        let command = parse_delete(
            OsString::from("-rfv"),
            vec![OsString::from("target"), OsString::from("other")],
        )
        .unwrap();

        let Command::Delete(options) = command else {
            panic!("expected delete command");
        };

        assert!(options.recursive);
        assert!(options.force);
        assert!(options.verbose);
        assert_eq!(options.paths.len(), 2);
    }

    #[test]
    fn i_overrides_previous_f() {
        let command = parse_delete(OsString::from("-fi"), vec![OsString::from("target")]).unwrap();

        let Command::Delete(options) = command else {
            panic!("expected delete command");
        };

        assert!(!options.force);
        assert!(options.interactive_each);
    }

    #[test]
    fn parses_d_x_p_flags() {
        let command =
            parse_delete(OsString::from("-dxPv"), vec![OsString::from("target")]).unwrap();

        let Command::Delete(options) = command else {
            panic!("expected delete command");
        };

        assert!(options.allow_dir);
        assert!(options.one_file_system);
        assert!(options.permanent);
        assert!(options.verbose);
    }

    #[test]
    fn parses_mode_long_option() {
        let command = parse_delete(
            OsString::from("--mode"),
            vec![OsString::from("interactive"), OsString::from("target")],
        )
        .unwrap();

        let Command::Delete(options) = command else {
            panic!("expected delete command");
        };

        assert_eq!(options.mode, RequestedMode::Interactive);
        assert_eq!(options.paths, vec![PathBuf::from("target")]);
    }

    #[test]
    fn parses_mode_equals_syntax() {
        let command = parse_delete(
            OsString::from("--mode=batch"),
            vec![OsString::from("target")],
        )
        .unwrap();

        let Command::Delete(options) = command else {
            panic!("expected delete command");
        };

        assert_eq!(options.mode, RequestedMode::Batch);
    }

    #[test]
    fn parses_unlink_path() {
        let command = parse_unlink(vec![OsString::from("--"), OsString::from("-file")]).unwrap();

        let Command::Unlink(options) = command else {
            panic!("expected unlink command");
        };

        assert_eq!(options.path, PathBuf::from("-file"));
    }

    #[test]
    fn w_flag_returns_error() {
        let error =
            parse_delete(OsString::from("-W"), vec![OsString::from("target")]).unwrap_err();
        assert!(error.contains("restore"));
    }

    #[test]
    fn unlink_detects_program_name() {
        assert!(invoked_as_unlink(Some(&OsString::from(
            "/usr/local/bin/unlink"
        ))));
        assert!(!invoked_as_unlink(Some(&OsString::from(
            "/usr/local/bin/sure-rm"
        ))));
    }
}
