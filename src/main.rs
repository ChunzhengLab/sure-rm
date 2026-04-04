mod cli;
mod color;
mod store;

use std::fs;
use std::io::{IsTerminal, stderr, stdin};
use std::os::unix::fs::MetadataExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command as ProcessCommand, ExitCode};

use cli::{
    Command, DeleteOptions, Invocation, PurgeOptions, RequestedMode, RestoreOptions, SureBypass,
    UnlinkOptions,
};
use store::{MoveOutcome, TrashRecord};

enum DeleteResult {
    PermanentlyDeleted(PathBuf),
    Trashed(TrashRecord),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EffectiveMode {
    Interactive,
    Batch,
}

fn main() -> ExitCode {
    match cli::parse_invocation() {
        Ok(Invocation::Bypass(bypass)) => exec_bypass(bypass),
        Ok(Invocation::Command(command)) => match run(command) {
            Ok(()) => ExitCode::SUCCESS,
            Err(message) => {
                color::print_error(message);
                ExitCode::from(1)
            }
        },
        Err(message) => {
            color::print_error(message);
            ExitCode::from(1)
        }
    }
}

fn exec_bypass(bypass: SureBypass) -> ExitCode {
    let error = ProcessCommand::new(bypass.program)
        .args(&bypass.args)
        .exec();
    color::print_error(format_args!("failed to exec {}: {error}", bypass.program));
    ExitCode::from(1)
}

fn run(command: Command) -> Result<(), String> {
    match command {
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
            && paths_need_prompt(&options.paths));

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
                color::print_error(format_args!("{}: {error}", path.display()));
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
        .map_err(|e| e.to_string())
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

    let canonical_target = fs::canonicalize(path).map_err(|e| e.to_string())?;
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let canonical_cwd = fs::canonicalize(&cwd).map_err(|e| e.to_string())?;
    let home = store::home_dir().map_err(|e| e.to_string())?;
    let canonical_home = fs::canonicalize(&home).unwrap_or(home);

    if canonical_target == Path::new("/") {
        return Err("refusing to remove /".to_string());
    }

    if canonical_target == canonical_cwd || canonical_cwd.starts_with(&canonical_target) {
        return Err(
            "refusing to remove the current working directory or one of its parents".to_string(),
        );
    }

    if canonical_target == canonical_home || canonical_home.starts_with(&canonical_target) {
        return Err("refusing to remove the home directory or one of its parents".to_string());
    }

    Ok(())
}

fn ensure_directory_is_empty(path: &Path) -> Result<(), String> {
    let mut entries = fs::read_dir(path).map_err(|e| e.to_string())?;
    if entries.next().is_some() {
        Err("directory not empty".to_string())
    } else {
        Ok(())
    }
}

fn ensure_same_file_system_tree(path: &Path, expected_dev: u64) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if metadata.dev() != expected_dev {
        return Err(format!(
            "cross-device entry blocked by -x: {}",
            path.display()
        ));
    }

    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Ok(());
    }

    for entry in fs::read_dir(path).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
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
            fs::remove_dir_all(path).map_err(|e| e.to_string())
        } else if options.allow_dir {
            fs::remove_dir(path).map_err(|e| e.to_string())
        } else {
            Err("is a directory (use -r, -R, or -d)".to_string())
        }
    } else {
        fs::remove_file(path).map_err(|e| e.to_string())
    }
}

fn run_list() -> Result<(), String> {
    let repaired = store::repair_metadata().map_err(|e| e.to_string())?;
    if repaired > 0 {
        color::print_warning(format_args!(
            "repaired {repaired} corrupted metadata entries"
        ));
    }

    let expired = store::purge_expired_records().map_err(|e| e.to_string())?;
    for warning in &expired.warnings {
        color::print_warning(warning);
    }
    for record in &expired.purged {
        color::print_warning(format_args!(
            "expired (TTL): {}",
            record.original_path.display()
        ));
    }

    let mut records = store::load_records().map_err(|e| e.to_string())?;
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

fn resolve_record(query: &str) -> Result<TrashRecord, String> {
    match store::read_record(query) {
        Ok(Some(record)) => return Ok(record),
        Err(e) if e.kind() == std::io::ErrorKind::InvalidInput => {}
        Err(e) => return Err(e.to_string()),
        Ok(None) => {}
    }
    store::find_latest_record_by_original_path(Path::new(query))
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("not found in trash: {query}"))
}

fn run_restore(options: RestoreOptions) -> Result<(), String> {
    let record = resolve_record(&options.query)?;
    let outcome =
        store::restore(&record.id, options.destination.as_deref()).map_err(|e| e.to_string())?;
    if let Some(error) = &outcome.warning {
        color::print_warning(format_args!(
            "restored but failed to remove metadata: {error}"
        ));
    }
    println!("restored {}", outcome.path.display());
    Ok(())
}

fn run_purge(options: PurgeOptions) -> Result<(), String> {
    if options.all {
        let records = store::load_records().map_err(|e| e.to_string())?;
        for record in records {
            purge_record(&record)?;
        }
        return Ok(());
    }

    if options.queries.is_empty() {
        return Err("purge requires at least one id/path or --all".to_string());
    }

    for query in &options.queries {
        purge_record(&resolve_record(query)?)?;
    }

    Ok(())
}

fn purge_record(record: &TrashRecord) -> Result<(), String> {
    let outcome = store::purge_record_data(record).map_err(|e| e.to_string())?;
    if let Some(error) = &outcome.warning {
        color::print_warning(format_args!(
            "purged {} but failed to remove metadata: {error}",
            record.id
        ));
    }
    println!("purged {}", record.id);
    Ok(())
}

fn paths_need_prompt(paths: &[PathBuf]) -> bool {
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

    let p = color::stderr_painter();
    eprint!("{} ", p.emphasis(format_args!("{prompt} [y/N]")));
    io::stderr().flush().map_err(|e| e.to_string())?;

    let mut input = String::new();
    io::stdin()
        .read_line(&mut input)
        .map_err(|e| e.to_string())?;

    let answer = input.trim();
    Ok(answer.eq_ignore_ascii_case("y") || answer.eq_ignore_ascii_case("yes"))
}

fn print_delete_result(result: DeleteResult) {
    let p = color::stdout_painter();
    match result {
        DeleteResult::PermanentlyDeleted(path) => {
            println!("{} {}", p.bad("deleted"), path.display());
        }
        DeleteResult::Trashed(record) => {
            let suffix = match record.outcome {
                MoveOutcome::CentralTrash => "",
                MoveOutcome::SiblingFallback => " (same-directory fallback)",
            };
            println!(
                "{} {} -> {}{}",
                p.good("trashed"),
                record.original_path.display(),
                record.trashed_path.display(),
                suffix
            );
        }
    }
}

/// Crate-wide lock for tests that mutate process-global environment variables.
/// Every `unsafe { env::set_var / remove_var }` call in any test module must
/// hold this lock to prevent data races across the test harness's thread pool.
#[cfg(test)]
pub(crate) static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests {
    use super::{EffectiveMode, resolve_mode, should_auto_prompt_once};
    use crate::cli::{DeleteOptions, RequestedMode};

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
}
