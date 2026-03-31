mod cli;
mod store;

use std::env;
use std::fs;
use std::io::{IsTerminal, stderr, stdin};
use std::os::unix::fs::MetadataExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command as ProcessCommand, ExitCode};

use cli::{Command, DeleteOptions, PurgeOptions, RequestedMode, RestoreOptions, UnlinkOptions};
use store::{MoveOutcome, TrashRecord};

trait FmtErr<T> {
    fn fmt_err(self) -> Result<T, String>;
}

impl<T, E: std::fmt::Display> FmtErr<T> for Result<T, E> {
    fn fmt_err(self) -> Result<T, String> {
        self.map_err(|e| e.to_string())
    }
}

enum DeleteResult {
    PermanentlyDeleted(PathBuf),
    Trashed(TrashRecord),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EffectiveMode {
    Interactive,
    Batch,
}

#[derive(Debug, Eq, PartialEq)]
struct SureBypass {
    program: &'static str,
    args: Vec<std::ffi::OsString>,
}

fn main() -> ExitCode {
    if let Err(message) = maybe_exec_sure_bypass() {
        eprintln!("sure-rm: {message}");
        return ExitCode::from(1);
    }

    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("sure-rm: {message}");
            ExitCode::from(1)
        }
    }
}

fn run() -> Result<(), String> {
    match cli::parse_args()? {
        Command::Delete(options) => run_delete(options),
        Command::List => run_list(),
        Command::Restore(options) => run_restore(options),
        Command::Purge(options) => run_purge(options),
        Command::Unlink(options) => run_unlink(options),
        Command::Help(topic) => {
            print!("{}", cli::subcommand_usage(topic));
            Ok(())
        }
    }
}

fn maybe_exec_sure_bypass() -> Result<(), String> {
    let argv: Vec<std::ffi::OsString> = env::args_os().collect();

    let Some(bypass) = build_sure_bypass(&argv) else {
        return Ok(());
    };

    let error = ProcessCommand::new(bypass.program)
        .args(&bypass.args)
        .exec();

    Err(format!("failed to exec {}: {}", bypass.program, error))
}

fn is_non_bypass_subcommand(arg: Option<&std::ffi::OsString>) -> bool {
    matches!(
        arg.and_then(|a| a.to_str()),
        Some("list" | "restore" | "purge" | "help" | "--help")
    )
}

fn has_sure_flag(args: &[std::ffi::OsString]) -> bool {
    for arg in args {
        if arg == "--" {
            return false;
        }
        if arg == "--sure" {
            return true;
        }
        if let Some(text) = arg.to_str() {
            if text.starts_with('-') && !text.starts_with("--") && text.contains('s') {
                return true;
            }
        }
    }
    false
}

fn build_sure_bypass(argv: &[std::ffi::OsString]) -> Option<SureBypass> {
    if argv.len() <= 1 || !has_sure_flag(&argv[1..]) {
        return None;
    }

    // --sure bypass only applies to delete and unlink operations.
    // Anything recognized as a subcommand is excluded. When adding a new
    // subcommand to cli::parse_args, add it here too so it won't bypass.
    if is_non_bypass_subcommand(argv.get(1)) {
        return None;
    }

    if cli::invoked_as_unlink(argv.first()) {
        return Some(SureBypass {
            program: "/bin/unlink",
            args: filter_passthrough_args(&argv[1..]),
        });
    }

    let invoked_subcommand_unlink = argv.get(1).and_then(|arg| arg.to_str()) == Some("unlink");
    let (program, args) = if invoked_subcommand_unlink {
        ("/bin/unlink", filter_passthrough_args(&argv[2..]))
    } else {
        ("/bin/rm", filter_passthrough_args(&argv[1..]))
    };

    Some(SureBypass { program, args })
}

fn filter_passthrough_args(args: &[std::ffi::OsString]) -> Vec<std::ffi::OsString> {
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
                    filtered.push(std::ffi::OsString::from(format!("-{remaining}")));
                }
                continue;
            }
        }

        filtered.push(arg.clone());
    }

    filtered
}

fn run_unlink(options: UnlinkOptions) -> Result<(), String> {
    let delete_options = DeleteOptions {
        mode: RequestedMode::Batch,
        paths: vec![options.path.clone()],
        ..DeleteOptions::default()
    };

    match delete_one(&options.path, &delete_options) {
        Ok(_) => Ok(()),
        Err(error) => Err(format!("{}: {}", options.path.display(), error)),
    }
}

fn run_delete(options: DeleteOptions) -> Result<(), String> {
    if options.paths.is_empty() {
        if options.force {
            return Ok(());
        }

        print!("{}", cli::usage());
        return Err("missing path operands".to_string());
    }

    let mode = resolve_mode(options.mode);

    let prompt_once = options.interactive_once
        || (mode == EffectiveMode::Interactive
            && should_auto_prompt_once(&options)
            && paths_warrant_prompt(&options.paths));

    if prompt_once {
        let prompt = format!(
            "{} {} entr{}?",
            if options.permanent {
                "permanently delete"
            } else {
                "move"
            },
            options.paths.len(),
            if options.paths.len() == 1 { "y" } else { "ies" }
        );
        if !confirm(&prompt)? {
            return Ok(());
        }
    }

    let mut had_error = false;

    for path in &options.paths {
        if options.interactive_each {
            let prompt = format!("remove {}?", path.display());
            if !confirm(&prompt)? {
                continue;
            }
        }

        match delete_one(path, &options) {
            Ok(Some(result)) => {
                if options.verbose {
                    print_delete_result(result);
                }
            }
            Ok(None) => {}
            Err(error) => {
                had_error = true;
                eprintln!("sure-rm: {}: {}", path.display(), error);
            }
        }
    }

    if had_error {
        Err("one or more paths could not be removed".to_string())
    } else {
        Ok(())
    }
}

fn delete_one(path: &Path, options: &DeleteOptions) -> Result<Option<DeleteResult>, String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if options.force && error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(None);
        }
        Err(error) => return Err(error.to_string()),
    };

    reject_dangerous_target(path, &metadata)?;

    if !metadata.file_type().is_symlink() && metadata.is_dir() {
        if options.recursive {
            if options.one_file_system {
                ensure_same_file_system_tree(path, metadata.dev())?;
            }
        } else if options.allow_dir {
            ensure_directory_is_empty(path)?;
        } else {
            return Err("is a directory (use -r, -R, or -d)".to_string());
        }
    }

    if options.permanent {
        permanently_delete(path, &metadata, options)?;
        return Ok(Some(DeleteResult::PermanentlyDeleted(path.to_path_buf())));
    }

    store::move_to_trash(path)
        .map(DeleteResult::Trashed)
        .map(Some)
        .fmt_err()
}

fn reject_dangerous_target(path: &Path, metadata: &fs::Metadata) -> Result<(), String> {
    if path == Path::new("/") {
        return Err("refusing to remove /".to_string());
    }

    if path == Path::new(".") || path == Path::new("..") {
        return Err("refusing to remove . or ..".to_string());
    }

    if metadata.file_type().is_symlink() {
        return Ok(());
    }

    let canonical_target = fs::canonicalize(path).fmt_err()?;
    let cwd = std::env::current_dir().fmt_err()?;
    let canonical_cwd = fs::canonicalize(&cwd).fmt_err()?;
    let home = store::home_dir().fmt_err()?;
    let canonical_home = fs::canonicalize(&home).unwrap_or(home);

    if canonical_target == PathBuf::from("/") {
        return Err("refusing to remove /".to_string());
    }

    if canonical_target == canonical_cwd || canonical_cwd.starts_with(&canonical_target) {
        return Err(
            "refusing to remove the current working directory or one of its parents".to_string(),
        );
    }

    if canonical_target == canonical_home || canonical_home.starts_with(&canonical_target) {
        return Err(
            "refusing to remove the home directory or one of its parents".to_string(),
        );
    }

    Ok(())
}

fn ensure_directory_is_empty(path: &Path) -> Result<(), String> {
    let mut entries = fs::read_dir(path).fmt_err()?;
    if entries.next().is_some() {
        Err("directory not empty".to_string())
    } else {
        Ok(())
    }
}

fn ensure_same_file_system_tree(path: &Path, expected_dev: u64) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path).fmt_err()?;
    if metadata.dev() != expected_dev {
        return Err(format!(
            "cross-device entry blocked by -x: {}",
            path.display()
        ));
    }

    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Ok(());
    }

    for entry in fs::read_dir(path).fmt_err()? {
        let entry = entry.fmt_err()?;
        let child_path = entry.path();
        ensure_same_file_system_tree(&child_path, expected_dev)?;
    }

    Ok(())
}

fn permanently_delete(
    path: &Path,
    metadata: &fs::Metadata,
    options: &DeleteOptions,
) -> Result<(), String> {
    if !metadata.file_type().is_symlink() && metadata.is_dir() {
        if options.recursive {
            fs::remove_dir_all(path).fmt_err()
        } else if options.allow_dir {
            fs::remove_dir(path).fmt_err()
        } else {
            Err("is a directory (use -r, -R, or -d)".to_string())
        }
    } else {
        fs::remove_file(path).fmt_err()
    }
}

fn run_list() -> Result<(), String> {
    let purged = store::purge_expired_records().fmt_err()?;
    for record in &purged {
        eprintln!("sure-rm: expired (TTL): {}", record.original_path.display());
    }

    let mut records = store::list_records().fmt_err()?;
    if records.is_empty() {
        println!("sure-rm trash is empty");
        return Ok(());
    }

    records.sort_by(|left, right| right.deleted_at_secs.cmp(&left.deleted_at_secs));

    for record in records {
        println!(
            "{}\t{}\t{}\t{}",
            record.id,
            record.kind.as_str(),
            record.deleted_at_secs,
            record.original_path.display()
        );
    }

    Ok(())
}

fn resolve_record(input: &str) -> Result<TrashRecord, String> {
    if let Ok(Some(record)) = store::read_record(input) {
        return Ok(record);
    }
    let path = PathBuf::from(input);
    store::find_latest_record_by_original_path(&path)
        .fmt_err()?
        .ok_or_else(|| format!("not found in trash: {input}"))
}

fn run_restore(options: RestoreOptions) -> Result<(), String> {
    let record = resolve_record(&options.id)?;
    let restored_path = store::restore(&record.id, options.destination.as_deref()).fmt_err()?;
    println!("restored {}", restored_path.display());
    Ok(())
}

fn run_purge(options: PurgeOptions) -> Result<(), String> {
    if options.all {
        let records = store::list_records().fmt_err()?;
        for record in records {
            purge_record(&record)?;
        }
        return Ok(());
    }

    if options.ids.is_empty() {
        return Err("purge requires at least one id or --all".to_string());
    }

    for id in &options.ids {
        purge_record(&resolve_record(id)?)?;
    }

    Ok(())
}

fn purge_record(record: &TrashRecord) -> Result<(), String> {
    store::purge_record_data(record).fmt_err()?;
    println!("purged {}", record.id);
    Ok(())
}

fn paths_warrant_prompt(paths: &[PathBuf]) -> bool {
    if paths.len() > 3 {
        return true;
    }

    for path in paths {
        match fs::symlink_metadata(path) {
            Ok(metadata) if !metadata.file_type().is_symlink() && metadata.is_dir() => return true,
            Ok(_) => {}
            Err(_) => {}
        }
    }

    false
}

fn should_auto_prompt_once(options: &DeleteOptions) -> bool {
    !options.force
        && !options.interactive_each
        && !options.interactive_once
        && (options.recursive || options.allow_dir || options.paths.len() > 3)
}

fn resolve_mode(requested: RequestedMode) -> EffectiveMode {
    match requested {
        RequestedMode::Auto => {
            if stdin().is_terminal() && stderr().is_terminal() {
                EffectiveMode::Interactive
            } else {
                EffectiveMode::Batch
            }
        }
        RequestedMode::Interactive => EffectiveMode::Interactive,
        RequestedMode::Batch => EffectiveMode::Batch,
    }
}

fn confirm(prompt: &str) -> Result<bool, String> {
    use std::io::{self, Write};

    eprint!("{prompt} [y/N] ");
    io::stderr().flush().fmt_err()?;

    let mut input = String::new();
    io::stdin()
        .read_line(&mut input)
        .fmt_err()?;

    let answer = input.trim();
    Ok(answer.eq_ignore_ascii_case("y") || answer.eq_ignore_ascii_case("yes"))
}

fn print_delete_result(result: DeleteResult) {
    match result {
        DeleteResult::PermanentlyDeleted(path) => {
            println!("deleted {}", path.display());
        }
        DeleteResult::Trashed(record) => {
            let suffix = match record.outcome {
                MoveOutcome::CentralTrash => "",
                MoveOutcome::SiblingFallback => " (same-directory fallback)",
            };
            println!(
                "trashed {} -> {}{}",
                record.original_path.display(),
                record.trashed_path.display(),
                suffix
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        EffectiveMode, SureBypass, build_sure_bypass, filter_passthrough_args, resolve_mode,
        should_auto_prompt_once,
    };
    use crate::cli::{DeleteOptions, RequestedMode};
    use std::ffi::OsString;

    #[test]
    fn sure_after_double_dash_does_not_trigger_bypass() {
        let bypass = build_sure_bypass(&[
            OsString::from("sure-rm"),
            OsString::from("--"),
            OsString::from("--sure"),
        ]);

        assert!(bypass.is_none());
    }

    #[test]
    fn sure_before_double_dash_still_triggers_bypass() {
        let bypass = build_sure_bypass(&[
            OsString::from("sure-rm"),
            OsString::from("--sure"),
            OsString::from("--"),
            OsString::from("file"),
        ]);

        assert!(bypass.is_some());
    }

    #[test]
    fn sure_bypass_skips_non_delete_subcommands() {
        for subcommand in &["list", "restore", "purge", "help", "--help"] {
            let bypass = build_sure_bypass(&[
                OsString::from("sure-rm"),
                OsString::from(*subcommand),
                OsString::from("--sure"),
            ]);

            assert!(
                bypass.is_none(),
                "bypass should not trigger for {subcommand}"
            );
        }
    }

    #[test]
    fn auto_prompt_once_for_recursive_delete() {
        let options = DeleteOptions {
            recursive: true,
            ..DeleteOptions::default()
        };

        assert!(should_auto_prompt_once(&options));
    }

    #[test]
    fn force_disables_auto_prompt_once() {
        let options = DeleteOptions {
            recursive: true,
            force: true,
            ..DeleteOptions::default()
        };

        assert!(!should_auto_prompt_once(&options));
    }

    #[test]
    fn explicit_mode_resolution_is_stable() {
        assert_eq!(
            resolve_mode(RequestedMode::Interactive),
            EffectiveMode::Interactive
        );
        assert_eq!(resolve_mode(RequestedMode::Batch), EffectiveMode::Batch);
    }

    #[test]
    fn force_without_paths_is_a_noop() {
        let options = DeleteOptions {
            force: true,
            ..DeleteOptions::default()
        };

        assert!(super::run_delete(options).is_ok());
    }

    #[test]
    fn sure_bypass_uses_rm_and_strips_sure_rm_flags() {
        let bypass = build_sure_bypass(&[
            OsString::from("sure-rm"),
            OsString::from("--mode"),
            OsString::from("interactive"),
            OsString::from("--sure"),
            OsString::from("-rf"),
            OsString::from("target"),
        ])
        .unwrap();

        assert_eq!(
            bypass,
            SureBypass {
                program: "/bin/rm",
                args: vec![OsString::from("-rf"), OsString::from("target")],
            }
        );
    }

    #[test]
    fn sure_bypass_uses_unlink_for_subcommand() {
        let bypass = build_sure_bypass(&[
            OsString::from("sure-rm"),
            OsString::from("unlink"),
            OsString::from("--sure"),
            OsString::from("--"),
            OsString::from("-file"),
        ])
        .unwrap();

        assert_eq!(
            bypass,
            SureBypass {
                program: "/bin/unlink",
                args: vec![OsString::from("--"), OsString::from("-file")],
            }
        );
    }

    #[test]
    fn short_s_triggers_sure_bypass() {
        let bypass = build_sure_bypass(&[
            OsString::from("sure-rm"),
            OsString::from("-sf"),
            OsString::from("target"),
        ])
        .unwrap();

        assert_eq!(
            bypass,
            SureBypass {
                program: "/bin/rm",
                args: vec![OsString::from("-f"), OsString::from("target")],
            }
        );
    }

    #[test]
    fn short_s_alone_triggers_sure_bypass() {
        let bypass = build_sure_bypass(&[
            OsString::from("sure-rm"),
            OsString::from("-s"),
            OsString::from("target"),
        ])
        .unwrap();

        assert_eq!(
            bypass,
            SureBypass {
                program: "/bin/rm",
                args: vec![OsString::from("target")],
            }
        );
    }

    #[test]
    fn filter_passthrough_args_keeps_operands_after_double_dash() {
        let filtered = filter_passthrough_args(&[
            OsString::from("--mode=interactive"),
            OsString::from("--"),
            OsString::from("--mode"),
            OsString::from("--sure"),
            OsString::from("file"),
        ]);

        assert_eq!(
            filtered,
            vec![
                OsString::from("--"),
                OsString::from("--mode"),
                OsString::from("--sure"),
                OsString::from("file"),
            ]
        );
    }
}
