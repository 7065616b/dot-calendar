use std::collections::HashSet;
use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, WAIT_ABANDONED, WAIT_OBJECT_0};
use windows_sys::Win32::Storage::FileSystem::{
    MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
};
use windows_sys::Win32::System::Threading::{
    CreateEventW, CreateMutexW, ReleaseMutex, SetEvent, WaitForSingleObject, INFINITE,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{FindWindowW, PostMessageW, WM_APP};

use crate::model::{Date, Event};

const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;
const MAX_ARCHIVE_BYTES: u64 = 64 * 1024;
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
const INITIALIZED_MARKER_NAME: &str = "calendar.initialized";
const INITIALIZED_MARKER_BYTES: &[u8] = b"DotCalendar initialized v1\n";
pub const STORE_CHANGED: u32 = WM_APP + 1;
static NEXT_ID: AtomicU64 = AtomicU64::new(0);

#[derive(serde::Serialize, serde::Deserialize)]
struct DeletedEvent {
    version: u32,
    sequence: u64,
    event: Event,
}

#[derive(PartialEq, serde::Serialize, serde::Deserialize)]
struct Database {
    version: u32,
    events: Vec<Event>,
    #[serde(default)]
    retired_request_ids: Vec<String>,
}

impl Default for Database {
    fn default() -> Self {
        Self {
            version: 1,
            events: Vec::new(),
            retired_request_ids: Vec::new(),
        }
    }
}

pub struct Store {
    path: PathBuf,
    mutex: HANDLE,
    changed: HANDLE,
    event_name: String,
    observed_primary: AtomicBool,
}

struct LockedMutex(HANDLE);

impl Drop for LockedMutex {
    fn drop(&mut self) {
        unsafe { ReleaseMutex(self.0) };
    }
}

impl Store {
    pub fn open() -> Result<Self, String> {
        let directory = crate::data_paths::directory()?;
        Self::open_at(directory.join("calendar.json"))
    }

    fn open_at(path: PathBuf) -> Result<Self, String> {
        let parent = path.parent().ok_or("calendar path has no directory")?;
        fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create calendar directory: {error}"))?;
        let canonical_parent = fs::canonicalize(parent)
            .map_err(|error| format!("cannot resolve calendar directory: {error}"))?;
        let path = canonical_parent.join(path.file_name().ok_or("calendar path has no filename")?);
        let key = path.to_string_lossy().to_lowercase();
        let hash = key.bytes().fold(0xcbf29ce484222325_u64, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
        });
        let mutex_name = format!("Local\\DotCalendar.Mutex.{hash:016x}");
        let event_name = format!("Local\\DotCalendar.Changed.{hash:016x}");
        let mutex =
            unsafe { CreateMutexW(std::ptr::null(), 0, wide(OsStr::new(&mutex_name)).as_ptr()) };
        if mutex.is_null() {
            return Err(format!(
                "cannot create calendar mutex: {}",
                std::io::Error::last_os_error()
            ));
        }
        // Auto reset wakes the desktop widget once for each committed write.
        let changed = unsafe {
            CreateEventW(
                std::ptr::null(),
                0,
                0,
                wide(OsStr::new(&event_name)).as_ptr(),
            )
        };
        if changed.is_null() {
            unsafe { CloseHandle(mutex) };
            return Err(format!(
                "cannot create calendar change event: {}",
                std::io::Error::last_os_error()
            ));
        }
        let store = Self {
            path,
            mutex,
            changed,
            event_name,
            observed_primary: AtomicBool::new(false),
        };
        // Detect corruption at startup; never overwrite a malformed existing file.
        store.with_lock(|store| store.read_database().map(|_| ()))?;
        Ok(store)
    }

    /// Name used by a Win32 listener to open the same auto-reset event.
    pub fn change_event_name(&self) -> &str {
        &self.event_name
    }

    /// Valid while this Store is alive. The owner closes it on drop.
    pub fn change_event_handle(&self) -> HANDLE {
        self.changed
    }

    pub fn list_events(&self, date: Option<&str>) -> Result<Vec<Event>, String> {
        date.map(Date::parse).transpose()?;
        self.with_lock(|store| {
            let mut events = store.read_database()?.events;
            if let Some(date) = date {
                events.retain(|event| event.date == date);
            }
            events.sort_by(|a, b| {
                (&a.date, &a.time, &a.title, &a.id).cmp(&(&b.date, &b.time, &b.title, &b.id))
            });
            Ok(events)
        })
    }

    /// Read and validate the entire current file without sorting unrelated rows.
    pub fn event_by_id(&self, id: &str) -> Result<Option<Event>, String> {
        self.with_lock(|store| {
            Ok(store
                .read_database()?
                .events
                .into_iter()
                .find(|event| event.id == id))
        })
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "Public atomic create accepts flat event fields used by CLI, MCP, and widget"
    )]
    pub fn create_event_with_details(
        &self,
        date: &str,
        time: Option<&str>,
        title: &str,
        notes: &str,
        request_id: Option<&str>,
        completed: bool,
        color: Option<&str>,
        recurrence: Option<&str>,
        reminder_minutes: Option<u32>,
    ) -> Result<Event, String> {
        Date::parse(date)?;
        let time = valid_time(time)?;
        let title = valid_title(title)?;
        valid_notes(notes)?;
        let request_id = valid_request_id(request_id)?;
        let color = valid_color(color)?.map(str::to_ascii_uppercase);
        let recurrence = valid_recurrence(recurrence)?;
        valid_reminder(reminder_minutes)?;
        self.with_lock(|store| {
            let mut db = store.read_database()?;
            if let Some(request_id) = request_id {
                if db.retired_request_ids.iter().any(|used| used == request_id) {
                    return Err("request-id belongs to a deleted event".into());
                }
                if let Some(existing) = db
                    .events
                    .iter()
                    .find(|event| event.request_id.as_deref() == Some(request_id))
                {
                    if existing.date == date
                        && existing.time.as_deref() == time
                        && existing.title == title
                        && existing.notes == notes
                        && existing.completed == completed
                        && existing.color == color
                        && existing.recurrence.as_deref() == recurrence
                        && existing.reminder_minutes == reminder_minutes
                    {
                        return Ok(existing.clone());
                    }
                    return Err("request-id already belongs to a different event payload".into());
                }
            }
            let event = Event {
                id: unique_id(&db),
                date: date.to_owned(),
                time: time.map(str::to_owned),
                title: title.to_owned(),
                notes: notes.to_owned(),
                request_id: request_id.map(str::to_owned),
                completed,
                color,
                recurrence: recurrence.map(str::to_owned),
                reminder_minutes,
            };
            db.events.push(event.clone());
            store.write_database(&db)?;
            Ok(event)
        })
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "Public atomic update accepts flat event fields used by CLI, MCP, and widget"
    )]
    pub fn update_event_with_details(
        &self,
        id: &str,
        date: &str,
        time: Option<&str>,
        title: &str,
        notes: &str,
        completed: Option<bool>,
        color: Option<Option<&str>>,
        recurrence: Option<Option<&str>>,
        reminder_minutes: Option<Option<u32>>,
    ) -> Result<Event, String> {
        self.update_event_with_details_expected(
            id,
            date,
            time,
            title,
            notes,
            completed,
            color,
            recurrence,
            reminder_minutes,
            None,
        )
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "Atomic update accepts flat event fields plus an optional concurrency snapshot"
    )]
    pub fn update_event_with_details_expected(
        &self,
        id: &str,
        date: &str,
        time: Option<&str>,
        title: &str,
        notes: &str,
        completed: Option<bool>,
        color: Option<Option<&str>>,
        recurrence: Option<Option<&str>>,
        reminder_minutes: Option<Option<u32>>,
        expected: Option<&Event>,
    ) -> Result<Event, String> {
        Date::parse(date)?;
        let time = valid_time(time)?;
        let title = valid_title(title)?;
        valid_notes(notes)?;
        let color = color
            .map(valid_color)
            .transpose()?
            .map(|value| value.map(str::to_ascii_uppercase));
        let recurrence = recurrence.map(valid_recurrence).transpose()?;
        if let Some(reminder_minutes) = reminder_minutes {
            valid_reminder(reminder_minutes)?;
        }
        self.with_lock(|store| {
            let mut db = store.read_database()?;
            let event =
                db.events
                    .iter_mut()
                    .find(|event| event.id == id)
                    .ok_or(if expected.is_some() {
                        "event changed since it was read"
                    } else {
                        "event not found"
                    })?;
            if expected.is_some_and(|snapshot| snapshot != &*event) {
                return Err("event changed since it was read".into());
            }
            let changed = event.date != date
                || event.time.as_deref() != time
                || event.title != title
                || event.notes != notes
                || completed.is_some_and(|value| event.completed != value)
                || color
                    .as_ref()
                    .is_some_and(|value| event.color.as_ref() != value.as_ref())
                || recurrence.is_some_and(|value| event.recurrence.as_deref() != value)
                || reminder_minutes.is_some_and(|value| event.reminder_minutes != value);
            if !changed {
                return Ok(event.clone());
            }
            event.date = date.to_owned();
            event.time = time.map(str::to_owned);
            event.title = title.to_owned();
            event.notes = notes.to_owned();
            if let Some(completed) = completed {
                event.completed = completed;
            }
            if let Some(color) = color {
                event.color = color;
            }
            if let Some(recurrence) = recurrence {
                event.recurrence = recurrence.map(str::to_owned);
            }
            if let Some(reminder_minutes) = reminder_minutes {
                event.reminder_minutes = reminder_minutes;
            }
            let updated = event.clone();
            store.write_database(&db)?;
            Ok(updated)
        })
    }

    pub fn delete_event(&self, id: &str) -> Result<Event, String> {
        self.delete_event_expected(id, None)
    }

    pub fn delete_event_expected(
        &self,
        id: &str,
        expected: Option<&Event>,
    ) -> Result<Event, String> {
        self.delete_event_selected(|events| {
            let event =
                events
                    .iter()
                    .find(|event| event.id == id)
                    .ok_or(if expected.is_some() {
                        "event changed since it was read"
                    } else {
                        "event not found"
                    })?;
            if expected.is_some_and(|snapshot| snapshot != event) {
                return Err("event changed since it was read".into());
            }
            Ok(event.id.clone())
        })
    }

    /// Select and delete from the same locked snapshot so conversational matching cannot race an edit.
    pub fn delete_event_selected(
        &self,
        selector: impl FnOnce(&[Event]) -> Result<String, String>,
    ) -> Result<Event, String> {
        self.with_lock(|store| {
            let mut db = store.read_database()?;
            let id = selector(&db.events)?;
            let position = db
                .events
                .iter()
                .position(|event| event.id == id)
                .ok_or("selected event no longer exists")?;
            store.archive_event(&db.events[position])?;
            let removed = db.events.remove(position);
            if let Some(request_id) = &removed.request_id {
                db.retired_request_ids.push(request_id.clone());
            }
            store.write_database(&db)?;
            Ok(removed)
        })
    }

    /// Patch only supplied settings. Outer `None` means preserve, inner `None` means clear.
    pub fn patch_details(
        &self,
        id: &str,
        completed: Option<bool>,
        color: Option<Option<&str>>,
        recurrence: Option<Option<&str>>,
        reminder_minutes: Option<Option<u32>>,
    ) -> Result<Event, String> {
        self.patch_details_expected(id, completed, color, recurrence, reminder_minutes, None)
    }

    pub fn patch_details_expected(
        &self,
        id: &str,
        completed: Option<bool>,
        color: Option<Option<&str>>,
        recurrence: Option<Option<&str>>,
        reminder_minutes: Option<Option<u32>>,
        expected: Option<&Event>,
    ) -> Result<Event, String> {
        let color = color
            .map(valid_color)
            .transpose()?
            .map(|value| value.map(str::to_ascii_uppercase));
        let recurrence = recurrence.map(valid_recurrence).transpose()?;
        if let Some(reminder_minutes) = reminder_minutes {
            valid_reminder(reminder_minutes)?;
        }
        self.with_lock(|store| {
            let mut db = store.read_database()?;
            let event =
                db.events
                    .iter_mut()
                    .find(|event| event.id == id)
                    .ok_or(if expected.is_some() {
                        "event changed since it was read"
                    } else {
                        "event not found"
                    })?;
            if expected.is_some_and(|snapshot| snapshot != &*event) {
                return Err("event changed since it was read".into());
            }
            let changed = completed.is_some_and(|value| event.completed != value)
                || color
                    .as_ref()
                    .is_some_and(|value| event.color.as_ref() != value.as_ref())
                || recurrence.is_some_and(|value| event.recurrence.as_deref() != value)
                || reminder_minutes.is_some_and(|value| event.reminder_minutes != value);
            if !changed {
                return Ok(event.clone());
            }
            if let Some(completed) = completed {
                event.completed = completed;
            }
            if let Some(color) = color {
                event.color = color;
            }
            if let Some(recurrence) = recurrence {
                event.recurrence = recurrence.map(str::to_owned);
            }
            if let Some(reminder_minutes) = reminder_minutes {
                event.reminder_minutes = reminder_minutes;
            }
            let updated = event.clone();
            store.write_database(&db)?;
            Ok(updated)
        })
    }

    /// Atomic compare-and-swap for sync clients; `false` means a concurrent edit won.
    pub fn replace_if_unchanged(
        &self,
        expected: &Event,
        replacement: &Event,
    ) -> Result<bool, String> {
        if expected.id != replacement.id || expected.request_id != replacement.request_id {
            return Err("replacement must preserve event id and request-id".into());
        }
        validate_event(replacement)?;
        self.with_lock(|store| {
            let mut db = store.read_database()?;
            let Some(event) = db.events.iter_mut().find(|event| event.id == expected.id) else {
                return Ok(false);
            };
            if &*event != expected {
                return Ok(false);
            }
            if replacement != expected {
                *event = replacement.clone();
                store.write_database(&db)?;
            }
            Ok(true)
        })
    }

    /// Delete only if the saved event still matches the sync client's snapshot.
    pub fn delete_if_unchanged(&self, expected: &Event) -> Result<bool, String> {
        self.with_lock(|store| {
            let mut db = store.read_database()?;
            let Some(position) = db.events.iter().position(|event| event.id == expected.id) else {
                return Ok(false);
            };
            if db.events[position] != *expected {
                return Ok(false);
            }
            store.archive_event(&db.events[position])?;
            let removed = db.events.remove(position);
            if let Some(request_id) = removed.request_id {
                db.retired_request_ids.push(request_id);
            }
            store.write_database(&db)?;
            Ok(true)
        })
    }

    /// Deleted events are read only on demand; the desktop widget loads active rows alone.
    pub fn list_deleted_events(&self) -> Result<Vec<Event>, String> {
        self.with_lock(|store| {
            let active = store.read_database()?;
            let mut archives = store.read_deleted_events()?;
            archives.retain(|record| {
                !active
                    .events
                    .iter()
                    .any(|event| event.id == record.event.id)
            });
            archives.sort_unstable_by_key(|record| std::cmp::Reverse(record.sequence));
            Ok(archives.into_iter().map(|record| record.event).collect())
        })
    }

    pub fn restore_deleted_event(&self, id: &str) -> Result<Event, String> {
        self.with_lock(|store| {
            let directory = store
                .checked_deleted_dir(false)?
                .ok_or("deleted event not found")?;
            let name = archive_name(id);
            let path = directory.join(&name);
            match fs::symlink_metadata(&path) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    return Err("deleted event not found".into());
                }
                Err(error) => return Err(format!("cannot inspect deleted event: {error}")),
                Ok(_) => {}
            }
            let record = Self::read_deleted_event_file(&path, &name)?;
            if record.event.id != id {
                return Err(
                    "deleted-event archive filename collision; original was preserved".into(),
                );
            }
            store.restore_archived_event(&record)
        })
    }

    pub fn restore_last_deleted(&self) -> Result<Event, String> {
        self.with_lock(|store| {
            let db = store.read_database()?;
            let record = store
                .read_deleted_events()?
                .into_iter()
                .filter(|record| !db.events.iter().any(|event| event.id == record.event.id))
                .max_by_key(|record| record.sequence)
                .ok_or("deleted event not found")?;
            store.restore_archived_event_with_database(&record, db)
        })
    }

    /// Export active events and deleted request IDs; the separate deleted-event archive is not included.
    pub fn export_json(&self) -> Result<String, String> {
        self.with_lock(|store| {
            serde_json::to_string_pretty(&store.read_database()?)
                .map_err(|error| format!("cannot encode calendar backup: {error}"))
        })
    }

    /// Replace all saved data from a validated backup. Call only for an explicit restore action.
    pub fn restore_json(&self, json: &str) -> Result<(), String> {
        let db = Self::decode_database(json.as_bytes())?;
        self.with_lock(|store| {
            // Refuse to overwrite a malformed live file, so the user can recover it separately.
            if store.read_database()? == db {
                return Ok(());
            }
            store.write_database(&db)
        })
    }

    fn deleted_dir(&self) -> PathBuf {
        self.path
            .parent()
            .expect("calendar has a parent")
            .join("deleted")
    }

    fn checked_deleted_dir(&self, create: bool) -> Result<Option<PathBuf>, String> {
        let path = self.deleted_dir();
        if create {
            fs::create_dir_all(&path)
                .map_err(|error| format!("cannot create deleted-event archive: {error}"))?;
        }
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if !create && error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(None);
            }
            Err(error) => return Err(format!("cannot inspect deleted-event archive: {error}")),
        };
        if !metadata.is_dir() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err("deleted-event archive is not a regular directory".into());
        }
        Ok(Some(path))
    }

    fn read_deleted_events(&self) -> Result<Vec<DeletedEvent>, String> {
        let Some(directory) = self.checked_deleted_dir(false)? else {
            return Ok(Vec::new());
        };
        let mut records = Vec::new();
        for entry in fs::read_dir(directory)
            .map_err(|error| format!("cannot list deleted-event archive: {error}"))?
        {
            let entry =
                entry.map_err(|error| format!("cannot list deleted-event archive: {error}"))?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            if !is_archive_name(name) {
                continue;
            }
            records.push(Self::read_deleted_event_file(&entry.path(), name)?);
        }
        Ok(records)
    }

    fn read_deleted_event_file(path: &Path, name: &str) -> Result<DeletedEvent, String> {
        let metadata = fs::symlink_metadata(path)
            .map_err(|error| format!("cannot inspect deleted event: {error}"))?;
        if !metadata.is_file() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err("deleted event is not a regular file".into());
        }
        if metadata.len() > MAX_ARCHIVE_BYTES {
            return Err("deleted event exceeds 64 KiB".into());
        }
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        File::open(path)
            .map_err(|error| format!("cannot open deleted event: {error}"))?
            .take(MAX_ARCHIVE_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| format!("cannot read deleted event: {error}"))?;
        if bytes.len() as u64 > MAX_ARCHIVE_BYTES {
            return Err("deleted event exceeds 64 KiB".into());
        }
        let record: DeletedEvent = serde_json::from_slice(&bytes).map_err(|error| {
            format!("deleted event is malformed and was left unchanged: {error}")
        })?;
        if record.version != 1 || record.sequence == 0 || archive_name(&record.event.id) != name {
            return Err(
                "deleted event has invalid identity or version; archive was left unchanged".into(),
            );
        }
        validate_event(&record.event)
            .map_err(|error| format!("deleted event is invalid and was left unchanged: {error}"))?;
        Ok(record)
    }

    fn reserve_deleted_sequence(&self, directory: &Path) -> Result<u64, String> {
        let sequence_path = directory.join("sequence.txt");
        let previous = match fs::symlink_metadata(&sequence_path) {
            Ok(metadata) => {
                if !metadata.is_file()
                    || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
                    || metadata.len() > 20
                {
                    return Err(
                        "deleted-event sequence file is invalid; event was not deleted".into(),
                    );
                }
                let mut bytes = Vec::with_capacity(metadata.len() as usize);
                File::open(&sequence_path)
                    .map_err(|error| format!("cannot open deleted-event sequence: {error}"))?
                    .take(21)
                    .read_to_end(&mut bytes)
                    .map_err(|error| format!("cannot read deleted-event sequence: {error}"))?;
                if bytes.is_empty() || bytes.len() > 20 || !bytes.iter().all(u8::is_ascii_digit) {
                    return Err(
                        "deleted-event sequence file is invalid; event was not deleted".into(),
                    );
                }
                std::str::from_utf8(&bytes)
                    .ok()
                    .and_then(|value| value.parse::<u64>().ok())
                    .ok_or("deleted-event sequence file is invalid; event was not deleted")?
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                // One-time recovery for an archive created before the counter existed.
                self.read_deleted_events()?
                    .into_iter()
                    .map(|record| record.sequence)
                    .max()
                    .unwrap_or(0)
            }
            Err(error) => return Err(format!("cannot inspect deleted-event sequence: {error}")),
        };
        let next = previous
            .checked_add(1)
            .ok_or("deleted-event sequence exhausted")?;
        let temp = directory.join(format!(
            ".sequence-{}-{}.tmp",
            std::process::id(),
            next_number()
        ));
        let result = (|| {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temp)
                .map_err(|error| {
                    format!("cannot create deleted-event sequence temp file: {error}")
                })?;
            write!(file, "{next}")
                .map_err(|error| format!("cannot write deleted-event sequence: {error}"))?;
            file.sync_all()
                .map_err(|error| format!("cannot flush deleted-event sequence: {error}"))?;
            drop(file);
            let moved = unsafe {
                MoveFileExW(
                    wide(temp.as_os_str()).as_ptr(),
                    wide(sequence_path.as_os_str()).as_ptr(),
                    MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
                )
            };
            if moved == 0 {
                return Err(format!(
                    "cannot save deleted-event sequence; event was not deleted: {}",
                    std::io::Error::last_os_error()
                ));
            }
            Ok(next)
        })();
        if result.is_err() {
            let _ = fs::remove_file(temp);
        }
        result
    }

    fn archive_event(&self, event: &Event) -> Result<(), String> {
        let directory = self
            .checked_deleted_dir(true)?
            .expect("archive directory was created");
        let name = archive_name(&event.id);
        let destination = directory.join(&name);
        match fs::symlink_metadata(&destination) {
            Ok(_) => {
                if Self::read_deleted_event_file(&destination, &name)?.event.id != event.id {
                    return Err(
                        "deleted-event archive filename collision; event was not deleted".into(),
                    );
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("cannot inspect prior deleted event: {error}")),
        }
        let sequence = self.reserve_deleted_sequence(&directory)?;
        let record = DeletedEvent {
            version: 1,
            sequence,
            event: event.clone(),
        };
        let bytes = serde_json::to_vec(&record)
            .map_err(|error| format!("cannot encode deleted event: {error}"))?;
        if bytes.len() as u64 > MAX_ARCHIVE_BYTES {
            return Err("deleted event exceeds 64 KiB; event was not deleted".into());
        }
        let temp = directory.join(format!(
            ".deleted-{}-{}.tmp",
            std::process::id(),
            next_number()
        ));
        let result = (|| {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temp)
                .map_err(|error| format!("cannot create deleted-event temp file: {error}"))?;
            file.write_all(&bytes)
                .map_err(|error| format!("cannot write deleted event: {error}"))?;
            file.sync_all()
                .map_err(|error| format!("cannot flush deleted event: {error}"))?;
            drop(file);
            let moved = unsafe {
                MoveFileExW(
                    wide(temp.as_os_str()).as_ptr(),
                    wide(destination.as_os_str()).as_ptr(),
                    MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
                )
            };
            if moved == 0 {
                return Err(format!(
                    "cannot save deleted event; event was not deleted: {}",
                    std::io::Error::last_os_error()
                ));
            }
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(temp);
        }
        result
    }

    fn restore_archived_event(&self, record: &DeletedEvent) -> Result<Event, String> {
        self.restore_archived_event_with_database(record, self.read_database()?)
    }

    fn restore_archived_event_with_database(
        &self,
        record: &DeletedEvent,
        mut db: Database,
    ) -> Result<Event, String> {
        let event = &record.event;
        if db.events.iter().any(|active| active.id == event.id) {
            return Err("event ID is already active; deleted copy was preserved".into());
        }
        if event.request_id.as_ref().is_some_and(|request_id| {
            db.events
                .iter()
                .any(|active| active.request_id.as_ref() == Some(request_id))
        }) {
            return Err("request-id is already active; deleted copy was preserved".into());
        }
        if let Some(request_id) = &event.request_id {
            if let Some(position) = db
                .retired_request_ids
                .iter()
                .position(|id| id == request_id)
            {
                db.retired_request_ids.remove(position);
            }
        }
        db.events.push(event.clone());
        self.write_database(&db)?;
        // The live row is durable now. If archive cleanup fails, filtering active IDs hides
        // the stale copy; a later deletion of the same ID safely replaces it.
        let _ = fs::remove_file(self.deleted_dir().join(archive_name(&event.id)));
        Ok(event.clone())
    }

    fn with_lock<T>(&self, action: impl FnOnce(&Self) -> Result<T, String>) -> Result<T, String> {
        let result = unsafe { WaitForSingleObject(self.mutex, INFINITE) };
        if result != WAIT_OBJECT_0 && result != WAIT_ABANDONED {
            return Err(format!(
                "cannot lock calendar: {}",
                std::io::Error::last_os_error()
            ));
        }
        let _guard = LockedMutex(self.mutex);
        action(self)
    }

    fn read_database(&self) -> Result<Database, String> {
        let file = match File::open(&self.path) {
            Ok(file) => {
                // Even a corrupt/unreadable body is evidence that this is not a virgin store.
                self.observed_primary.store(true, Ordering::Relaxed);
                file
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                // A dangling reparse link is an existing primary path, not a virgin store.
                // Probe the path itself before interpreting the missing target as no data.
                match fs::symlink_metadata(&self.path) {
                    Ok(_) => return Err(
                        "calendar path exists but cannot be opened; existing data was not replaced"
                            .into(),
                    ),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => {
                        return Err(format!("cannot inspect missing calendar path: {error}"))
                    }
                }
                if self.observed_primary.load(Ordering::Relaxed)
                    || self.initialized_marker_exists()?
                    || self.checked_deleted_dir(false)?.is_some()
                {
                    return Err("calendar file is missing from an initialized store; existing data was not replaced".into());
                }
                return Ok(Database::default());
            }
            Err(error) => return Err(format!("cannot read calendar: {error}")),
        };
        let length = file
            .metadata()
            .map_err(|error| format!("cannot inspect calendar: {error}"))?
            .len();
        if length > MAX_FILE_BYTES {
            return Err("calendar file exceeds 4 MiB".into());
        }
        let mut bytes = Vec::with_capacity(length as usize);
        file.take(MAX_FILE_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| format!("cannot read calendar: {error}"))?;
        Self::decode_database(&bytes)
    }

    fn initialized_marker_path(&self) -> PathBuf {
        self.path
            .parent()
            .expect("calendar has a parent")
            .join(INITIALIZED_MARKER_NAME)
    }

    fn initialized_marker_exists(&self) -> Result<bool, String> {
        let path = self.initialized_marker_path();
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => {
                return Err(format!(
                    "cannot inspect calendar initialization marker: {error}"
                ))
            }
        };
        if !metadata.is_file()
            || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
            || metadata.len() != INITIALIZED_MARKER_BYTES.len() as u64
        {
            return Err(
                "calendar initialization marker is invalid; existing data was not replaced".into(),
            );
        }
        let mut bytes = Vec::with_capacity(INITIALIZED_MARKER_BYTES.len());
        File::open(path)
            .map_err(|error| format!("cannot open calendar initialization marker: {error}"))?
            .take(INITIALIZED_MARKER_BYTES.len() as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| format!("cannot read calendar initialization marker: {error}"))?;
        if bytes != INITIALIZED_MARKER_BYTES {
            return Err(
                "calendar initialization marker is invalid; existing data was not replaced".into(),
            );
        }
        Ok(true)
    }

    /// Called under the same store mutex as the primary write. Never rewrites an existing marker.
    fn ensure_initialized_marker(&self) -> Result<bool, String> {
        if self.initialized_marker_exists()? {
            return Ok(false);
        }
        let destination = self.initialized_marker_path();
        let temp = destination.parent().unwrap().join(format!(
            ".initialized-{}-{}.tmp",
            std::process::id(),
            next_number()
        ));
        let mut created_temp = false;
        let result = (|| {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temp)
                .map_err(|error| {
                    format!("cannot create calendar initialization marker: {error}")
                })?;
            created_temp = true;
            file.write_all(INITIALIZED_MARKER_BYTES)
                .map_err(|error| format!("cannot write calendar initialization marker: {error}"))?;
            file.sync_all()
                .map_err(|error| format!("cannot flush calendar initialization marker: {error}"))?;
            drop(file);
            let moved = unsafe {
                MoveFileExW(
                    wide(temp.as_os_str()).as_ptr(),
                    wide(destination.as_os_str()).as_ptr(),
                    MOVEFILE_WRITE_THROUGH,
                )
            };
            if moved == 0 {
                let error = std::io::Error::last_os_error();
                if self.initialized_marker_exists()? {
                    return Ok(false);
                }
                return Err(format!(
                    "cannot save calendar initialization marker: {error}"
                ));
            }
            Ok(true)
        })();
        if created_temp {
            let _ = fs::remove_file(temp);
        }
        result
    }

    fn decode_database(bytes: &[u8]) -> Result<Database, String> {
        if bytes.len() as u64 > MAX_FILE_BYTES {
            return Err("calendar file exceeds 4 MiB".into());
        }
        let db: Database = serde_json::from_slice(bytes).map_err(|error| {
            format!("calendar file is malformed and was left unchanged: {error}")
        })?;
        if db.version != 1 {
            return Err(format!(
                "unsupported calendar file version {}; file was left unchanged",
                db.version
            ));
        }
        let mut ids = HashSet::with_capacity(db.events.len());
        let request_count = db
            .events
            .iter()
            .filter(|event| event.request_id.is_some())
            .count()
            + db.retired_request_ids.len();
        let mut requests = HashSet::with_capacity(request_count);
        for event in &db.events {
            Date::parse(&event.date)
                .map_err(|_| "calendar contains invalid event date; file was left unchanged")?;
            valid_time(event.time.as_deref())
                .map_err(|_| "calendar contains invalid event time; file was left unchanged")?;
            valid_title(&event.title)
                .map_err(|_| "calendar contains invalid event title; file was left unchanged")?;
            valid_notes(&event.notes)
                .map_err(|_| "calendar contains invalid event notes; file was left unchanged")?;
            valid_request_id(event.request_id.as_deref())
                .map_err(|_| "calendar contains invalid request-id; file was left unchanged")?;
            valid_color(event.color.as_deref())
                .map_err(|_| "calendar contains invalid event color; file was left unchanged")?;
            valid_recurrence(event.recurrence.as_deref())
                .map_err(|_| "calendar contains invalid recurrence; file was left unchanged")?;
            valid_reminder(event.reminder_minutes)
                .map_err(|_| "calendar contains invalid reminder; file was left unchanged")?;
            if event.id.is_empty() || !ids.insert(&event.id) {
                return Err(
                    "calendar contains duplicate or empty event IDs; file was left unchanged"
                        .into(),
                );
            }
            if let Some(request_id) = &event.request_id {
                if !requests.insert(request_id) {
                    return Err(
                        "calendar contains duplicate request IDs; file was left unchanged".into(),
                    );
                }
            }
        }
        for request_id in &db.retired_request_ids {
            valid_request_id(Some(request_id)).map_err(|_| {
                "calendar contains invalid retired request-id; file was left unchanged"
            })?;
            if !requests.insert(request_id) {
                return Err(
                    "calendar contains duplicate request IDs; file was left unchanged".into(),
                );
            }
        }
        Ok(db)
    }

    fn write_database(&self, db: &Database) -> Result<(), String> {
        let bytes = serde_json::to_vec_pretty(db)
            .map_err(|error| format!("cannot encode calendar: {error}"))?;
        if bytes.len() as u64 > MAX_FILE_BYTES {
            return Err("calendar would exceed 4 MiB; existing data was not changed".into());
        }
        let parent = self.path.parent().ok_or("calendar path has no directory")?;
        let created_marker = self.ensure_initialized_marker()?;
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let temp = parent.join(format!(
            ".calendar-{}-{stamp:x}-{}.tmp",
            std::process::id(),
            next_number()
        ));
        let mut result = (|| {
            #[cfg(test)]
            if tests::FAIL_PRIMARY_WRITE.with(|fail| fail.replace(false)) {
                return Err("synthetic primary write failure".into());
            }
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temp)
                .map_err(|error| format!("cannot create temporary calendar file: {error}"))?;
            file.write_all(&bytes)
                .map_err(|error| format!("cannot write calendar: {error}"))?;
            file.sync_all()
                .map_err(|error| format!("cannot flush calendar: {error}"))?;
            drop(file);
            let moved = unsafe {
                MoveFileExW(
                    wide(temp.as_os_str()).as_ptr(),
                    wide(self.path.as_os_str()).as_ptr(),
                    MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
                )
            };
            if moved == 0 {
                return Err(format!(
                    "cannot replace calendar: {}",
                    std::io::Error::last_os_error()
                ));
            }
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temp);
            if created_marker
                && !self.observed_primary.load(Ordering::Relaxed)
                && matches!(fs::symlink_metadata(&self.path), Err(error) if error.kind() == std::io::ErrorKind::NotFound)
            {
                if let Err(error) = fs::remove_file(self.initialized_marker_path()) {
                    let original = result.unwrap_err();
                    result = Err(format!("{original}; cannot remove this failed first-save initialization marker: {error}"));
                }
            }
        } else {
            self.observed_primary.store(true, Ordering::Relaxed);
            // A failed notification does not turn a successful durable write into a retryable error.
            unsafe {
                SetEvent(self.changed);
                // The UI may be in a modal dialog or dragging a window, when its normal
                // wait loop is not running. A queued message reaches that same UI thread.
                let controller = FindWindowW(
                    wide(OsStr::new("DotCalendarController")).as_ptr(),
                    wide(OsStr::new(&self.event_name)).as_ptr(),
                );
                if !controller.is_null() {
                    PostMessageW(controller, STORE_CHANGED, 0, 0);
                }
            }
        }
        result
    }
}

impl Drop for Store {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.changed);
            CloseHandle(self.mutex);
        }
    }
}

fn valid_time(value: Option<&str>) -> Result<Option<&str>, String> {
    let Some(value) = value else { return Ok(None) };
    let bytes = value.as_bytes();
    if bytes.len() != 5
        || bytes[2] != b':'
        || ![0, 1, 3, 4].into_iter().all(|i| bytes[i].is_ascii_digit())
    {
        return Err("time must use HH:MM".into());
    }
    let hour: u32 = value[0..2].parse().map_err(|_| "invalid hour")?;
    let minute: u32 = value[3..5].parse().map_err(|_| "invalid minute")?;
    if hour > 23 || minute > 59 {
        return Err("time is outside 00:00–23:59".into());
    }
    Ok(Some(value))
}

fn valid_title(value: &str) -> Result<&str, String> {
    let value = value.trim();
    if value.is_empty() || (value.len() > 200 && value.chars().count() > 200) {
        return Err("title must contain 1–200 characters".into());
    }
    Ok(value)
}

fn valid_notes(value: &str) -> Result<(), String> {
    if value.len() > 4096 && value.chars().count() > 4096 {
        return Err("notes cannot exceed 4096 characters".into());
    }
    Ok(())
}

fn valid_request_id(value: Option<&str>) -> Result<Option<&str>, String> {
    let Some(value) = value else { return Ok(None) };
    if value.is_empty() || (value.len() > 128 && value.chars().count() > 128) {
        return Err("request-id must contain 1–128 characters".into());
    }
    Ok(Some(value))
}

fn valid_color(value: Option<&str>) -> Result<Option<&str>, String> {
    let Some(value) = value else { return Ok(None) };
    let bytes = value.as_bytes();
    if bytes.len() != 7 || bytes[0] != b'#' || !bytes[1..].iter().all(u8::is_ascii_hexdigit) {
        return Err("color must use #RRGGBB".into());
    }
    Ok(Some(value))
}

fn valid_recurrence(value: Option<&str>) -> Result<Option<&str>, String> {
    match value {
        None => Ok(None),
        Some("daily" | "weekly" | "monthly" | "yearly") => Ok(value),
        Some(_) => Err("recurrence must be daily, weekly, monthly, or yearly".into()),
    }
}

fn valid_reminder(value: Option<u32>) -> Result<(), String> {
    if value.is_some_and(|minutes| minutes > 525_600) {
        return Err("reminder cannot exceed one year (525600 minutes)".into());
    }
    Ok(())
}

fn validate_event(event: &Event) -> Result<(), String> {
    if event.id.is_empty() {
        return Err("event id cannot be empty".into());
    }
    Date::parse(&event.date)?;
    valid_time(event.time.as_deref())?;
    valid_title(&event.title)?;
    valid_notes(&event.notes)?;
    valid_request_id(event.request_id.as_deref())?;
    valid_color(event.color.as_deref())?;
    valid_recurrence(event.recurrence.as_deref())?;
    valid_reminder(event.reminder_minutes)
}

fn next_number() -> u64 {
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

fn archive_name(id: &str) -> String {
    let hash = id.bytes().fold(
        144066263297769815596495629667062367629_u128,
        |hash, byte| (hash ^ u128::from(byte)).wrapping_mul(309485009821345068724781371_u128),
    );
    format!("t-{hash:032x}.json")
}

fn is_archive_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    bytes.len() == 39
        && bytes.starts_with(b"t-")
        && bytes.ends_with(b".json")
        && bytes[2..34].iter().all(u8::is_ascii_hexdigit)
}

fn unique_id(db: &Database) -> String {
    loop {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let id = format!("e-{now:x}-{:x}-{:x}", std::process::id(), next_number());
        if !db.events.iter().any(|event| event.id == id) {
            return id;
        }
    }
}

fn wide(value: &OsStr) -> Vec<u16> {
    value.encode_wide().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::env;
    use std::sync::Mutex;

    static TEST_LOCK: Mutex<()> = Mutex::new(());
    thread_local! {
        pub(super) static FAIL_PRIMARY_WRITE: Cell<bool> = const { Cell::new(false) };
    }

    #[test]
    fn string_limits_preserve_unicode_scalar_and_trim_rules() {
        for unit in ["a", "한", "😀", "e\u{301}", "\0"] {
            for count in [0, 1, 127, 128, 129, 199, 200, 201, 4095, 4096, 4097] {
                let value = unit.repeat(count);
                let scalars = value.chars().count();
                assert_eq!(valid_notes(&value).is_ok(), scalars <= 4096);
                assert_eq!(
                    valid_request_id(Some(&value)).is_ok(),
                    (1..=128).contains(&scalars)
                );
                let padded = format!("  {value}\t\n");
                assert_eq!(valid_title(&padded).is_ok(), (1..=200).contains(&scalars));
                if let Ok(title) = valid_title(&padded) {
                    assert_eq!(title, value);
                }
            }
        }
        assert_eq!(valid_request_id(None).unwrap(), None);
    }

    #[test]
    fn event_lookup_reloads_same_metadata_and_validates_unrelated_rows() {
        let store = test_store();
        let first = create_basic_event(
            &store,
            "2026-10-07",
            Some("09:30"),
            "Zulu",
            "old",
            Some("lookup-1"),
        )
        .unwrap();
        let second =
            create_basic_event(&store, "2026-10-06", None, "Alpha", "", Some("lookup-2")).unwrap();
        assert_eq!(store.event_by_id(&first.id).unwrap(), Some(first.clone()));
        assert_eq!(store.event_by_id(&second.id).unwrap(), Some(second));
        assert_eq!(store.event_by_id("absent").unwrap(), None);
        let before = fs::metadata(&store.path).unwrap();
        let mut db = store.read_database().unwrap();
        db.events[0].notes = "new".into();
        let bytes = serde_json::to_vec_pretty(&db).unwrap();
        assert_eq!(bytes.len() as u64, before.len());
        fs::write(&store.path, &bytes).unwrap();
        File::options()
            .write(true)
            .open(&store.path)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(before.modified().unwrap()))
            .unwrap();
        assert_eq!(
            fs::metadata(&store.path).unwrap().modified().unwrap(),
            before.modified().unwrap()
        );
        assert_eq!(store.event_by_id(&first.id).unwrap().unwrap().notes, "new");
        db.events[1].date = "invalid".into();
        let invalid = serde_json::to_vec_pretty(&db).unwrap();
        fs::write(&store.path, &invalid).unwrap();
        assert!(store.event_by_id(&first.id).is_err());
        assert_eq!(fs::read(&store.path).unwrap(), invalid);
        fs::remove_file(&store.path).unwrap();
        assert!(store
            .event_by_id(&first.id)
            .unwrap_err()
            .contains("missing"));
        assert!(!store.path.exists());
        let _ = fs::remove_dir_all(store.path.parent().unwrap());
    }

    fn test_store() -> Store {
        let directory = env::temp_dir().join(format!(
            "dot-calendar-test-{}-{}",
            std::process::id(),
            next_number()
        ));
        Store::open_at(directory.join("calendar.json")).unwrap()
    }

    fn create_basic_event(
        store: &Store,
        date: &str,
        time: Option<&str>,
        title: &str,
        notes: &str,
        request_id: Option<&str>,
    ) -> Result<Event, String> {
        store.create_event_with_details(
            date, time, title, notes, request_id, false, None, None, None,
        )
    }

    #[test]
    fn crud_reopen_and_idempotency() {
        let _guard = TEST_LOCK.lock().unwrap();
        let store = test_store();
        let first = create_basic_event(
            &store,
            "2026-10-07",
            Some("09:30"),
            "Meeting",
            "Room A",
            Some("req-1"),
        )
        .unwrap();
        assert_eq!(
            first,
            create_basic_event(
                &store,
                "2026-10-07",
                Some("09:30"),
                "Meeting",
                "Room A",
                Some("req-1")
            )
            .unwrap()
        );
        assert!(create_basic_event(
            &store,
            "2026-10-07",
            Some("10:00"),
            "Meeting",
            "Room A",
            Some("req-1")
        )
        .is_err());
        let reopened = Store::open_at(store.path.clone()).unwrap();
        assert_eq!(
            reopened.list_events(Some("2026-10-07")).unwrap(),
            vec![first.clone()]
        );
        let updated = reopened
            .update_event_with_details(
                &first.id,
                "2026-10-08",
                None,
                "Moved",
                "",
                None,
                None,
                None,
                None,
            )
            .unwrap();
        assert_eq!(updated.request_id, first.request_id);
        assert!(store.list_events(Some("2026-10-07")).unwrap().is_empty());
        assert_eq!(store.delete_event(&first.id).unwrap(), updated);
        assert!(reopened.list_events(None).unwrap().is_empty());
        assert!(create_basic_event(
            &store,
            "2026-10-07",
            Some("09:30"),
            "Meeting",
            "Room A",
            Some("req-1")
        )
        .is_err());
        let _ = fs::remove_dir_all(store.path.parent().unwrap());
    }

    #[test]
    fn repeated_drop_and_reopen_preserves_89_events_and_completed_changes() {
        let _guard = TEST_LOCK.lock().unwrap();
        let store = test_store();
        let path = store.path.clone();
        let directory = path.parent().unwrap().to_owned();
        let mut expected = Vec::new();
        for index in 0..89 {
            expected.push(
                store
                    .create_event_with_details(
                        "2026-10-11",
                        Some("18:00"),
                        &format!("Synthetic event {index:03}"),
                        &format!("Synthetic note {index:03}\nSecond line"),
                        Some(&format!("reopen-{index:03}")),
                        index % 3 == 0,
                        Some("#123456"),
                        (index % 2 == 0).then_some("weekly"),
                        Some(15),
                    )
                    .unwrap(),
            );
        }
        let mut saved = fs::read(&path).unwrap();
        drop(store);
        assert_eq!(fs::read(&path).unwrap(), saved);

        for cycle in 0..4 {
            let reopened = Store::open_at(path.clone()).unwrap();
            assert_eq!(reopened.list_events(None).unwrap(), expected);
            assert_eq!(fs::read(&path).unwrap(), saved);

            let index = 88 - cycle;
            let snapshot = expected[index].clone();
            expected[index] = reopened
                .patch_details_expected(
                    &snapshot.id,
                    Some(!snapshot.completed),
                    None,
                    None,
                    None,
                    Some(&snapshot),
                )
                .unwrap();
            saved = fs::read(&path).unwrap();
            drop(reopened);
            assert_eq!(fs::read(&path).unwrap(), saved);
        }

        let reopened = Store::open_at(path).unwrap();
        assert_eq!(reopened.list_events(None).unwrap(), expected);
        drop(reopened);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn missing_primary_after_observation_does_not_become_an_empty_store() {
        let _guard = TEST_LOCK.lock().unwrap();
        let store = test_store();
        for index in 0..89 {
            create_basic_event(
                &store,
                "2031-11-01",
                None,
                &format!("Synthetic event {index:02}"),
                "Synthetic persistence regression",
                Some(&format!("missing-primary-fixture-{index:02}")),
            )
            .unwrap();
        }
        assert_eq!(store.list_events(None).unwrap().len(), 89);
        let directory = store.path.parent().unwrap().to_owned();
        assert!(directory.starts_with(fs::canonicalize(env::temp_dir()).unwrap()));
        let preserved = directory.join("primary-preserved-by-test.json");
        let before = fs::read(&store.path).unwrap();
        fs::rename(&store.path, &preserved).unwrap();
        let result = store.list_events(None);
        assert!(
            result.is_err(),
            "Missing previously observed primary unexpectedly succeeded: {result:?}"
        );
        assert!(
            create_basic_event(&store, "2031-11-02", None, "Do not replace", "", None).is_err()
        );
        assert!(!store.path.exists());
        assert_eq!(fs::read(preserved).unwrap(), before);
        drop(store);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn legacy_read_only_observation_guards_missing_without_creating_marker() {
        let _guard = TEST_LOCK.lock().unwrap();
        let store = test_store();
        let bytes = br#"{"version":1,"events":[{"id":"legacy-fixture","date":"2031-11-01","time":null,"title":"Legacy synthetic","notes":"","request_id":"legacy-request","completed":true,"color":null,"recurrence":null,"reminder_minutes":null}],"retired_request_ids":["previous-deletion"]}"#;
        fs::write(&store.path, bytes).unwrap();
        assert!(store.list_events(None).unwrap()[0].completed);
        assert!(!store.initialized_marker_path().exists());
        assert_eq!(fs::read(&store.path).unwrap(), bytes);
        let directory = store.path.parent().unwrap().to_owned();
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
        let preserved = directory.join("legacy-preserved.json");
        fs::rename(&store.path, &preserved).unwrap();
        assert!(store.list_events(None).is_err());
        assert!(
            create_basic_event(&store, "2031-11-02", None, "No replacement", "", None).is_err()
        );
        assert_eq!(fs::read(preserved).unwrap(), bytes);
        assert!(!store.path.exists());
        drop(store);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn malformed_primary_observation_is_not_forgotten_after_missing() {
        let _guard = TEST_LOCK.lock().unwrap();
        let store = test_store();
        let malformed = b"{ synthetic incomplete primary";
        fs::write(&store.path, malformed).unwrap();
        assert!(store.list_events(None).is_err());
        assert!(!store.initialized_marker_path().exists());
        let directory = store.path.parent().unwrap().to_owned();
        let preserved = directory.join("malformed-preserved.json");
        fs::rename(&store.path, &preserved).unwrap();
        assert!(store.list_events(None).is_err());
        assert!(create_basic_event(&store, "2031-11-02", None, "No reset", "", None).is_err());
        assert!(!store.path.exists());
        assert_eq!(fs::read(preserved).unwrap(), malformed);
        drop(store);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn initialized_marker_guards_reopen_without_archives() {
        let _guard = TEST_LOCK.lock().unwrap();
        let store = test_store();
        assert!(store.list_events(None).unwrap().is_empty());
        assert!(!store.path.exists());
        assert!(!store.initialized_marker_path().exists());
        create_basic_event(
            &store,
            "2031-11-01",
            None,
            "Saved synthetic",
            "",
            Some("saved-request"),
        )
        .unwrap();
        let marker = store.initialized_marker_path();
        assert_eq!(fs::read(&marker).unwrap(), INITIALIZED_MARKER_BYTES);
        assert!(!store.deleted_dir().exists());
        let directory = store.path.parent().unwrap().to_owned();
        let preserved = directory.join("saved-preserved.json");
        let before = fs::read(&store.path).unwrap();
        fs::rename(&store.path, &preserved).unwrap();
        let path = store.path.clone();
        drop(store);
        assert!(Store::open_at(path.clone()).is_err());
        assert!(!path.exists());
        assert_eq!(fs::read(preserved).unwrap(), before);
        assert_eq!(fs::read(marker).unwrap(), INITIALIZED_MARKER_BYTES);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn legacy_archives_guard_missing_primary_and_preserve_retired_ids() {
        let _guard = TEST_LOCK.lock().unwrap();
        let store = test_store();
        let deleted = create_basic_event(
            &store,
            "2031-11-01",
            None,
            "Deleted synthetic",
            "",
            Some("retired-fixture"),
        )
        .unwrap();
        store.delete_event(&deleted.id).unwrap();
        let archive = store.deleted_dir().join(archive_name(&deleted.id));
        let archived_bytes = fs::read(&archive).unwrap();
        let sequence = store.deleted_dir().join("sequence.txt");
        let sequence_bytes = fs::read(&sequence).unwrap();
        assert_eq!(
            store.read_database().unwrap().retired_request_ids,
            vec!["retired-fixture"]
        );
        fs::remove_file(store.initialized_marker_path()).unwrap();
        let directory = store.path.parent().unwrap().to_owned();
        let preserved = directory.join("retired-preserved.json");
        let before = fs::read(&store.path).unwrap();
        fs::rename(&store.path, &preserved).unwrap();
        let path = store.path.clone();
        drop(store);
        assert!(Store::open_at(path.clone()).is_err());
        assert!(!path.exists());
        assert_eq!(fs::read(preserved).unwrap(), before);
        assert_eq!(fs::read(archive).unwrap(), archived_bytes);
        assert_eq!(fs::read(sequence).unwrap(), sequence_bytes);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn interrupted_first_save_with_marker_and_candidates_is_not_auto_initialized() {
        let _guard = TEST_LOCK.lock().unwrap();
        let store = test_store();
        store
            .with_lock(|store| store.ensure_initialized_marker())
            .unwrap();
        let directory = store.path.parent().unwrap().to_owned();
        let partial = directory.join(".calendar-synthetic-partial.tmp");
        let complete = directory.join(".calendar-synthetic-complete.tmp");
        let partial_bytes = b"{\"version\":1,\"events\":[";
        let complete_bytes =
            br#"{"version":1,"events":[],"retired_request_ids":["uncommitted-fixture"]}"#;
        fs::write(&partial, partial_bytes).unwrap();
        fs::write(&complete, complete_bytes).unwrap();
        let marker = store.initialized_marker_path();
        let path = store.path.clone();
        drop(store);
        assert!(Store::open_at(path.clone()).is_err());
        assert!(Store::open_at(path.clone()).is_err());
        assert!(!path.exists());
        assert_eq!(fs::read(partial).unwrap(), partial_bytes);
        assert_eq!(fs::read(complete).unwrap(), complete_bytes);
        assert_eq!(fs::read(marker).unwrap(), INITIALIZED_MARKER_BYTES);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn failed_first_save_removes_only_its_new_marker_and_allows_retry() {
        let _guard = TEST_LOCK.lock().unwrap();
        let store = test_store();
        FAIL_PRIMARY_WRITE.with(|fail| fail.set(true));
        assert_eq!(
            create_basic_event(
                &store,
                "2031-11-01",
                None,
                "Retry synthetic",
                "",
                Some("first-retry")
            )
            .unwrap_err(),
            "synthetic primary write failure"
        );
        assert!(!store.path.exists());
        assert!(!store.initialized_marker_path().exists());
        assert!(store.list_events(None).unwrap().is_empty());
        let saved = create_basic_event(
            &store,
            "2031-11-01",
            None,
            "Retry synthetic",
            "",
            Some("first-retry"),
        )
        .unwrap();
        assert_eq!(store.list_events(None).unwrap(), vec![saved]);
        let directory = store.path.parent().unwrap().to_owned();
        drop(store);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn marker_failure_and_failed_legacy_write_preserve_primary_and_retired_ids() {
        let _guard = TEST_LOCK.lock().unwrap();
        let store = test_store();
        let deleted = create_basic_event(
            &store,
            "2031-11-01",
            None,
            "Deleted synthetic",
            "",
            Some("retain-retired"),
        )
        .unwrap();
        store.delete_event(&deleted.id).unwrap();
        let kept = create_basic_event(
            &store,
            "2031-11-02",
            None,
            "Kept synthetic",
            "",
            Some("retain-active"),
        )
        .unwrap();
        let before = fs::read(&store.path).unwrap();
        let marker = store.initialized_marker_path();
        fs::remove_file(&marker).unwrap();
        fs::create_dir(&marker).unwrap();
        assert!(
            create_basic_event(&store, "2031-11-03", None, "Blocked marker", "", None)
                .unwrap_err()
                .starts_with("calendar initialization marker is invalid")
        );
        assert_eq!(fs::read(&store.path).unwrap(), before);
        assert_eq!(store.list_events(None).unwrap(), vec![kept]);
        assert_eq!(
            store.read_database().unwrap().retired_request_ids,
            vec!["retain-retired"]
        );
        fs::remove_dir(&marker).unwrap();
        FAIL_PRIMARY_WRITE.with(|fail| fail.set(true));
        assert_eq!(
            create_basic_event(&store, "2031-11-03", None, "Failed primary", "", None).unwrap_err(),
            "synthetic primary write failure"
        );
        assert_eq!(fs::read(&store.path).unwrap(), before);
        assert_eq!(fs::read(marker).unwrap(), INITIALIZED_MARKER_BYTES);
        let directory = store.path.parent().unwrap().to_owned();
        drop(store);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn invalid_initialization_marker_with_missing_primary_is_preserved() {
        let _guard = TEST_LOCK.lock().unwrap();
        let store = test_store();
        let marker = store.initialized_marker_path();
        let invalid = b"unknown initialization format";
        fs::write(&marker, invalid).unwrap();
        let path = store.path.clone();
        let directory = path.parent().unwrap().to_owned();
        drop(store);
        assert!(Store::open_at(path.clone()).is_err());
        assert!(!path.exists());
        assert_eq!(fs::read(marker).unwrap(), invalid);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn failed_atomic_replace_preserves_original_and_cleans_temporary_file() {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Foundation::WAIT_TIMEOUT;
        use windows_sys::Win32::Storage::FileSystem::{FILE_SHARE_READ, FILE_SHARE_WRITE};

        let _guard = TEST_LOCK.lock().unwrap();
        let store = test_store();
        let original =
            create_basic_event(&store, "2026-10-11", None, "Keep original", "", None).unwrap();
        let before = fs::read(&store.path).unwrap();
        assert_eq!(
            unsafe { WaitForSingleObject(store.changed, 0) },
            WAIT_OBJECT_0
        );
        // A reader allowing reads and writes, but not deletion, blocks the final
        // atomic replacement after the new temporary file has been flushed.
        let reader = OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .open(&store.path)
            .unwrap();
        let error = store
            .patch_details_expected(&original.id, Some(true), None, None, None, Some(&original))
            .unwrap_err();
        assert!(error.starts_with("cannot replace calendar: "), "{error}");
        assert_eq!(fs::read(&store.path).unwrap(), before);
        assert_eq!(store.list_events(None).unwrap(), vec![original.clone()]);
        assert_eq!(
            unsafe { WaitForSingleObject(store.changed, 0) },
            WAIT_TIMEOUT
        );
        let directory = store.path.parent().unwrap().to_owned();
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 2);
        assert_eq!(
            fs::read(store.initialized_marker_path()).unwrap(),
            INITIALIZED_MARKER_BYTES
        );
        drop(reader);

        let updated = store
            .patch_details_expected(&original.id, Some(true), None, None, None, Some(&original))
            .unwrap();
        let path = store.path.clone();
        drop(store);
        let reopened = Store::open_at(path).unwrap();
        assert_eq!(reopened.list_events(None).unwrap(), vec![updated]);
        drop(reopened);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn separate_store_preserves_external_additions_and_rejects_stale_details() {
        let _guard = TEST_LOCK.lock().unwrap();
        let store = test_store();
        let original =
            create_basic_event(&store, "2026-10-11", None, "Original", "", None).unwrap();
        let external = Store::open_at(store.path.clone()).unwrap();
        let edited = external
            .patch_details_expected(&original.id, Some(true), None, None, None, Some(&original))
            .unwrap();
        let externally_added =
            create_basic_event(&external, "2026-10-11", None, "External", "", None).unwrap();
        let locally_added =
            create_basic_event(&store, "2026-10-11", None, "Local", "", None).unwrap();
        let saved = fs::read(&store.path).unwrap();
        assert_eq!(
            store
                .patch_details_expected(
                    &original.id,
                    Some(false),
                    None,
                    None,
                    None,
                    Some(&original),
                )
                .unwrap_err(),
            "event changed since it was read"
        );
        assert_eq!(fs::read(&store.path).unwrap(), saved);
        assert_eq!(
            store.list_events(None).unwrap(),
            vec![externally_added, locally_added, edited]
        );
        let directory = store.path.parent().unwrap().to_owned();
        drop(external);
        drop(store);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn deleted_event_survives_reopen_and_restores_exact_details() {
        let _guard = TEST_LOCK.lock().unwrap();
        let store = test_store();
        let created = store
            .create_event_with_details(
                "2026-10-15",
                Some("08:30"),
                "복구할 일정",
                "전체 메모\n둘째 줄",
                Some("restore-original"),
                true,
                Some("#83AACD"),
                Some("weekly"),
                Some(15),
            )
            .unwrap();
        assert_eq!(store.delete_event(&created.id).unwrap(), created);
        assert!(store.list_events(None).unwrap().is_empty());
        let reopened = Store::open_at(store.path.clone()).unwrap();
        assert_eq!(
            reopened.list_deleted_events().unwrap(),
            vec![created.clone()]
        );
        assert_eq!(
            reopened.restore_deleted_event(&created.id).unwrap(),
            created
        );
        assert_eq!(reopened.list_events(None).unwrap(), vec![created.clone()]);
        assert!(reopened.list_deleted_events().unwrap().is_empty());
        assert_eq!(
            create_basic_event(
                &reopened,
                &created.date,
                created.time.as_deref(),
                &created.title,
                &created.notes,
                created.request_id.as_deref()
            )
            .unwrap_err(),
            "request-id already belongs to a different event payload"
        );
        let _ = fs::remove_dir_all(store.path.parent().unwrap());
    }

    #[test]
    fn restore_last_deleted_keeps_order_and_other_retired_requests() {
        let _guard = TEST_LOCK.lock().unwrap();
        let store = test_store();
        let first =
            create_basic_event(&store, "2026-10-08", None, "First", "", Some("first-r")).unwrap();
        let second =
            create_basic_event(&store, "2026-10-09", None, "Second", "", Some("second-r")).unwrap();
        assert!(store.delete_if_unchanged(&first).unwrap());
        store.delete_event(&second.id).unwrap();
        assert_eq!(
            store.list_deleted_events().unwrap(),
            vec![second.clone(), first.clone()]
        );
        assert_eq!(store.restore_last_deleted().unwrap(), second);
        assert!(
            create_basic_event(&store, "2026-10-08", None, "Other", "", Some("first-r")).is_err()
        );
        assert_eq!(store.restore_last_deleted().unwrap(), first);
        assert!(store.list_deleted_events().unwrap().is_empty());
        let _ = fs::remove_dir_all(store.path.parent().unwrap());
    }

    #[test]
    fn failed_archive_and_stale_delete_leave_active_event_untouched() {
        let _guard = TEST_LOCK.lock().unwrap();
        let store = test_store();
        let original = create_basic_event(&store, "2026-10-08", None, "Keep", "", None).unwrap();
        let changed = store
            .update_event_with_details(
                &original.id,
                "2026-10-08",
                None,
                "Keep",
                "edited",
                None,
                None,
                None,
                None,
            )
            .unwrap();
        assert!(store
            .delete_event_expected(&original.id, Some(&original))
            .is_err());
        assert!(!store.delete_if_unchanged(&original).unwrap());
        assert!(store.list_deleted_events().unwrap().is_empty());
        fs::write(store.deleted_dir(), b"not a directory").unwrap();
        assert!(store.delete_event(&changed.id).is_err());
        assert!(store.delete_if_unchanged(&changed).is_err());
        assert_eq!(store.list_events(None).unwrap(), vec![changed]);
        let _ = fs::remove_dir_all(store.path.parent().unwrap());
    }

    #[test]
    fn restore_collision_preserves_archived_event() {
        let _guard = TEST_LOCK.lock().unwrap();
        let store = test_store();
        let original = create_basic_event(
            &store,
            "2026-10-08",
            None,
            "Original",
            "",
            Some("collision-r"),
        )
        .unwrap();
        let backup = store.export_json().unwrap();
        store.delete_event(&original.id).unwrap();
        store.restore_json(&backup).unwrap();
        assert!(store.restore_deleted_event(&original.id).is_err());
        assert_eq!(store.list_events(None).unwrap(), vec![original.clone()]);
        assert!(store
            .deleted_dir()
            .join(archive_name(&original.id))
            .exists());
        let _ = fs::remove_dir_all(store.path.parent().unwrap());
    }

    #[test]
    fn restore_size_failure_preserves_archived_event() {
        let _guard = TEST_LOCK.lock().unwrap();
        let store = test_store();
        let original =
            create_basic_event(&store, "2026-10-08", None, "Archived", "", Some("size-r")).unwrap();
        store.delete_event(&original.id).unwrap();
        let archive = store.deleted_dir().join(archive_name(&original.id));
        let before = fs::read(&archive).unwrap();

        let mut db = store.read_database().unwrap();
        let mut filler = original.clone();
        filler.id = "f".into();
        filler.request_id = None;
        db.events.push(filler);
        let base_size = serde_json::to_vec_pretty(&db).unwrap().len();
        db.events[0].id = "f".repeat(MAX_FILE_BYTES as usize - base_size - 31);
        store.write_database(&db).unwrap();

        assert!(store.restore_deleted_event(&original.id).is_err());
        assert_eq!(fs::read(&archive).unwrap(), before);
        assert_eq!(store.list_deleted_events().unwrap(), vec![original]);
        let _ = fs::remove_dir_all(store.path.parent().unwrap());
    }

    #[test]
    fn selected_delete_uses_locked_snapshot_and_does_not_write_on_selection_error() {
        let _guard = TEST_LOCK.lock().unwrap();
        let store = test_store();
        let first = create_basic_event(&store, "2026-10-08", None, "Keep", "", None).unwrap();
        let second = create_basic_event(&store, "2026-10-09", None, "Delete", "", None).unwrap();
        let before = fs::read(&store.path).unwrap();
        assert_eq!(
            store
                .delete_event_selected(|events| {
                    assert_eq!(events.len(), 2);
                    Err("ambiguous selection".into())
                })
                .unwrap_err(),
            "ambiguous selection"
        );
        assert!(store
            .delete_event_selected(|_| Ok("missing".into()))
            .is_err());
        assert_eq!(fs::read(&store.path).unwrap(), before);
        assert!(!store.deleted_dir().exists());
        assert_eq!(
            store
                .delete_event_selected(|events| {
                    Ok(events
                        .iter()
                        .find(|event| event.title == "Delete")
                        .unwrap()
                        .id
                        .clone())
                })
                .unwrap(),
            second
        );
        assert_eq!(store.list_events(None).unwrap(), vec![first]);
        assert_eq!(store.list_deleted_events().unwrap(), vec![second]);
        let _ = fs::remove_dir_all(store.path.parent().unwrap());
    }

    #[test]
    fn delete_sequence_persists_without_scanning_other_archived_bodies() {
        let _guard = TEST_LOCK.lock().unwrap();
        let store = test_store();
        let first = create_basic_event(&store, "2026-10-08", None, "First", "", None).unwrap();
        store.delete_event(&first.id).unwrap();
        let first_sequence = store.read_deleted_events().unwrap()[0].sequence;
        assert_eq!(first_sequence, 1);

        let broken = store.deleted_dir().join(archive_name("unrelated-broken"));
        fs::write(&broken, b"{broken").unwrap();
        let reopened = Store::open_at(store.path.clone()).unwrap();
        let second = create_basic_event(&reopened, "2026-10-09", None, "Second", "", None).unwrap();
        reopened.delete_event(&second.id).unwrap();
        assert_eq!(
            fs::read_to_string(store.deleted_dir().join("sequence.txt")).unwrap(),
            "2"
        );
        assert_eq!(reopened.restore_deleted_event(&first.id).unwrap(), first);
        fs::remove_file(broken).unwrap();
        assert_eq!(
            reopened
                .read_deleted_events()
                .unwrap()
                .iter()
                .map(|item| item.sequence)
                .max(),
            Some(2)
        );
        let _ = fs::remove_dir_all(store.path.parent().unwrap());
    }

    #[test]
    fn inline_blank_notes_delete_with_conflict_and_retry_protection() {
        let _guard = TEST_LOCK.lock().unwrap();
        let store = test_store();
        let date = Date::parse("2026-10-08").unwrap();
        crate::widget::save_inline_note(&store, date, None, " \r\n\t").unwrap();
        assert!(!store.path.exists());
        let original = create_basic_event(
            &store,
            "2026-10-08",
            Some("14:00"),
            "원본",
            "본문",
            Some("clear-note"),
        )
        .unwrap();
        crate::widget::save_inline_note(&store, date, Some(&original), "\r\n 남은 본문\r\n둘째 줄")
            .unwrap();
        let edited = store.list_events(None).unwrap().remove(0);
        assert_eq!(edited.title, "남은 본문");
        assert_eq!(edited.notes, "둘째 줄");
        assert_eq!(edited.time, original.time);
        assert!(crate::widget::save_inline_note(&store, date, Some(&original), "").is_err());
        assert_eq!(store.list_events(None).unwrap(), vec![edited.clone()]);
        crate::widget::save_inline_note(&store, date, Some(&edited), " \n").unwrap();
        assert!(store.list_events(None).unwrap().is_empty());
        assert!(
            create_basic_event(&store, "2026-10-08", None, "재시도", "", Some("clear-note"))
                .is_err()
        );
        let _ = fs::remove_dir_all(store.path.parent().unwrap());
    }

    #[test]
    fn invalid_input_does_not_write() {
        let _guard = TEST_LOCK.lock().unwrap();
        let store = test_store();
        assert!(create_basic_event(&store, "2025-02-29", None, "x", "", None).is_err());
        assert!(create_basic_event(&store, "2026-10-07", Some("24:00"), "x", "", None).is_err());
        assert!(create_basic_event(&store, "2026-10-07", None, "  ", "", None).is_err());
        assert!(store.list_events(None).unwrap().is_empty());
        assert!(!store.path.exists());
        let _ = fs::remove_dir_all(store.path.parent().unwrap());
    }

    #[test]
    fn malformed_database_is_preserved() {
        let _guard = TEST_LOCK.lock().unwrap();
        let store = test_store();
        fs::write(&store.path, b"{ broken").unwrap();
        assert!(Store::open_at(store.path.clone()).is_err());
        assert!(create_basic_event(&store, "2026-10-07", None, "x", "", None).is_err());
        assert_eq!(fs::read(&store.path).unwrap(), b"{ broken");
        let _ = fs::remove_dir_all(store.path.parent().unwrap());
    }

    #[test]
    fn old_json_defaults_and_series_details() {
        let _guard = TEST_LOCK.lock().unwrap();
        let store = test_store();
        fs::write(&store.path, br##"{"version":1,"events":[{"id":"old","date":"2026-10-07","time":null,"title":"Old","notes":"","request_id":null}]}"##).unwrap();
        let reopened = Store::open_at(store.path.clone()).unwrap();
        let event = reopened.list_events(None).unwrap().remove(0);
        assert!(!event.completed);
        assert!(
            event.color.is_none() && event.recurrence.is_none() && event.reminder_minutes.is_none()
        );
        let updated = reopened
            .patch_details(
                "old",
                Some(true),
                Some(Some("#12abef")),
                Some(Some("weekly")),
                Some(Some(30)),
            )
            .unwrap();
        assert!(updated.completed);
        assert_eq!(updated.color.as_deref(), Some("#12ABEF"));
        assert_eq!(updated.recurrence.as_deref(), Some("weekly"));
        assert!(reopened
            .patch_details(
                "old",
                Some(false),
                Some(Some("red")),
                Some(None),
                Some(None)
            )
            .is_err());
        assert_eq!(reopened.list_events(None).unwrap()[0], updated);
        let _ = fs::remove_dir_all(store.path.parent().unwrap());
    }

    #[test]
    fn expected_snapshot_rejects_stale_update_details_and_delete() {
        let _guard = TEST_LOCK.lock().unwrap();
        let store = test_store();
        let original = create_basic_event(
            &store,
            "2026-10-09",
            Some("15:00"),
            "Meeting",
            "",
            Some("meeting-1"),
        )
        .unwrap();
        let external = store
            .update_event_with_details(
                &original.id,
                &original.date,
                original.time.as_deref(),
                &original.title,
                "Room B",
                None,
                None,
                None,
                None,
            )
            .unwrap();
        assert_eq!(
            store
                .update_event_with_details_expected(
                    &original.id,
                    &original.date,
                    Some("16:00"),
                    &original.title,
                    &original.notes,
                    None,
                    None,
                    None,
                    None,
                    Some(&original),
                )
                .unwrap_err(),
            "event changed since it was read"
        );
        assert!(store
            .patch_details_expected(&original.id, Some(true), None, None, None, Some(&original))
            .is_err());
        assert!(store
            .delete_event_expected(&original.id, Some(&original))
            .is_err());
        assert_eq!(store.list_events(None).unwrap(), vec![external.clone()]);

        let moved = store
            .update_event_with_details_expected(
                &external.id,
                &external.date,
                Some("16:00"),
                &external.title,
                &external.notes,
                None,
                None,
                None,
                None,
                Some(&external),
            )
            .unwrap();
        assert_eq!(moved.time.as_deref(), Some("16:00"));
        assert_eq!(moved.notes, "Room B");
        assert!(store
            .delete_event_expected(&moved.id, Some(&external))
            .is_err());
        assert_eq!(
            store
                .delete_event_expected(&moved.id, Some(&moved))
                .unwrap(),
            moved
        );
        assert!(store.list_events(None).unwrap().is_empty());
        let _ = fs::remove_dir_all(store.path.parent().unwrap());
    }

    #[test]
    fn compare_and_swap_preserves_concurrent_edits() {
        let _guard = TEST_LOCK.lock().unwrap();
        let store = test_store();
        let original =
            create_basic_event(&store, "2026-10-07", None, "Task", "", Some("sync-1")).unwrap();
        let locally_changed = store
            .update_event_with_details(
                &original.id,
                "2026-10-08",
                None,
                "Task",
                "local edit",
                None,
                None,
                None,
                None,
            )
            .unwrap();
        let mut remote_change = original.clone();
        remote_change.title = "remote edit".into();
        assert!(!store
            .replace_if_unchanged(&original, &remote_change)
            .unwrap());
        assert!(!store.delete_if_unchanged(&original).unwrap());
        assert!(store
            .replace_if_unchanged(&locally_changed, &remote_change)
            .unwrap());
        assert_eq!(store.list_events(None).unwrap()[0].title, "remote edit");
        assert!(store.delete_if_unchanged(&remote_change).unwrap());
        assert!(store.list_events(None).unwrap().is_empty());
        let _ = fs::remove_dir_all(store.path.parent().unwrap());
    }

    #[test]
    fn backup_restore_round_trip_and_invalid_backup_is_rejected() {
        let _guard = TEST_LOCK.lock().unwrap();
        let store = test_store();
        let saved =
            create_basic_event(&store, "2026-10-07", None, "Backup", "", Some("backup-1")).unwrap();
        let backup = store.export_json().unwrap();
        store.delete_event(&saved.id).unwrap();
        assert!(store.restore_json("{bad").is_err());
        assert!(store.list_events(None).unwrap().is_empty());
        store.restore_json(&backup).unwrap();
        assert_eq!(store.list_events(None).unwrap(), vec![saved]);
        let _ = fs::remove_dir_all(store.path.parent().unwrap());
    }

    #[test]
    fn detailed_create_retry_and_partial_update_are_atomic() {
        let _guard = TEST_LOCK.lock().unwrap();
        let store = test_store();
        let created = store
            .create_event_with_details(
                "2026-10-07",
                Some("08:00"),
                "Daily",
                "",
                Some("details-1"),
                false,
                Some("#aabbcc"),
                Some("daily"),
                Some(10),
            )
            .unwrap();
        assert_eq!(created.color.as_deref(), Some("#AABBCC"));
        assert_eq!(
            created,
            store
                .create_event_with_details(
                    "2026-10-07",
                    Some("08:00"),
                    "Daily",
                    "",
                    Some("details-1"),
                    false,
                    Some("#AABBCC"),
                    Some("daily"),
                    Some(10),
                )
                .unwrap()
        );
        assert!(store
            .create_event_with_details(
                "2026-10-07",
                Some("08:00"),
                "Daily",
                "",
                Some("details-1"),
                false,
                Some("#AABBCC"),
                Some("weekly"),
                Some(10),
            )
            .is_err());
        let updated = store
            .update_event_with_details(
                &created.id,
                "2026-10-08",
                None,
                "Moved",
                "",
                Some(true),
                Some(None),
                None,
                None,
            )
            .unwrap();
        assert!(updated.completed && updated.color.is_none());
        assert_eq!(updated.recurrence.as_deref(), Some("daily"));
        assert_eq!(updated.reminder_minutes, Some(10));
        assert!(store
            .update_event_with_details(
                &created.id,
                "2026-10-09",
                None,
                "Moved",
                "",
                None,
                None,
                Some(Some("invalid")),
                None,
            )
            .is_err());
        assert_eq!(store.list_events(None).unwrap()[0], updated);
        let _ = fs::remove_dir_all(store.path.parent().unwrap());
    }

    #[test]
    fn unchanged_edits_do_not_signal_or_rewrite() {
        use windows_sys::Win32::Foundation::WAIT_TIMEOUT;

        let _guard = TEST_LOCK.lock().unwrap();
        let store = test_store();
        let created = create_basic_event(&store, "2026-10-07", None, "Quiet", "", None).unwrap();
        assert_eq!(
            unsafe { WaitForSingleObject(store.changed, 0) },
            WAIT_OBJECT_0
        );
        let before = fs::metadata(&store.path).unwrap().modified().unwrap();
        assert_eq!(
            store
                .update_event_with_details(
                    &created.id,
                    "2026-10-07",
                    None,
                    "Quiet",
                    "",
                    None,
                    None,
                    None,
                    None
                )
                .unwrap(),
            created
        );
        assert_eq!(
            store
                .patch_details(&created.id, None, None, None, None)
                .unwrap(),
            created
        );
        let backup = store.export_json().unwrap();
        store.restore_json(&backup).unwrap();
        assert_eq!(
            unsafe { WaitForSingleObject(store.changed, 0) },
            WAIT_TIMEOUT
        );
        assert_eq!(
            fs::metadata(&store.path).unwrap().modified().unwrap(),
            before
        );
        let _ = fs::remove_dir_all(store.path.parent().unwrap());
    }
}
