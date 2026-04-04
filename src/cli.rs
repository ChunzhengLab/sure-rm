use std::env;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub enum Invocation {
    Command(Command),
    Bypass(SureBypass),
}

#[derive(Debug, Eq, PartialEq)]
pub struct SureBypass {
    pub program: &'static str,
    pub args: Vec<OsString>,
}

#[derive(Debug)]
pub enum Command {
    Delete(DeleteOptions),
    List,
    Restore(RestoreOptions),
    Purge(PurgeOptions),
    Unlink(UnlinkOptions),
    Help(HelpTopic),
    Version,
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
    pub query: String,
    pub destination: Option<PathBuf>,
}

#[derive(Debug, Default)]
pub struct PurgeOptions {
    pub all: bool,
    pub queries: Vec<String>,
}

#[derive(Debug)]
pub struct UnlinkOptions {
    pub path: PathBuf,
}

pub fn parse_invocation() -> Result<Invocation, String> {
    let mut args = env::args_os();
    let program = args.next();
    let argv: Vec<OsString> = args.collect();

    // Strip --mode before bypass check so that alias scenarios like
    // `alias rm='sure-rm --mode interactive'` + `rm list --sure`
    // correctly see "list" as the first real arg, not "--mode".
    let (leading_mode, rest) = split_leading_mode(argv)?;

    if let Some(bypass) = build_sure_bypass(program.as_ref(), &rest) {
        return Ok(Invocation::Bypass(bypass));
    }

    let command = if invoked_as_unlink(program.as_ref()) {
        parse_unlink(rest)
    } else {
        parse_command(leading_mode, rest)
    }?;

    Ok(Invocation::Command(command))
}

fn parse_command(leading_mode: Vec<OsString>, rest: Vec<OsString>) -> Result<Command, String> {
    if rest.is_empty() {
        return Ok(Command::Help(HelpTopic::General));
    }

    match classify_first_arg(rest.first()) {
        FirstArg::Subcommand("--version") => Ok(Command::Version),
        FirstArg::Subcommand("help" | "--help" | "-h") => Ok(Command::Help(HelpTopic::General)),
        FirstArg::Subcommand("unlink") => parse_unlink(rest[1..].to_vec()),
        FirstArg::Subcommand("list") => parse_list(rest[1..].to_vec()),
        FirstArg::Subcommand("restore") => parse_restore(rest[1..].to_vec()),
        FirstArg::Subcommand("purge") => parse_purge(rest[1..].to_vec()),
        FirstArg::Subcommand(_) => unreachable!(),
        FirstArg::DeleteOrPath => {
            let mut delete_args = leading_mode;
            delete_args.extend(rest);
            parse_delete(delete_args)
        }
    }
}

fn split_leading_mode(argv: Vec<OsString>) -> Result<(Vec<OsString>, Vec<OsString>), String> {
    let mut rest = argv;
    let mut mode_args: Vec<OsString> = Vec::new();
    loop {
        let Some(first) = rest.first() else {
            break;
        };
        if let Some(text) = first.to_str() {
            if text == "--mode" {
                if rest.len() < 2 {
                    return Err("missing value after --mode".to_string());
                }
                mode_args.extend(rest.drain(..2));
                continue;
            }
            if text.starts_with("--mode=") {
                mode_args.push(rest.remove(0));
                continue;
            }
        }
        break;
    }
    Ok((mode_args, rest))
}

enum FirstArg<'a> {
    Subcommand(&'a str),
    DeleteOrPath,
}

fn classify_first_arg(arg: Option<&OsString>) -> FirstArg<'_> {
    match arg.and_then(|a| a.to_str()) {
        Some(name @ ("help" | "--help" | "-h" | "--version" | "list" | "restore" | "purge" | "unlink")) => {
            FirstArg::Subcommand(name)
        }
        _ => FirstArg::DeleteOrPath,
    }
}

fn has_sure_flag(args: &[OsString]) -> bool {
    for arg in args {
        if arg == "--" {
            return false;
        }
        if arg == "--sure" {
            return true;
        }
        if let Some(text) = arg.to_str()
            && text.starts_with('-')
            && !text.starts_with("--")
            && text.contains('s')
        {
            return true;
        }
    }
    false
}

pub fn build_sure_bypass(program: Option<&OsString>, argv: &[OsString]) -> Option<SureBypass> {
    if argv.is_empty() || !has_sure_flag(argv) {
        return None;
    }

    // Bypass only applies to delete and unlink, not to other subcommands.
    if let FirstArg::Subcommand(name) = classify_first_arg(argv.first())
        && name != "unlink"
    {
        return None;
    }

    if invoked_as_unlink(program) {
        return Some(SureBypass {
            program: "/bin/unlink",
            args: filter_passthrough_args(argv),
        });
    }

    let is_unlink_subcommand = argv.first().and_then(|a| a.to_str()) == Some("unlink");
    let (program, args) = if is_unlink_subcommand {
        ("/bin/unlink", filter_passthrough_args(&argv[1..]))
    } else {
        ("/bin/rm", filter_passthrough_args(argv))
    };

    Some(SureBypass { program, args })
}

pub fn filter_passthrough_args(args: &[OsString]) -> Vec<OsString> {
    let mut filtered = Vec::new();
    let mut parsing_options = true;
    let mut skip_mode_value = false;

    for arg in args {
        if skip_mode_value {
            skip_mode_value = false;
            continue;
        }

        if parsing_options {
            if arg == "--sure" {
                continue;
            }

            if arg == "--" {
                parsing_options = false;
                filtered.push(arg.clone());
                continue;
            }

            if arg == "--mode" {
                skip_mode_value = true;
                continue;
            }

            if let Some(text) = arg.to_str()
                && text.starts_with("--mode=")
            {
                continue;
            }

            // Strip -s from combined short flags, keep the rest
            if let Some(text) = arg.to_str()
                && text.starts_with('-')
                && !text.starts_with("--")
                && text.contains('s')
            {
                let remaining: String = text[1..].chars().filter(|&c| c != 's').collect();
                if !remaining.is_empty() {
                    filtered.push(OsString::from(format!("-{remaining}")));
                }
                continue;
            }
        }

        filtered.push(arg.clone());
    }

    filtered
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
  -r, -R         allow directory removal
  -d             allow removing an empty directory without -r
  -p             permanently delete instead of moving into sure-rm trash
  -x             refuse recursive operations that would cross filesystem boundaries
  -f             ignore missing files and disable per-path prompts
  -i             ask before every removal
  -I             ask once before removing many paths or any directory
  --mode         auto, interactive, or batch
  -s, --sure     bypass sure-rm and exec the system rm/unlink command
  -v             print where the entry was moved
  -h, --help     show this help
  --version      show version

",
        env!("CARGO_PKG_VERSION")
    )
}

pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
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
    let mut query: Option<String> = None;
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
                if query.is_some() {
                    return Err("restore accepts exactly one id or path".to_string());
                }
                query = Some(value.to_string());
            }
            None => return Err("restore arguments must be valid UTF-8".to_string()),
        }
    }

    let query = query.ok_or_else(|| "restore requires an id or path".to_string())?;
    Ok(Command::Restore(RestoreOptions { query, destination }))
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
            Some(value) => options.queries.push(value.to_string()),
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
    parse_delete_with_mode(RequestedMode::from_env()?, args)
}

fn parse_delete_with_mode(
    default_mode: RequestedMode,
    args: Vec<OsString>,
) -> Result<Command, String> {
    let mut options = DeleteOptions {
        mode: default_mode,
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

        if parsing_options && let Some(text) = arg.to_str() {
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

        options.paths.push(PathBuf::from(arg));
    }

    Ok(Command::Delete(options))
}

#[cfg(test)]
mod tests {
    use super::{
        Command, RequestedMode, SureBypass, build_sure_bypass, filter_passthrough_args,
        invoked_as_unlink, parse_delete_with_mode, parse_unlink,
    };
    use std::ffi::OsString;
    use std::path::PathBuf;

    fn os(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
    }

    #[test]
    fn parses_combined_short_flags() {
        let Command::Delete(options) =
            parse_delete_with_mode(RequestedMode::Auto, os(&["-rfv", "target", "other"])).unwrap()
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
        let Command::Delete(options) =
            parse_delete_with_mode(RequestedMode::Auto, os(&["-fi", "target"])).unwrap()
        else {
            panic!("expected delete command");
        };

        assert!(!options.force);
        assert!(options.interactive_each);
    }

    #[test]
    fn parses_d_x_p_flags() {
        let Command::Delete(options) =
            parse_delete_with_mode(RequestedMode::Auto, os(&["-dxpv", "target"])).unwrap()
        else {
            panic!("expected delete command");
        };

        assert!(options.allow_dir);
        assert!(options.one_file_system);
        assert!(options.permanent);
        assert!(options.verbose);
    }

    #[test]
    fn parses_mode_long_option() {
        let Command::Delete(options) = parse_delete_with_mode(
            RequestedMode::Auto,
            os(&["--mode", "interactive", "target"]),
        )
        .unwrap() else {
            panic!("expected delete command");
        };

        assert_eq!(options.mode, RequestedMode::Interactive);
        assert_eq!(options.paths, vec![PathBuf::from("target")]);
    }

    #[test]
    fn parses_mode_equals_syntax() {
        let Command::Delete(options) =
            parse_delete_with_mode(RequestedMode::Auto, os(&["--mode=batch", "target"])).unwrap()
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
        let error = parse_delete_with_mode(RequestedMode::Auto, os(&["-W", "target"])).unwrap_err();
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

    fn bypass(args: &[&str]) -> Option<SureBypass> {
        let argv = os(args);
        build_sure_bypass(None, &argv)
    }

    #[test]
    fn sure_after_double_dash_does_not_trigger_bypass() {
        assert!(bypass(&["--", "--sure"]).is_none());
    }

    #[test]
    fn sure_before_double_dash_still_triggers_bypass() {
        assert!(bypass(&["--sure", "--", "file"]).is_some());
    }

    #[test]
    fn sure_bypass_skips_non_delete_subcommands() {
        for sub in &["list", "restore", "purge", "help", "--help"] {
            assert!(
                bypass(&[sub, "--sure"]).is_none(),
                "should not bypass for {sub}"
            );
        }
    }

    #[test]
    fn sure_bypass_uses_rm_and_strips_sure_rm_flags() {
        let b = bypass(&["--mode", "interactive", "--sure", "-rf", "target"]).unwrap();
        assert_eq!(
            b,
            SureBypass {
                program: "/bin/rm",
                args: os(&["-rf", "target"]),
            }
        );
    }

    #[test]
    fn sure_bypass_uses_unlink_for_subcommand() {
        let b = bypass(&["unlink", "--sure", "--", "-file"]).unwrap();
        assert_eq!(
            b,
            SureBypass {
                program: "/bin/unlink",
                args: os(&["--", "-file"]),
            }
        );
    }

    #[test]
    fn short_s_triggers_sure_bypass() {
        let b = bypass(&["-sf", "target"]).unwrap();
        assert_eq!(
            b,
            SureBypass {
                program: "/bin/rm",
                args: os(&["-f", "target"]),
            }
        );
    }

    #[test]
    fn short_s_alone_triggers_sure_bypass() {
        let b = bypass(&["-s", "target"]).unwrap();
        assert_eq!(
            b,
            SureBypass {
                program: "/bin/rm",
                args: os(&["target"]),
            }
        );
    }

    #[test]
    fn filter_passthrough_args_keeps_operands_after_double_dash() {
        let filtered = filter_passthrough_args(&os(&[
            "--mode=interactive",
            "--",
            "--mode",
            "--sure",
            "file",
        ]));
        assert_eq!(filtered, os(&["--", "--mode", "--sure", "file"]));
    }
}
