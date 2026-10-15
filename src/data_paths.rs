use std::env;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};

const SHARED_DIRECTORY: &str = ".dot-calendar";
const STORE_TRACES: [&str; 3] = ["calendar.json", "calendar.initialized", "deleted"];
const LEGACY_SETTINGS: [&str; 4] = [
    "widget.json",
    "window.json",
    "google-config.json",
    "google-tokens.dpapi",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PathKind {
    Missing,
    File,
    Directory,
    Other,
}

trait Probe {
    fn kind(&mut self, path: &Path) -> Result<PathKind, String>;
    fn names(&mut self, directory: &Path) -> Result<Vec<OsString>, String>;
}

struct FileSystemProbe;

impl Probe for FileSystemProbe {
    fn kind(&mut self, path: &Path) -> Result<PathKind, String> {
        match fs::symlink_metadata(path) {
            Ok(metadata) if metadata.file_attributes() & 0x400 != 0 => Ok(PathKind::Other),
            Ok(metadata) if metadata.is_file() => Ok(PathKind::File),
            Ok(metadata) if metadata.is_dir() => Ok(PathKind::Directory),
            Ok(_) => Ok(PathKind::Other),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(PathKind::Missing),
            Err(error) => Err(format!(
                "cannot inspect calendar path {}: {error}",
                path.display()
            )),
        }
    }

    fn names(&mut self, directory: &Path) -> Result<Vec<OsString>, String> {
        fs::read_dir(directory)
            .map_err(|error| {
                format!(
                    "cannot inspect legacy calendar directory {}: {error}",
                    directory.display()
                )
            })?
            .map(|entry| {
                entry
                    .map(|entry| entry.file_name())
                    .map_err(|error| format!("cannot inspect legacy calendar entry: {error}"))
            })
            .collect()
    }
}

/// Resolve one directory for the GUI, CLI, preferences, and Google integration.
/// This function only inspects paths: migration and directory creation are explicit
/// operations performed elsewhere, never a fallback from an unreadable old store.
pub fn directory() -> Result<PathBuf, String> {
    let explicit = env::var_os("DOT_CALENDAR_DATA_DIR");
    let profile = env::var_os("USERPROFILE");
    let local = env::var_os("LOCALAPPDATA");
    resolve(
        explicit.as_deref(),
        profile.as_deref(),
        local.as_deref(),
        &mut FileSystemProbe,
    )
}

fn absolute(value: Option<&OsStr>, variable: &str) -> Result<PathBuf, String> {
    let value = value.filter(|value| !value.is_empty()).ok_or_else(|| {
        format!("{variable} is unavailable; set an absolute DOT_CALENDAR_DATA_DIR")
    })?;
    let path = PathBuf::from(value);
    if !path.is_absolute() {
        return Err(format!("{variable} must be an absolute path"));
    }
    Ok(path)
}

fn directory_kind(path: &Path, probe: &mut impl Probe) -> Result<PathKind, String> {
    let kind = probe.kind(path)?;
    if !matches!(kind, PathKind::Missing | PathKind::Directory) {
        return Err(format!(
            "calendar data directory is not a regular directory: {}",
            path.display()
        ));
    }
    Ok(kind)
}

fn has_store_trace(directory: &Path, probe: &mut impl Probe) -> Result<bool, String> {
    for name in STORE_TRACES {
        if probe.kind(&directory.join(name))? != PathKind::Missing {
            return Ok(true);
        }
    }
    Ok(false)
}

fn legacy_setting(path: &Path, probe: &mut impl Probe) -> Result<bool, String> {
    match probe.kind(path)? {
        PathKind::Missing => Ok(false),
        PathKind::File => Ok(true),
        _ => Err(format!(
            "legacy calendar settings path is not a regular file: {}",
            path.display()
        )),
    }
}

fn resolve(
    explicit: Option<&OsStr>,
    profile: Option<&OsStr>,
    local: Option<&OsStr>,
    probe: &mut impl Probe,
) -> Result<PathBuf, String> {
    if explicit.is_some_and(|value| !value.is_empty()) {
        return absolute(explicit, "DOT_CALENDAR_DATA_DIR");
    }

    let shared = absolute(profile, "USERPROFILE")?.join(SHARED_DIRECTORY);
    directory_kind(&shared, probe)?;
    // Any primary/marker/archive trace identifies the prepared shared store.
    // The Store validates its contents and refuses a missing or corrupt primary.
    if has_store_trace(&shared, probe)? {
        return Ok(shared);
    }

    let legacy = absolute(local, "LOCALAPPDATA")?.join("DotCalendar");
    let legacy_kind = directory_kind(&legacy, probe)?;
    let mut legacy_present = has_store_trace(&legacy, probe)?;
    if !legacy_present {
        for name in LEGACY_SETTINGS {
            if legacy_setting(&legacy.join(name), probe)? {
                legacy_present = true;
                break;
            }
        }
    }
    if !legacy_present && legacy_kind == PathKind::Directory {
        for name in probe.names(&legacy)? {
            let name_text = name.to_string_lossy().to_ascii_lowercase();
            if name_text.starts_with("google-sync-")
                && name_text.ends_with(".json")
                && legacy_setting(&legacy.join(&name), probe)?
            {
                legacy_present = true;
                break;
            }
        }
    }
    if legacy_present {
        return Err(format!(
            "calendar migration required: existing data or settings at {} must be reviewed and explicitly migrated to {}; no data was copied or initialized",
            legacy.display(), shared.display()
        ));
    }
    Ok(shared)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    const PROFILE: &str = r"C:\Users\Synthetic";
    const LOCAL: &str = r"C:\Users\Synthetic\AppData\Local";

    #[derive(Default)]
    struct MockProbe {
        paths: HashMap<PathBuf, PathKind>,
        inspected: Vec<PathBuf>,
        error_at: Option<PathBuf>,
        names_error: bool,
    }

    impl Probe for MockProbe {
        fn kind(&mut self, path: &Path) -> Result<PathKind, String> {
            self.inspected.push(path.to_path_buf());
            if self.error_at.as_deref() == Some(path) {
                return Err("synthetic read access denied".into());
            }
            Ok(self.paths.get(path).copied().unwrap_or(PathKind::Missing))
        }

        fn names(&mut self, directory: &Path) -> Result<Vec<OsString>, String> {
            if self.names_error {
                return Err("synthetic directory read failed".into());
            }
            Ok(self
                .paths
                .keys()
                .filter(|path| path.parent() == Some(directory))
                .filter_map(|path| path.file_name().map(OsStr::to_os_string))
                .collect())
        }
    }

    fn shared() -> PathBuf {
        PathBuf::from(PROFILE).join(SHARED_DIRECTORY)
    }

    fn legacy() -> PathBuf {
        PathBuf::from(LOCAL).join("DotCalendar")
    }

    fn default_resolve(probe: &mut MockProbe) -> Result<PathBuf, String> {
        resolve(
            None,
            Some(OsStr::new(PROFILE)),
            Some(OsStr::new(LOCAL)),
            probe,
        )
    }

    #[test]
    fn explicit_absolute_directory_needs_no_default_environment_or_probe() {
        let mut probe = MockProbe::default();
        let path = r"E:\Synthetic\isolated";
        assert_eq!(
            resolve(Some(OsStr::new(path)), None, None, &mut probe).unwrap(),
            PathBuf::from(path)
        );
        assert!(probe.inspected.is_empty());
    }

    #[test]
    fn relative_override_is_an_error_instead_of_a_default_fallback() {
        let mut probe = MockProbe::default();
        for path in ["relative", r"C:relative", r"\rooted-without-drive"] {
            assert!(resolve(
                Some(OsStr::new(path)),
                Some(OsStr::new(PROFILE)),
                Some(OsStr::new(LOCAL)),
                &mut probe
            )
            .unwrap_err()
            .contains("DOT_CALENDAR_DATA_DIR must be an absolute path"));
        }
        assert!(probe.inspected.is_empty());
    }

    #[test]
    fn missing_empty_or_relative_profile_never_uses_the_working_directory() {
        for profile in [None, Some(OsStr::new("")), Some(OsStr::new("relative"))] {
            let mut probe = MockProbe::default();
            assert!(resolve(None, profile, Some(OsStr::new(LOCAL)), &mut probe).is_err());
            assert!(probe.inspected.is_empty());
        }
    }

    #[test]
    fn virgin_install_and_empty_override_select_shared_directory() {
        let mut probe = MockProbe::default();
        assert_eq!(default_resolve(&mut probe).unwrap(), shared());
        assert_eq!(
            resolve(
                Some(OsStr::new("")),
                Some(OsStr::new(PROFILE)),
                Some(OsStr::new(LOCAL)),
                &mut probe
            )
            .unwrap(),
            shared()
        );
    }

    #[test]
    fn empty_legacy_directory_and_unrelated_files_do_not_require_migration() {
        let mut probe = MockProbe::default();
        probe.paths.insert(legacy(), PathKind::Directory);
        probe
            .paths
            .insert(legacy().join("unrelated.txt"), PathKind::File);
        assert_eq!(default_resolve(&mut probe).unwrap(), shared());
    }

    #[test]
    fn prepared_shared_store_wins_without_inspecting_legacy_or_decoding_contents() {
        for name in STORE_TRACES {
            let mut probe = MockProbe::default();
            probe.paths.insert(shared(), PathKind::Directory);
            probe.paths.insert(shared().join(name), PathKind::File);
            probe.error_at = Some(legacy());
            assert_eq!(default_resolve(&mut probe).unwrap(), shared());
            assert!(!probe.inspected.contains(&legacy()));
        }
    }

    #[test]
    fn shared_trace_with_invalid_type_is_left_for_store_validation() {
        for kind in [PathKind::Directory, PathKind::Other] {
            let mut probe = MockProbe::default();
            probe.paths.insert(shared().join("calendar.json"), kind);
            assert_eq!(default_resolve(&mut probe).unwrap(), shared());
        }
    }

    #[test]
    fn legacy_primary_marker_and_deleted_each_require_explicit_migration() {
        for name in STORE_TRACES {
            let mut probe = MockProbe::default();
            probe.paths.insert(legacy().join(name), PathKind::File);
            assert!(default_resolve(&mut probe)
                .unwrap_err()
                .contains("calendar migration required"));
        }
    }

    #[test]
    fn each_legacy_settings_file_requires_explicit_migration() {
        for name in LEGACY_SETTINGS {
            let mut probe = MockProbe::default();
            probe.paths.insert(legacy().join(name), PathKind::File);
            assert!(default_resolve(&mut probe)
                .unwrap_err()
                .contains("calendar migration required"));
        }
    }

    #[test]
    fn legacy_google_sync_state_alone_requires_migration() {
        for name in [
            "google-sync-synthetic.json",
            "GOOGLE-SYNC-SYNTHETIC.JSON",
            "Google-Sync-Synthetic.Json",
        ] {
            let mut probe = MockProbe::default();
            probe.paths.insert(legacy(), PathKind::Directory);
            probe.paths.insert(legacy().join(name), PathKind::File);
            assert!(default_resolve(&mut probe)
                .unwrap_err()
                .contains("calendar migration required"));
        }
    }

    #[test]
    fn shared_or_legacy_probe_errors_never_become_empty_success() {
        for path in [
            shared(),
            shared().join("calendar.json"),
            legacy(),
            legacy().join("calendar.json"),
            legacy().join("widget.json"),
        ] {
            let mut probe = MockProbe {
                error_at: Some(path),
                ..MockProbe::default()
            };
            assert_eq!(
                default_resolve(&mut probe).unwrap_err(),
                "synthetic read access denied"
            );
        }
    }

    #[test]
    fn legacy_directory_iteration_error_never_becomes_empty_success() {
        let mut probe = MockProbe {
            names_error: true,
            ..MockProbe::default()
        };
        probe.paths.insert(legacy(), PathKind::Directory);
        assert_eq!(
            default_resolve(&mut probe).unwrap_err(),
            "synthetic directory read failed"
        );
    }

    #[test]
    fn shared_reparse_or_file_directory_is_rejected() {
        for kind in [PathKind::File, PathKind::Other] {
            let mut probe = MockProbe::default();
            probe.paths.insert(shared(), kind);
            assert!(default_resolve(&mut probe)
                .unwrap_err()
                .contains("not a regular directory"));
        }
    }

    #[test]
    fn missing_or_relative_local_path_cannot_prove_a_virgin_install() {
        for local in [None, Some(OsStr::new("")), Some(OsStr::new("relative"))] {
            let mut probe = MockProbe::default();
            assert!(resolve(None, Some(OsStr::new(PROFILE)), local, &mut probe).is_err());
        }
    }

    #[test]
    fn prepared_shared_store_does_not_require_local_app_data() {
        let mut probe = MockProbe::default();
        probe
            .paths
            .insert(shared().join("calendar.initialized"), PathKind::File);
        assert_eq!(
            resolve(None, Some(OsStr::new(PROFILE)), None, &mut probe).unwrap(),
            shared()
        );
    }

    #[test]
    fn different_local_app_data_contexts_resolve_the_same_shared_directory() {
        for local in [
            LOCAL,
            r"C:\Users\Synthetic\AppData\Local\Packages\Synthetic\LocalCache\Local",
        ] {
            let mut probe = MockProbe::default();
            assert_eq!(
                resolve(
                    None,
                    Some(OsStr::new(PROFILE)),
                    Some(OsStr::new(local)),
                    &mut probe
                )
                .unwrap(),
                shared()
            );
        }
    }
}
