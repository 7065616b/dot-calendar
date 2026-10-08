use serde::{Deserialize, Serialize};
use std::{fs, path::Path};
use windows_sys::Win32::{
    Foundation::*,
    System::{DataExchange::*, Memory::*},
    UI::{Controls::*, WindowsAndMessaging::*},
};

pub const INTRO: &str = "내 기본 캘린더는 연결된 PC의 Dot Calendar야. 앞으로 캘린더 요청은 여기에 반영하고, 작업 중 완료 예정일이나 마감일이 나오면 추가할지 먼저 물어봐. 이 설정을 기억하고 오늘 일정을 확인해줘.";
const FILES: [(&str, &str); 3] = [
    (
        "SKILL.md",
        include_str!("../integration/dot-calendar/SKILL.md"),
    ),
    (
        "agents/openai.yaml",
        include_str!("../integration/dot-calendar/agents/openai.yaml"),
    ),
    (
        "scripts/invoke-calendar.ps1",
        include_str!("../integration/dot-calendar/scripts/invoke-calendar.ps1"),
    ),
];

#[derive(Serialize, Deserialize)]
struct Connection {
    executable_path: std::path::PathBuf,
    data_directory: std::path::PathBuf,
}

fn wide(value: &std::ffi::OsStr) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    value.encode_wide().chain(Some(0)).collect()
}

fn user_directory() -> Result<std::path::PathBuf, String> {
    let home = std::env::var_os("USERPROFILE").ok_or("Windows 사용자 폴더를 찾을 수 없습니다.")?;
    let home = std::path::PathBuf::from(home);
    if !home.is_absolute() {
        return Err("사용자 폴더의 경로가 올바르지 않습니다.".into());
    }
    Ok(home.join(".agents/skills/dot-calendar"))
}

// Do not traverse redirected skill files or folders while installing/removing.
fn check_path(path: &Path) -> Result<(), String> {
    use std::os::windows::fs::MetadataExt;
    for ancestor in path.ancestors() {
        match fs::symlink_metadata(ancestor) {
            Ok(metadata) if metadata.file_attributes() & 0x400 != 0 => {
                return Err(format!(
                    "연결 파일 경로가 다른 위치로 연결되어 있습니다: {}",
                    ancestor.display()
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    Ok(())
}

fn write_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    check_path(path)?;
    let existing = match fs::read(path) {
        Ok(existing) if existing == bytes => return Ok(()),
        Ok(existing) => Some(existing),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.to_string()),
    };
    let parent = path.parent().ok_or("연결 파일 경로가 올바르지 않습니다.")?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let mut nonce = [0u8; 8];
    getrandom::getrandom(&mut nonce).map_err(|error| error.to_string())?;
    let temp = parent.join(format!(
        ".dot-calendar-{:016x}.tmp",
        u64::from_le_bytes(nonce)
    ));
    use std::io::Write;
    if let Some(existing) = existing {
        // Preserve earlier/customized integration files before refreshing them.
        // The .backup suffix prevents discovery as another SKILL.md.
        let backup = parent.join(format!(
            "{}.{:016x}.backup",
            path.file_name()
                .ok_or("연결 파일 이름이 없습니다.")?
                .to_string_lossy(),
            u64::from_le_bytes(nonce)
        ));
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(backup)
            .map_err(|error| error.to_string())?;
        file.write_all(&existing)
            .and_then(|_| file.sync_all())
            .map_err(|error| error.to_string())?;
    }
    let result = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .map_err(|error| error.to_string())?;
        file.write_all(bytes)
            .and_then(|_| file.sync_all())
            .map_err(|error| error.to_string())?;
        drop(file);
        let ok = unsafe {
            windows_sys::Win32::Storage::FileSystem::MoveFileExW(
                wide(temp.as_os_str()).as_ptr(),
                wide(path.as_os_str()).as_ptr(),
                9,
            )
        };
        if ok == 0 {
            Err(std::io::Error::last_os_error().to_string())
        } else {
            Ok(())
        }
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}

fn install_to(directory: &Path, executable: &Path, data: &Path) -> Result<(), String> {
    if !directory.is_absolute()
        || !executable.is_absolute()
        || !data.is_absolute()
        || !executable.is_file()
    {
        return Err("달력 연결에 필요한 절대 경로를 확인할 수 없습니다.".into());
    }
    for (name, _) in FILES {
        check_path(&directory.join(name))?;
    }
    check_path(&directory.join("connection.json"))?;
    for (name, contents) in FILES {
        write_file(&directory.join(name), contents.as_bytes())?;
    }
    let connection = Connection {
        executable_path: executable.into(),
        data_directory: data.into(),
    };
    let bytes = serde_json::to_vec_pretty(&connection).map_err(|error| error.to_string())?;
    write_file(&directory.join("connection.json"), &bytes)
}

pub fn install() -> Result<(), String> {
    install_to(
        &user_directory()?,
        &std::env::current_exe().map_err(|error| error.to_string())?,
        &crate::preferences::directory(),
    )
}

fn remove_from(directory: &Path, executable: &Path) -> Result<(), String> {
    let config = directory.join("connection.json");
    check_path(&config)?;
    let bytes = match fs::read(&config) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.to_string()),
    };
    let connection: Connection =
        serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    if !connection
        .executable_path
        .as_os_str()
        .to_string_lossy()
        .eq_ignore_ascii_case(&executable.as_os_str().to_string_lossy())
    {
        return Ok(()); // A newer installation now owns the connection.
    }
    for (name, contents) in FILES {
        let path = directory.join(name);
        check_path(&path)?;
        if fs::read(&path).is_ok_and(|bytes| bytes == contents.as_bytes()) {
            fs::remove_file(path).map_err(|error| error.to_string())?;
        }
    }
    fs::remove_file(config).map_err(|error| error.to_string())?;
    for folder in [
        directory.join("agents"),
        directory.join("scripts"),
        directory.to_path_buf(),
    ] {
        let _ = fs::remove_dir(folder); // Empty app-owned folders only; preserve customized/extra files.
    }
    Ok(())
}

pub fn remove() -> Result<(), String> {
    remove_from(
        &user_directory()?,
        &std::env::current_exe().map_err(|error| error.to_string())?,
    )
}

unsafe fn copy_intro(owner: HWND) -> Result<(), String> {
    let value = wide(std::ffi::OsStr::new(INTRO));
    let memory = GlobalAlloc(GMEM_MOVEABLE, value.len() * 2);
    if memory.is_null() {
        return Err("복사할 메모리를 준비할 수 없습니다.".into());
    }
    let destination = GlobalLock(memory) as *mut u16;
    if destination.is_null() {
        GlobalFree(memory);
        return Err("복사할 메모리를 열 수 없습니다.".into());
    }
    std::ptr::copy_nonoverlapping(value.as_ptr(), destination, value.len());
    GlobalUnlock(memory);
    if OpenClipboard(owner) == 0 {
        GlobalFree(memory);
        return Err("클립보드를 사용 중입니다. 잠시 후 다시 눌러 주세요.".into());
    }
    let success = EmptyClipboard() != 0 && !SetClipboardData(13, memory).is_null();
    CloseClipboard();
    if success {
        Ok(())
    } else {
        GlobalFree(memory);
        Err("문장을 복사하지 못했습니다.".into())
    }
}

pub unsafe fn show(owner: HWND) -> Result<(), String> {
    install()?;
    let strings: Vec<Vec<u16>> = [
        "Dot Calendar · 닷과 함께",
        "이 PC의 연결 준비가 끝났어요",
        "처음 한 번, 아래 문장을 닷에게 보내 주세요.\n기본 캘린더와 일정 제안 방식을 기억하도록 요청합니다.\n\n그다음에는 짧게 말하세요.\n“내일 3시 회의 추가해줘”\n“이 내용을 캘린더에 반영해줘”\n“오늘 일정 알려줘”\n\n작업에 마감일이 나오면 닷이 추가할지 먼저 묻습니다.",
        "닷에 보낼 문장 복사",
        "닫기",
        "닷의 ‘Your computer’ 연결과 온라인 PC가 필요합니다.\n이 표시는 PC의 연결 준비 상태이며 닷 계정의 응답 확인은 별도입니다.",
        INTRO,
        "한 번 보낼 문장 보기",
        "문장 접기",
    ].iter().map(|value| wide(std::ffi::OsStr::new(value))).collect();
    let buttons = [
        TASKDIALOG_BUTTON {
            nButtonID: 100,
            pszButtonText: strings[3].as_ptr(),
        },
        TASKDIALOG_BUTTON {
            nButtonID: IDCANCEL,
            pszButtonText: strings[4].as_ptr(),
        },
    ];
    let config = TASKDIALOGCONFIG {
        cbSize: std::mem::size_of::<TASKDIALOGCONFIG>() as u32,
        hwndParent: owner,
        dwFlags: TDF_ALLOW_DIALOG_CANCELLATION,
        pszWindowTitle: strings[0].as_ptr(),
        pszMainInstruction: strings[1].as_ptr(),
        pszContent: strings[2].as_ptr(),
        cButtons: buttons.len() as u32,
        pButtons: buttons.as_ptr(),
        nDefaultButton: 100,
        pszFooter: strings[5].as_ptr(),
        pszExpandedInformation: strings[6].as_ptr(),
        pszExpandedControlText: strings[7].as_ptr(),
        pszCollapsedControlText: strings[8].as_ptr(),
        cxWidth: 330,
        ..std::mem::zeroed()
    };
    let mut chosen = 0;
    let result = TaskDialogIndirect(
        &config,
        &mut chosen,
        std::ptr::null_mut(),
        std::ptr::null_mut(),
    );
    if result < 0 {
        return Err(format!("닷 연결 화면을 열 수 없습니다 ({result:#x})."));
    }
    if chosen == 100 {
        copy_intro(owner)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn install_refresh_and_remove_preserve_data_and_other_files() {
        let mut nonce = [0u8; 8];
        getrandom::getrandom(&mut nonce).unwrap();
        let root = std::env::temp_dir().join(format!(
            "dot-connect-test-{:016x}",
            u64::from_le_bytes(nonce)
        ));
        fs::create_dir(&root).unwrap();
        let directory = root.join("skill");
        let data = root.join("calendar");
        fs::create_dir_all(&data).unwrap();
        fs::write(data.join("calendar.json"), b"preserve").unwrap();
        let exe = std::env::current_exe().unwrap();
        install_to(&directory, &exe, &data).unwrap();
        install_to(&directory, &exe, &data).unwrap();
        let config: Connection =
            serde_json::from_slice(&fs::read(directory.join("connection.json")).unwrap()).unwrap();
        assert_eq!(config.data_directory, data);
        fs::write(directory.join("SKILL.md"), b"previous customization").unwrap();
        install_to(&directory, &exe, &data).unwrap();
        let preserved = fs::read_dir(&directory)
            .unwrap()
            .filter_map(Result::ok)
            .any(|file| {
                file.path().extension().is_some_and(|ext| ext == "backup")
                    && fs::read(file.path()).is_ok_and(|bytes| bytes == b"previous customization")
            });
        assert!(preserved);
        fs::write(directory.join("custom.txt"), b"preserve").unwrap();
        remove_from(&directory, &root.join("other.exe")).unwrap();
        assert!(directory.join("connection.json").exists());
        fs::write(directory.join("SKILL.md"), b"user customization").unwrap();
        remove_from(&directory, &exe).unwrap();
        assert!(!directory.join("connection.json").exists());
        assert_eq!(
            fs::read(directory.join("SKILL.md")).unwrap(),
            b"user customization"
        );
        assert_eq!(fs::read(directory.join("custom.txt")).unwrap(), b"preserve");
        assert_eq!(fs::read(data.join("calendar.json")).unwrap(), b"preserve");
        let root = root.canonicalize().unwrap();
        assert!(root.starts_with(std::env::temp_dir().canonicalize().unwrap()));
        assert!(root
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("dot-connect-test-"));
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn install_rejects_relative_data_path_without_writing() {
        let root = std::env::temp_dir().join("dot-connect-invalid-unused");
        assert!(install_to(
            &root,
            &std::env::current_exe().unwrap(),
            Path::new("relative")
        )
        .is_err());
    }
}
