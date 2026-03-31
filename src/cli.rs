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
    Help(HelpTopic),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HelpTopic {
    General,
    List,
    Restore,
    Purge,
    Unlink,
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
            return Ok(Command::Help(HelpTopic::General));
        };
        if let Some(text) = first.to_str() {
            if text == "--mode" {
                if rest.len() < 2 {
                    return Err("missing value after --mode".to_string());
                }
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
        Some("help") => Ok(Command::Help(HelpTopic::General)),
        Some("--help") | Some("-h") => Ok(Command::Help(HelpTopic::General)),
        Some("unlink") => parse_unlink(rest[1..].to_vec()),
        Some("list") => parse_list(rest[1..].to_vec()),
        Some("restore") => parse_restore(rest[1..].to_vec()),
        Some("purge") => parse_purge(rest[1..].to_vec()),
        _ => {
            let mut delete_args = leading_mode;
            delete_args.extend(rest);
            parse_delete(delete_args)
        }
    }
}

pub fn usage() -> String {
    format!(
        "\
sure-rm {}
A safer rm that moves files to trash instead of deleting them.
Use -s/--sure to bypass and exec the system rm.

Usage:
  sure-rm [OPTIONS] <PATH>...
  sure-rm list
  sure-rm restore <ID> [--to <PATH>]
  sure-rm purge [--all] [ID...]
  sure-rm unlink [--] <PATH>

Options:
  -r, -R      allow directory removal
  -d          allow removing an empty directory without -r
  -p          permanently delete instead of moving into sure-rm trash
  -x          refuse recursive operations that would cross filesystem boundaries
  -f          ignore missing files and disable per-path prompts
  -i          ask before every removal
  -I          ask once before removing many paths or any directory
  --mode      auto, interactive, or batch
  -s, --sure  bypass sure-rm and exec the system rm/unlink command
  -v          print where the entry was moved
  -h, --help  show this help

",
        env!("CARGO_PKG_VERSION")
    )
}

pub fn subcommand_usage(topic: HelpTopic) -> String {
    match topic {
        HelpTopic::General => return usage(),
        HelpTopic::List => {
            "\
list all entries in sure-rm trash

Usage: sure-rm list

Options:
  -h, --help    show this help
"
        }
        HelpTopic::Restore => {
            "\
restore a trashed entry by id or by original path

Usage: sure-rm restore <ID|PATH> [--to <PATH>]

Options:
  --to <PATH>   restore to a different location
  -h, --help    show this help
"
        }
        HelpTopic::Purge => {
            "\
permanently delete entries from sure-rm trash

Usage: sure-rm purge [--all] [ID|PATH...]

Options:
  --all         purge all entries
  -h, --help    show this help
"
        }
        HelpTopic::Unlink => {
            "\
safely delete a single file

Usage: sure-rm unlink [--] <PATH>

Options:
  -h, --help    show this help
"
        }
    }
    .to_string()
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

fn parse_list(args: Vec<OsString>) -> Result<Command, String> {
    if let Some(arg) = args.first() {
        if matches!(arg.to_str(), Some("-h" | "--help")) {
            return Ok(Command::Help(HelpTopic::List));
        }
        return Err("list does not accept positional arguments".to_string());
    }
    Ok(Command::List)
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
            Some("--help") | Some("-h") => return Ok(Command::Help(HelpTopic::Restore)),
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
            Some("--help") | Some("-h") => return Ok(Command::Help(HelpTopic::Purge)),
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
            return Ok(Command::Help(HelpTopic::Unlink));
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

fn parse_delete(args: Vec<OsString>) -> Result<Command, String> {
    let mut options = DeleteOptions {
        mode: RequestedMode::from_env()?,
        ..DeleteOptions::default()
    };
    let mut parsing_options = true;

    let mut iter = args.into_iter();

    while let Some(arg) = iter.next() {
        if parsing_options && arg == "--help" {
            return Ok(Command::Help(HelpTopic::General));
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
                            'p' => options.permanent = true,
                            'v' => options.verbose = true,
                            'W' => {
                                return Err(
                                    "sure-rm no longer supports -W; use `sure-rm restore` instead"
                                        .to_string(),
                                );
                            }
                            'h' => return Ok(Command::Help(HelpTopic::General)),
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

    fn os(args: &[&str]) -> Vec<OsString> {
        args.iter().map(|s| OsString::from(s)).collect()
    }

    #[test]
    fn parses_combined_short_flags() {
        let Command::Delete(options) = parse_delete(os(&["-rfv", "target", "other"])).unwrap()
        else {
            panic!("expected delete command");
        };

        assert!(options.recursive);
        assert!(options.force);
        assert!(options.verbose);
        assert_eq!(options.paths.len(), 2);
    }

    #[test]
    fn i_overrides_previous_f() {
        let Command::Delete(options) = parse_delete(os(&["-fi", "target"])).unwrap() else {
            panic!("expected delete command");
        };

        assert!(!options.force);
        assert!(options.interactive_each);
    }

    #[test]
    fn parses_d_x_p_flags() {
        let Command::Delete(options) = parse_delete(os(&["-dxpv", "target"])).unwrap() else {
            panic!("expected delete command");
        };

        assert!(options.allow_dir);
        assert!(options.one_file_system);
        assert!(options.permanent);
        assert!(options.verbose);
    }

    #[test]
    fn parses_mode_long_option() {
        let Command::Delete(options) =
            parse_delete(os(&["--mode", "interactive", "target"])).unwrap()
        else {
            panic!("expected delete command");
        };

        assert_eq!(options.mode, RequestedMode::Interactive);
        assert_eq!(options.paths, vec![PathBuf::from("target")]);
    }

    #[test]
    fn parses_mode_equals_syntax() {
        let Command::Delete(options) = parse_delete(os(&["--mode=batch", "target"])).unwrap()
        else {
            panic!("expected delete command");
        };

        assert_eq!(options.mode, RequestedMode::Batch);
    }

    #[test]
    fn parses_unlink_path() {
        let Command::Unlink(options) = parse_unlink(os(&["--", "-file"])).unwrap() else {
            panic!("expected unlink command");
        };

        assert_eq!(options.path, PathBuf::from("-file"));
    }

    #[test]
    fn w_flag_returns_error() {
        let error = parse_delete(os(&["-W", "target"])).unwrap_err();
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
