//! Optional local collector worker. An absent configuration has no background
//! thread/process/timer, and ordinary CLI/MCP calls never launch collection.
use serde::Deserialize;
use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::{
        atomic::{AtomicBool, AtomicI64, Ordering},
        OnceLock,
    },
    time::{SystemTime, UNIX_EPOCH},
};

static CONFIG: OnceLock<Result<Option<(Config, PathBuf)>, String>> = OnceLock::new();
static RUNNING: AtomicBool = AtomicBool::new(false);
static LAST_START: AtomicI64 = AtomicI64::new(-1);

fn epoch() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
fn korean_day(seconds: i64) -> i64 {
    (seconds + 9 * 3600).div_euclid(86400)
}
fn due(now: i64, previous: i64) -> bool {
    previous < 0 || korean_day(now) > korean_day(previous) || now.saturating_sub(previous) >= 3600
}
fn until_korean_midnight(seconds: i64) -> u32 {
    ((86400 - (seconds + 9 * 3600).rem_euclid(86400)) * 1000 + 250) as u32
}

pub fn next_day_millis() -> Option<u32> {
    matches!(CONFIG.get(), Some(Ok(Some(_)))).then(|| until_korean_midnight(epoch()))
}

fn safe_path(path: &Path) -> Result<(), String> {
    use std::os::windows::fs::MetadataExt;
    for ancestor in path.ancestors() {
        match std::fs::symlink_metadata(ancestor) {
            Ok(m) if m.file_attributes() & 0x400 != 0 => {
                return Err("공모전 실행 경로가 다른 위치로 연결되어 있습니다.".into())
            }
            Ok(_) => (),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    schema_version: u32,
    enabled: bool,
    powershell_path: String,
    runner_path: String,
    helper_path: String,
}

fn read_config(bytes: &[u8]) -> Result<Option<Config>, String> {
    if bytes.len() > 8192 {
        return Err("공모전 시작 설정이 너무 큽니다.".into());
    }
    let value: Config = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    if value.schema_version != 1 {
        return Err("지원하지 않는 공모전 시작 설정입니다.".into());
    }
    if !value.enabled {
        return Ok(None);
    }
    for path in [
        &value.powershell_path,
        &value.runner_path,
        &value.helper_path,
    ] {
        if !Path::new(path).is_absolute() || path.contains('\0') {
            return Err("공모전 실행 경로는 명시적인 절대 경로여야 합니다.".into());
        }
    }
    Ok(Some(value))
}

pub fn start() -> Result<(), String> {
    // Existing GUI events call this function; no polling thread is retained.
    let runtime = CONFIG.get_or_init(|| {
        use std::io::Read;
        let directory = crate::data_paths::directory()?;
        let config_path = directory.join("contest-startup.json");
        safe_path(&config_path)?;
        let file = match std::fs::File::open(config_path) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.to_string()),
        };
        let mut bytes = Vec::new();
        file.take(8193)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        let Some(config) = read_config(&bytes)? else {
            return Ok(None);
        };
        for path in [
            &config.powershell_path,
            &config.runner_path,
            &config.helper_path,
        ] {
            safe_path(Path::new(path))?;
            if !Path::new(path).is_file() {
                return Err(format!("공모전 실행 파일이 없습니다: {path}"));
            }
        }
        Ok(Some((config, directory)))
    });
    let Some((config, directory)) = runtime.as_ref().map_err(Clone::clone)? else {
        return Ok(());
    };
    let now = epoch();
    if !due(now, LAST_START.load(Ordering::Relaxed)) || RUNNING.swap(true, Ordering::AcqRel) {
        return Ok(());
    }
    LAST_START.store(now, Ordering::Relaxed);
    std::thread::spawn(move || {
        let result = (|| -> Result<(), String> {
            use std::os::windows::process::CommandExt;
            let mut child = Command::new(&config.powershell_path)
                .args(["-NoLogo", "-NoProfile", "-NonInteractive", "-File"])
                .arg(&config.runner_path)
                .arg("-HelperPath")
                .arg(&config.helper_path)
                .arg("-ExpectedDataDirectory")
                .arg(directory)
                .args(["-Mode", "Apply", "-WatchProcessId"])
                .arg(std::process::id().to_string())
                .creation_flags(0x0800_0000) // CREATE_NO_WINDOW, no policy override.
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .map_err(|e| e.to_string())?;
            // A finite active task waits only for bounded retries; a successful
            // day exits immediately. Cross-process daily.lock remains authoritative.
            let status = child.wait().map_err(|e| e.to_string())?;
            if !status.success() {
                return Err(format!(
                    "공모전 수집 작업이 오류로 종료되었습니다: {status}"
                ));
            }
            Ok(())
        })();
        if let Err(error) = result {
            // Unique diagnostic only: never replace an existing user file.
            use std::io::Write;
            if safe_path(directory).is_ok() {
                let path = directory.join(format!(
                    "contest-startup-error-{}-{}.txt",
                    std::process::id(),
                    epoch()
                ));
                if let Ok(mut file) = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(path)
                {
                    let _ = file
                        .write_all(error.as_bytes())
                        .and_then(|_| file.sync_all());
                }
            }
        }
        RUNNING.store(false, Ordering::Release);
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_trigger_uses_korean_day_and_bounds_same_day_relaunches() {
        assert_eq!(korean_day(15 * 3600 - 1), 0);
        assert_eq!(korean_day(15 * 3600), 1);
        assert!(due(15 * 3600, 15 * 3600 - 1));
        assert!(!due(300, 0));
        assert!(due(3600, 0));
        assert!(!due(0, 3600));
        assert_eq!(until_korean_midnight(15 * 3600 - 1), 1250);
        assert_eq!(until_korean_midnight(15 * 3600), 86_400_250);
    }

    #[test]
    fn disabled_config_never_requires_or_launches_a_command() {
        let config = br#"{"schema_version":1,"enabled":false,"powershell_path":"","runner_path":"","helper_path":""}"#;
        assert!(read_config(config).unwrap().is_none());
    }

    #[test]
    fn active_config_requires_exact_absolute_paths_and_known_schema() {
        let mut value = serde_json::json!({
            "schema_version": 1, "enabled": true,
            "powershell_path": "C:\\Program Files\\PowerShell\\7\\pwsh.exe",
            "runner_path": "C:\\Calendar\\scripts\\run-contest-startup.ps1",
            "helper_path": "C:\\Profile\\.agents\\skills\\dot-calendar\\scripts\\invoke-calendar.ps1"
        });
        assert!(read_config(&serde_json::to_vec(&value).unwrap())
            .unwrap()
            .is_some());
        value["runner_path"] = "relative-script.ps1".into();
        assert!(read_config(&serde_json::to_vec(&value).unwrap()).is_err());
        value["enabled"] = false.into();
        value["schema_version"] = 2.into();
        assert!(read_config(&serde_json::to_vec(&value).unwrap()).is_err());
        assert!(read_config(&vec![b' '; 8193]).is_err());
    }
}
