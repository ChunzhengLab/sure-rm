use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EntryKind {
    File,
    Directory,
    Symlink,
    Other,
}

impl EntryKind {
    pub fn as_str(self) -> &'static str {
        match self {
            EntryKind::File => "file",
            EntryKind::Directory => "dir",
            EntryKind::Symlink => "symlink",
            EntryKind::Other => "other",
        }
    }

    fn from_str(value: &str) -> Option<Self> {
        match value {
            "file" => Some(EntryKind::File),
            "dir" => Some(EntryKind::Directory),
            "symlink" => Some(EntryKind::Symlink),
            "other" => Some(EntryKind::Other),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MoveOutcome {
    CentralTrash,
    SiblingFallback,
}

#[derive(Clone, Debug)]
pub struct TrashRecord {
    pub id: String,
    pub original_path: PathBuf,
    pub trashed_path: PathBuf,
    pub deleted_at_secs: u64,
    pub kind: EntryKind,
    pub outcome: MoveOutcome,
    pub meta_path: Option<PathBuf>,
}

pub fn home_dir() -> io::Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "HOME is not set"))
}

pub fn move_to_trash(path: &Path) -> io::Result<TrashRecord> {
    let metadata = fs::symlink_metadata(path)?;
    let deleted_at_secs = now_secs()?;
    let id = new_id(deleted_at_secs);
    let kind = entry_kind(&metadata);
    let original_path = absolute_path(path)?;

    let central_dest = trash_dir()?.join(&id);
    let sibling_dest = sibling_fallback_destination(path, &id)?;

    let (trashed_path, outcome) = match fs::rename(path, &central_dest) {
        Ok(()) => (central_dest, MoveOutcome::CentralTrash),
        Err(error) if error.kind() == io::ErrorKind::CrossesDevices => {
            fs::rename(path, &sibling_dest)?;
            (sibling_dest, MoveOutcome::SiblingFallback)
        }
        Err(error) => return Err(error),
    };

    let record = TrashRecord {
        id: id.clone(),
        original_path,
        trashed_path,
        deleted_at_secs,
        kind,
        outcome,
        meta_path: None,
    };

    if let Err(error) = write_record(&record) {
        if let Err(rollback_error) = fs::rename(&record.trashed_path, path) {
            return Err(io::Error::new(
                error.kind(),
                format!(
                    "failed to write metadata: {error}; rollback failed: {rollback_error}; file stranded at {}",
                    record.trashed_path.display()
                ),
            ));
        }
        return Err(error);
    }

    Ok(record)
}

pub fn list_records() -> io::Result<Vec<TrashRecord>> {
    let dir = metadata_dir()?;
    let mut records = Vec::new();

    if !dir.exists() {
        return Ok(records);
    }

    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();

        if path.extension().and_then(OsStr::to_str) != Some("meta") {
            continue;
        }

        match read_record_from_path(&path) {
            Ok(record) => records.push(record),
            Err(_) => {}
        }
    }

    Ok(records)
}

fn validate_id(id: &str) -> io::Result<()> {
    if id.is_empty() || id.contains('/') || id.contains("..") {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid trash id"));
    }
    Ok(())
}

pub fn read_record(id: &str) -> io::Result<Option<TrashRecord>> {
    validate_id(id)?;
    let path = metadata_dir()?.join(format!("{id}.meta"));
    if !path.exists() {
        return Ok(None);
    }
    read_record_from_path(&path).map(Some)
}

pub fn restore(id: &str, destination: Option<&Path>) -> io::Result<PathBuf> {
    let record = read_record(id)?
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "unknown trash id"))?;

    let target = destination
        .map(Path::to_path_buf)
        .unwrap_or_else(|| record.original_path.clone());

    if fs::symlink_metadata(&target).is_ok() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "restore target already exists",
        ));
    }

    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }

    fs::rename(&record.trashed_path, &target)?;
    if let Err(error) = delete_record_file(&record) {
        eprintln!(
            "sure-rm: warning: restored successfully but failed to remove metadata: {error}"
        );
    }
    Ok(target)
}

pub fn find_latest_record_by_original_path(path: &Path) -> io::Result<Option<TrashRecord>> {
    let target = absolute_path(path)?;
    let mut records = list_records()?;
    records.sort_by(|left, right| {
        right
            .deleted_at_secs
            .cmp(&left.deleted_at_secs)
            .then_with(|| right.id.cmp(&left.id))
    });

    for record in records {
        if record.original_path == target && fs::symlink_metadata(&record.trashed_path).is_ok() {
            return Ok(Some(record));
        }
    }

    Ok(None)
}

pub fn delete_record(id: &str) -> io::Result<()> {
    validate_id(id)?;
    let path = metadata_dir()?.join(format!("{id}.meta"));
    remove_meta_file(&path)
}

pub fn delete_record_file(record: &TrashRecord) -> io::Result<()> {
    match &record.meta_path {
        Some(path) => remove_meta_file(path),
        None => delete_record(&record.id),
    }
}

fn remove_meta_file(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn trash_root() -> io::Result<PathBuf> {
    if let Some(path) = std::env::var_os("SURE_RM_ROOT") {
        return Ok(PathBuf::from(path));
    }

    Ok(home_dir()?.join(".sure-rm"))
}

fn trash_dir() -> io::Result<PathBuf> {
    let path = trash_root()?.join("trash");
    fs::create_dir_all(&path)?;
    Ok(path)
}

fn metadata_dir() -> io::Result<PathBuf> {
    let path = trash_root()?.join("meta");
    fs::create_dir_all(&path)?;
    Ok(path)
}

fn sibling_fallback_destination(path: &Path, id: &str) -> io::Result<PathBuf> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no parent"))?;
    Ok(parent.join(format!(".sure-rm.{id}")))
}

fn write_record(record: &TrashRecord) -> io::Result<()> {
    let dir = metadata_dir()?;
    let final_path = dir.join(format!("{}.meta", record.id));
    let temp_path = dir.join(format!("{}.meta.tmp", record.id));

    let content = format!(
        "id={}\ndeleted_at_secs={}\nkind={}\noutcome={}\noriginal_path_hex={}\ntrashed_path_hex={}\n",
        record.id,
        record.deleted_at_secs,
        record.kind.as_str(),
        match record.outcome {
            MoveOutcome::CentralTrash => "central",
            MoveOutcome::SiblingFallback => "sibling",
        },
        hex_encode_path(&record.original_path),
        hex_encode_path(&record.trashed_path)
    );

    fs::write(&temp_path, content)?;
    fs::rename(&temp_path, &final_path)?;
    Ok(())
}

fn read_record_from_path(path: &Path) -> io::Result<TrashRecord> {
    let content = fs::read_to_string(path)?;

    let mut id: Option<String> = None;
    let mut deleted_at_secs: Option<u64> = None;
    let mut kind: Option<EntryKind> = None;
    let mut outcome: Option<MoveOutcome> = None;
    let mut original_path: Option<PathBuf> = None;
    let mut trashed_path: Option<PathBuf> = None;

    for line in content.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };

        match key {
            "id" => id = Some(value.to_string()),
            "deleted_at_secs" => deleted_at_secs = value.parse::<u64>().ok(),
            "kind" => kind = EntryKind::from_str(value),
            "outcome" => {
                outcome = match value {
                    "central" => Some(MoveOutcome::CentralTrash),
                    "sibling" => Some(MoveOutcome::SiblingFallback),
                    _ => None,
                };
            }
            "original_path_hex" => original_path = Some(hex_decode_path(value)?),
            "trashed_path_hex" => trashed_path = Some(hex_decode_path(value)?),
            _ => {}
        }
    }

    Ok(TrashRecord {
        id: id.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing id"))?,
        original_path: original_path
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing original path"))?,
        trashed_path: trashed_path
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing trashed path"))?,
        deleted_at_secs: deleted_at_secs
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing timestamp"))?,
        kind: kind.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing kind"))?,
        outcome: outcome
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing outcome"))?,
        meta_path: Some(path.to_path_buf()),
    })
}

fn absolute_path(path: &Path) -> io::Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };

    Ok(normalize_absolute_path(&absolute))
}

fn entry_kind(metadata: &fs::Metadata) -> EntryKind {
    let file_type = metadata.file_type();
    if file_type.is_symlink() {
        EntryKind::Symlink
    } else if metadata.is_dir() {
        EntryKind::Directory
    } else if metadata.is_file() {
        EntryKind::File
    } else {
        EntryKind::Other
    }
}

fn now_secs() -> io::Result<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|error| io::Error::other(error.to_string()))
}

fn new_id(timestamp_secs: u64) -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let pid = std::process::id();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.subsec_nanos())
        .unwrap_or(0);
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{timestamp_secs}-{pid}-{nanos}-{seq}")
}

fn hex_encode_path(path: &Path) -> String {
    hex_encode_bytes(path.as_os_str().as_bytes())
}

fn hex_decode_path(value: &str) -> io::Result<PathBuf> {
    let bytes = hex_decode_bytes(value)?;
    Ok(PathBuf::from(OsString::from_vec(bytes)))
}

fn hex_encode_bytes(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);

    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }

    out
}

fn hex_decode_bytes(value: &str) -> io::Result<Vec<u8>> {
    if !value.len().is_multiple_of(2) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "hex string must have even length",
        ));
    }

    let mut out = Vec::with_capacity(value.len() / 2);
    let bytes = value.as_bytes();

    for chunk in bytes.chunks_exact(2) {
        let high = hex_value(chunk[0])?;
        let low = hex_value(chunk[1])?;
        out.push((high << 4) | low);
    }

    Ok(out)
}

fn hex_value(byte: u8) -> io::Result<u8> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid hex digit in metadata",
        )),
    }
}

fn normalize_absolute_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();

    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Normal(part) => normalized.push(part),
        }
    }

    if normalized.as_os_str().is_empty() {
        PathBuf::from("/")
    } else {
        normalized
    }
}

#[cfg(test)]
mod tests {
    use super::{hex_decode_bytes, hex_encode_bytes, normalize_absolute_path};
    use std::path::Path;

    #[test]
    fn hex_round_trip() {
        let bytes = b"hello-\x00-world";
        let encoded = hex_encode_bytes(bytes);
        let decoded = hex_decode_bytes(&encoded).unwrap();
        assert_eq!(decoded, bytes);
    }

    #[test]
    fn normalize_absolute_path_collapses_dot_segments() {
        let path = normalize_absolute_path(Path::new("/tmp/./sure-rm/../sure-rm/file"));
        assert_eq!(path, Path::new("/tmp/sure-rm/file"));
    }

    #[test]
    fn validate_id_rejects_path_traversal() {
        assert!(super::validate_id("").is_err());
        assert!(super::validate_id("../bad").is_err());
        assert!(super::validate_id("foo/bar").is_err());
        assert!(super::validate_id("12345-100-0-0").is_ok());
    }

    #[test]
    fn read_record_rejects_invalid_id() {
        assert!(super::read_record("../etc/passwd").is_err());
    }

    #[test]
    fn delete_record_rejects_invalid_id() {
        assert!(super::delete_record("../nonexistent").is_err());
        assert!(super::delete_record("foo/bar").is_err());
    }

    // This test mutates the SURE_RM_ROOT env var. If more tests need to do
    // the same, they must be serialized (e.g. via serial_test crate) or use
    // a design that avoids global state.
    #[test]
    fn purge_cleans_up_corrupted_metadata() {
        use std::fs;

        let dir = std::env::temp_dir().join(format!(
            "sure-rm-test-purge-corrupted-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);

        // SAFETY: single test mutates this var; no other test depends on it.
        unsafe { std::env::set_var("SURE_RM_ROOT", &dir) };

        // Create a dummy file in trash
        let trash = dir.join("trash");
        fs::create_dir_all(&trash).unwrap();
        let trash_file = trash.join("bad-id");
        fs::write(&trash_file, "data").unwrap();

        // Write a .meta with a corrupted id containing path traversal
        let meta = dir.join("meta");
        fs::create_dir_all(&meta).unwrap();
        let meta_file = meta.join("bad-id.meta");
        let original_hex = super::hex_encode_bytes(b"/tmp/orig");
        let trashed_hex = super::hex_encode_path(&trash_file);
        fs::write(
            &meta_file,
            format!(
                "id=../bad\ndeleted_at_secs=1000\nkind=file\noutcome=central\noriginal_path_hex={original_hex}\ntrashed_path_hex={trashed_hex}\n"
            ),
        )
        .unwrap();

        // list_records should return the record despite the bad id
        let records = super::list_records().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].id, "../bad");

        // delete_record_file uses meta_path from the record, not the corrupted id
        super::delete_record_file(&records[0]).unwrap();
        assert!(!meta_file.exists(), ".meta file should be cleaned up");

        let _ = fs::remove_dir_all(&dir);
        unsafe { std::env::remove_var("SURE_RM_ROOT") };
    }
}
