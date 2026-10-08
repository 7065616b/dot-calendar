use std::collections::HashSet;
use std::env;
use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::windows::ffi::OsStrExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, WAIT_ABANDONED, WAIT_OBJECT_0};
use windows_sys::Win32::Storage::FileSystem::{
    MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
};
use windows_sys::Win32::System::Threading::{
    CreateEventW, CreateMutexW, ReleaseMutex, SetEvent, WaitForSingleObject, INFINITE,
};

use crate::model::{Date, Event};

const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;
static NEXT_ID: AtomicU64 = AtomicU64::new(0);

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
}

struct LockedMutex(HANDLE);

impl Drop for LockedMutex {
    fn drop(&mut self) {
        unsafe { ReleaseMutex(self.0) };
    }
}

impl Store {
    pub fn open() -> Result<Self, String> {
        let directory = match env::var_os("DOT_CALENDAR_DATA_DIR") {
            Some(value) if !value.is_empty() => PathBuf::from(value),
            _ => {
                let local = env::var_os("LOCALAPPDATA")
                    .ok_or("LOCALAPPDATA is unavailable; set DOT_CALENDAR_DATA_DIR")?;
                PathBuf::from(local).join("DotCalendar")
            }
        };
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
        self.with_lock(|store| {
            let mut db = store.read_database()?;
            let position =
                db.events
                    .iter()
                    .position(|event| event.id == id)
                    .ok_or(if expected.is_some() {
                        "event changed since it was read"
                    } else {
                        "event not found"
                    })?;
            if expected.is_some_and(|snapshot| snapshot != &db.events[position]) {
                return Err("event changed since it was read".into());
            }
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
            let removed = db.events.remove(position);
            if let Some(request_id) = removed.request_id {
                db.retired_request_ids.push(request_id);
            }
            store.write_database(&db)?;
            Ok(true)
        })
    }

    /// Export a complete backup, including deleted request IDs used for retry safety.
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
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Database::default())
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
        let mut ids = HashSet::new();
        let mut requests = HashSet::new();
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
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let temp = parent.join(format!(
            ".calendar-{}-{stamp:x}-{}.tmp",
            std::process::id(),
            next_number()
        ));
        let result = (|| {
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
        } else {
            // A failed notification does not turn a successful durable write into a retryable error.
            unsafe { SetEvent(self.changed) };
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
    if value.is_empty() || value.chars().count() > 200 {
        return Err("title must contain 1–200 characters".into());
    }
    Ok(value)
}

fn valid_notes(value: &str) -> Result<(), String> {
    if value.chars().count() > 4096 {
        return Err("notes cannot exceed 4096 characters".into());
    }
    Ok(())
}

fn valid_request_id(value: Option<&str>) -> Result<Option<&str>, String> {
    let Some(value) = value else { return Ok(None) };
    if value.is_empty() || value.chars().count() > 128 {
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
    use std::sync::Mutex;

    static TEST_LOCK: Mutex<()> = Mutex::new(());

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
