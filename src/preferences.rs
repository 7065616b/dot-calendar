use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::PathBuf};

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    pub position: Option<[i32; 4]>,
    pub opacity: u8,
    pub dark: bool,
    pub desktop: bool,
    pub locked: bool,
    pub lunar: bool,
    pub holidays: bool,
    pub show_google: bool,
    pub auto_start: bool,
    pub sync_minutes: u32,
    pub day_colors: BTreeMap<String, String>,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            position: None,
            opacity: 204,
            dark: true,
            desktop: true,
            locked: false,
            lunar: true,
            holidays: true,
            show_google: true,
            auto_start: false,
            sync_minutes: 5,
            day_colors: BTreeMap::new(),
        }
    }
}
pub fn directory() -> PathBuf {
    std::env::var_os("DOT_CALENDAR_DATA_DIR")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap_or_default()).join("DotCalendar")
        })
}
impl Preferences {
    pub fn load() -> Self {
        let path = directory().join("widget.json");
        if let Ok(bytes) = std::fs::read(path) {
            if let Ok(mut value) = serde_json::from_slice::<Self>(&bytes) {
                value.opacity = value.opacity.clamp(102, 255);
                value.sync_minutes = value.sync_minutes.clamp(1, 60);
                return value;
            }
        }
        Self::default()
    }
    pub fn save(&self) -> Result<(), String> {
        let dir = directory();
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let bytes = serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?;
        let temp = dir.join("widget.json.tmp");
        std::fs::write(&temp, bytes).map_err(|e| e.to_string())?;
        use std::os::windows::ffi::OsStrExt;
        let wide = |p: &std::path::Path| {
            p.as_os_str()
                .encode_wide()
                .chain(Some(0))
                .collect::<Vec<_>>()
        };
        let path = dir.join("widget.json");
        let ok = unsafe {
            windows_sys::Win32::Storage::FileSystem::MoveFileExW(
                wide(&temp).as_ptr(),
                wide(&path).as_ptr(),
                9,
            )
        };
        if ok == 0 {
            Err(std::io::Error::last_os_error().to_string())
        } else {
            Ok(())
        }
    }
}

pub fn set_startup(enabled: bool) -> Result<(), String> {
    use windows_sys::Win32::Foundation::*;
    use windows_sys::Win32::System::Registry::*;
    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
    let value = if enabled {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        Some(wide(&format!("\"{}\"", exe.display())))
    } else {
        None
    };
    unsafe {
        let mut key = std::ptr::null_mut();
        let status = RegCreateKeyExW(
            HKEY_CURRENT_USER,
            wide("Software\\Microsoft\\Windows\\CurrentVersion\\Run").as_ptr(),
            0,
            std::ptr::null(),
            0,
            KEY_SET_VALUE,
            std::ptr::null(),
            &mut key,
            std::ptr::null_mut(),
        );
        if status != ERROR_SUCCESS {
            return Err(format!("자동 시작 설정 실패: {status}"));
        }
        let name = wide("DotCalendar");
        let result = if let Some(value) = &value {
            RegSetValueExW(
                key,
                name.as_ptr(),
                0,
                REG_SZ,
                value.as_ptr() as *const u8,
                (value.len() * 2) as u32,
            )
        } else {
            RegDeleteValueW(key, name.as_ptr())
        };
        RegCloseKey(key);
        if result == ERROR_SUCCESS || !enabled && result == ERROR_FILE_NOT_FOUND {
            Ok(())
        } else {
            Err(format!("자동 시작 설정 실패: {result}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn google_visibility_defaults_on_and_round_trips_off() {
        let mut prefs: Preferences = serde_json::from_str(r#"{"opacity":235}"#).unwrap();
        assert!(prefs.show_google);
        prefs.show_google = false;
        let restored: Preferences =
            serde_json::from_slice(&serde_json::to_vec(&prefs).unwrap()).unwrap();
        assert!(!restored.show_google);
        assert_eq!(restored.opacity, 235);
    }
}
