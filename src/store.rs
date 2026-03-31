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
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid trash id",
        ));
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
        eprintln!("sure-rm: warning: restored successfully but failed to remove metadata: {error}");
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

pub fn ttl_secs() -> Option<u64> {
    let value = std::env::var_os("SURE_RM_TTL")?;
    let text = match value.to_str() {
        Some(t) => t,
        None => {
            eprintln!("sure-rm: warning: SURE_RM_TTL is not valid UTF-8, ignoring");
            return None;
        }
    };
    match parse_ttl(text) {
        TtlResult::Enabled(secs) => Some(secs),
        TtlResult::Disabled => None,
        TtlResult::Invalid => {
            eprintln!("sure-rm: warning: invalid SURE_RM_TTL value: {text}, ignoring");
            None
        }
    }
}

enum TtlResult {
    Enabled(u64),
    Disabled,
    Invalid,
}

impl TtlResult {
    fn as_secs(&self) -> Option<u64> {
        match self {
            TtlResult::Enabled(s) => Some(*s),
            _ => None,
        }
    }

    fn is_invalid(&self) -> bool {
        matches!(self, TtlResult::Invalid)
    }
}

fn parse_ttl(text: &str) -> TtlResult {
    let text = text.trim();
    if text.is_empty() {
        return TtlResult::Disabled;
    }

    let (digits, multiplier) = if let Some(d) = text.strip_suffix('d') {
        (d.trim(), 86400)
    } else if let Some(d) = text.strip_suffix('h') {
        (d.trim(), 3600)
    } else if let Some(d) = text.strip_suffix('s') {
        (d.trim(), 1)
    } else {
        (text, 86400)
    };

    let Ok(n) = digits.parse::<u64>() else {
        return TtlResult::Invalid;
    };

    if n == 0 {
        return TtlResult::Disabled;
    }

    match n.checked_mul(multiplier) {
        Some(secs) => TtlResult::Enabled(secs),
        None => TtlResult::Invalid,
    }
}

pub fn purge_record_data(record: &TrashRecord) -> io::Result<()> {
    match fs::symlink_metadata(&record.trashed_path) {
        Ok(metadata) if !metadata.file_type().is_symlink() && metadata.is_dir() => {
            fs::remove_dir_all(&record.trashed_path)?;
        }
        Ok(_) => {
            fs::remove_file(&record.trashed_path)?;
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }

    if let Err(error) = delete_record_file(record) {
        eprintln!(
            "sure-rm: warning: purged data but failed to remove metadata for {}: {error}",
            record.id
        );
    }

    Ok(())
}

pub fn purge_expired_records() -> io::Result<Vec<TrashRecord>> {
    let Some(ttl) = ttl_secs() else {
        return Ok(Vec::new());
    };

    let now = now_secs()?;
    let records = list_records()?;
    let mut purged = Vec::new();

    for record in records {
        let Some(age) = now.checked_sub(record.deleted_at_secs) else {
            continue;
        };
        if age > ttl {
            if let Err(error) = purge_record_data(&record) {
                eprintln!(
                    "sure-rm: warning: failed to purge expired entry {}: {error}",
                    record.id
                );
            } else {
                purged.push(record);
            }
        }
    }

    Ok(purged)
}

#[cfg(test)]
mod tests {
    use super::{hex_decode_bytes, hex_encode_bytes, normalize_absolute_path};
    use std::path::Path;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

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

    fn ttl(text: &str) -> Option<u64> {
        super::parse_ttl(text).as_secs()
    }

    fn ttl_invalid(text: &str) -> bool {
        super::parse_ttl(text).is_invalid()
    }

    #[test]
    fn parse_ttl_days_suffix() {
        assert_eq!(ttl("7d"), Some(7 * 86400));
        assert_eq!(ttl("30d"), Some(30 * 86400));
    }

    #[test]
    fn parse_ttl_hours_suffix() {
        assert_eq!(ttl("24h"), Some(24 * 3600));
    }

    #[test]
    fn parse_ttl_seconds_suffix() {
        assert_eq!(ttl("3600s"), Some(3600));
    }

    #[test]
    fn parse_ttl_bare_number_is_days() {
        assert_eq!(ttl("7"), Some(7 * 86400));
    }

    #[test]
    fn parse_ttl_zero_disables() {
        assert_eq!(ttl("0"), None);
        assert!(!ttl_invalid("0"));
        assert_eq!(ttl("0d"), None);
        assert!(!ttl_invalid("0d"));
        assert_eq!(ttl("0h"), None);
        assert!(!ttl_invalid("0h"));
    }

    #[test]
    fn parse_ttl_empty_and_disabled() {
        assert_eq!(ttl(""), None);
        assert!(!ttl_invalid(""));
        assert_eq!(ttl("  "), None);
        assert!(!ttl_invalid("  "));
    }

    #[test]
    fn parse_ttl_invalid_values() {
        assert!(ttl_invalid("abc"));
        assert!(ttl_invalid("-1"));
    }

    #[test]
    fn parse_ttl_overflow_is_invalid() {
        assert!(ttl_invalid("99999999999999999999d"));
        assert!(ttl_invalid("99999999999999999999h"));
        assert!(ttl_invalid("99999999999999999999"));
    }

    #[test]
    fn parse_ttl_max_valid_value_does_not_overflow() {
        let max_days = u64::MAX / 86400;
        assert!(ttl(&format!("{max_days}d")).is_some());
        assert!(ttl_invalid(&format!("{}d", max_days + 1)));
    }

    struct TestEnv {
        dir: std::path::PathBuf,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl TestEnv {
        fn new(name: &str) -> Self {
            let lock = ENV_LOCK.lock().unwrap();
            let dir = std::env::temp_dir()
                .join(format!("sure-rm-test-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            unsafe { std::env::set_var("SURE_RM_ROOT", &dir) };
            unsafe { std::env::remove_var("SURE_RM_TTL") };
            TestEnv { dir, _lock: lock }
        }

        fn add_record(&self, id: &str, age_secs: u64) -> (std::path::PathBuf, std::path::PathBuf) {
            let trash = self.dir.join("trash");
            std::fs::create_dir_all(&trash).unwrap();
            let trash_file = trash.join(id);
            std::fs::write(&trash_file, "data").unwrap();

            let meta = self.dir.join("meta");
            std::fs::create_dir_all(&meta).unwrap();
            let meta_file = meta.join(format!("{id}.meta"));
            let timestamp = super::now_secs().unwrap().saturating_sub(age_secs);
            let original_hex = super::hex_encode_bytes(format!("/tmp/{id}").as_bytes());
            let trashed_hex = super::hex_encode_path(&trash_file);
            std::fs::write(
                &meta_file,
                format!("id={id}\ndeleted_at_secs={timestamp}\nkind=file\noutcome=central\noriginal_path_hex={original_hex}\ntrashed_path_hex={trashed_hex}\n"),
            )
            .unwrap();

            (trash_file, meta_file)
        }
    }

    impl Drop for TestEnv {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
            unsafe { std::env::remove_var("SURE_RM_ROOT") };
            unsafe { std::env::remove_var("SURE_RM_TTL") };
        }
    }

    #[test]
    fn purge_expired_records_removes_old_entries() {
        let env = TestEnv::new("ttl-expired");
        unsafe { std::env::set_var("SURE_RM_TTL", "1d") };
        let (trash_file, meta_file) = env.add_record("old-file", 86401);

        let purged = super::purge_expired_records().unwrap();
        assert_eq!(purged.len(), 1);
        assert!(!trash_file.exists());
        assert!(!meta_file.exists());
    }

    #[test]
    fn purge_expired_keeps_fresh_entries() {
        let env = TestEnv::new("ttl-fresh");
        unsafe { std::env::set_var("SURE_RM_TTL", "7d") };
        let (trash_file, meta_file) = env.add_record("fresh-file", 100);

        let purged = super::purge_expired_records().unwrap();
        assert!(purged.is_empty());
        assert!(trash_file.exists());
        assert!(meta_file.exists());
    }

    #[test]
    fn purge_expired_does_nothing_without_ttl() {
        let _env = TestEnv::new("no-ttl");
        let purged = super::purge_expired_records().unwrap();
        assert!(purged.is_empty());
    }

    #[test]
    fn purge_cleans_up_corrupted_metadata() {
        let env = TestEnv::new("corrupted");

        // Manually create a record with a corrupted id
        let trash = env.dir.join("trash");
        std::fs::create_dir_all(&trash).unwrap();
        let trash_file = trash.join("bad-id");
        std::fs::write(&trash_file, "data").unwrap();

        let meta = env.dir.join("meta");
        std::fs::create_dir_all(&meta).unwrap();
        let meta_file = meta.join("bad-id.meta");
        let original_hex = super::hex_encode_bytes(b"/tmp/orig");
        let trashed_hex = super::hex_encode_path(&trash_file);
        std::fs::write(
            &meta_file,
            format!("id=../bad\ndeleted_at_secs=1000\nkind=file\noutcome=central\noriginal_path_hex={original_hex}\ntrashed_path_hex={trashed_hex}\n"),
        )
        .unwrap();

        let records = super::list_records().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].id, "../bad");

        super::delete_record_file(&records[0]).unwrap();
        assert!(!meta_file.exists());
    }
}
