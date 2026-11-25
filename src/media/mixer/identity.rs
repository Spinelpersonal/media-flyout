use std::path::{Path, PathBuf};
use windows::{
    Win32::{
        Foundation::{
            APPMODEL_ERROR_NO_APPLICATION, APPMODEL_ERROR_NO_PACKAGE, ERROR_NO_MORE_FILES,
            ERROR_NOT_FOUND, HANDLE, TYPE_E_ELEMENTNOTFOUND,
        },
        Storage::{
            EnhancedStorage::{
                PKEY_AppUserModel_ID, PKEY_Link_Arguments, PKEY_Link_TargetParsingPath,
            },
            Packaging::Appx::GetApplicationUserModelId,
        },
        System::{Com::CoTaskMemFree, Diagnostics::ToolHelp::*, Threading::*},
        UI::Shell::{FOLDERID_AppsFolder, IShellItem2, KF_FLAG_DEFAULT, SHCreateItemInKnownFolder},
    },
    core::{HSTRING, Owned, PWSTR},
};

/// The caller transfers one null-terminated CoTaskMem string.
pub(super) unsafe fn take_string(value: PWSTR) -> Option<String> {
    // SAFETY: Copy while the COM allocation is live, then free it even on a UTF-16 error.
    unsafe {
        let result = value.to_string().ok();
        CoTaskMemFree(Some(value.0.cast()));
        result
    }
}

pub(super) struct Source<'a> {
    id: &'a str,
    executable: Option<PathBuf>,
}

impl<'a> Source<'a> {
    pub(super) fn new(id: &'a str) -> Self {
        let executable = registered_executable(id).or_else(|| {
            let path = Path::new(id);
            if !is_executable(path) {
                return None;
            }
            if path.is_absolute() {
                return Some(path.into());
            }
            if path.file_name()? != path.as_os_str() {
                return None;
            }
            unique_running_image(id)
        });
        Self { id, executable }
    }

    pub(super) fn owns(&self, pid: u32) -> bool {
        // SAFETY: This read-only process handle is owned until identity queries finish.
        unsafe {
            let Ok(handle) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
                return false;
            };
            let handle = Owned::new(handle);
            let mut buffer = [0u16; 256];
            let mut len = buffer.len() as u32;
            let result =
                GetApplicationUserModelId(*handle, &mut len, Some(PWSTR(buffer.as_mut_ptr())));
            if result.is_ok() {
                let id = String::from_utf16_lossy(&buffer[..len.saturating_sub(1) as usize]);
                return self.id.eq_ignore_ascii_case(&id);
            }
            matches!(
                result,
                APPMODEL_ERROR_NO_APPLICATION | APPMODEL_ERROR_NO_PACKAGE
            ) && self.executable.as_deref().is_some_and(|path| {
                image_path(*handle).is_some_and(|image| same_path(&image, path))
            })
        }
    }
}

fn same_path(a: &Path, b: &Path) -> bool {
    a.as_os_str().eq_ignore_ascii_case(b.as_os_str())
}

fn is_executable(path: &Path) -> bool {
    // A shared runtime's image identifies the host, not the application it launched.
    const HOSTS: &[&str] = &[
        "applicationframehost",
        "dllhost",
        "dotnet",
        "electron",
        "java",
        "javaw",
        "msedgewebview2",
        "node",
        "python",
        "pythonw",
        "rundll32",
        "svchost",
        "wwahost",
    ];
    path.extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("exe"))
        && path
            .file_stem()
            .is_some_and(|stem| !HOSTS.iter().any(|host| stem.eq_ignore_ascii_case(host)))
}

fn registered_executable(id: &str) -> Option<PathBuf> {
    // Packaged identities must match the process AUMID. A shortcut with arguments
    // could launch a different app/profile through a shared host executable.
    if id.is_empty() || id.contains(['!', '\0']) || Path::new(id).is_absolute() {
        return None;
    }
    // SAFETY: The mixer worker owns its COM apartment. GetString transfers each result.
    unsafe {
        let item: IShellItem2 =
            SHCreateItemInKnownFolder(&FOLDERID_AppsFolder, KF_FLAG_DEFAULT, &HSTRING::from(id))
                .ok()?;
        if !take_string(item.GetString(&PKEY_AppUserModel_ID).ok()?)?.eq_ignore_ascii_case(id) {
            return None;
        }
        match item.GetString(&PKEY_Link_Arguments) {
            Ok(value) if take_string(value)?.is_empty() => {}
            Err(e)
                if e.code() == ERROR_NOT_FOUND.to_hresult()
                    || e.code() == TYPE_E_ELEMENTNOTFOUND => {}
            _ => return None,
        }
        let path = PathBuf::from(take_string(
            item.GetString(&PKEY_Link_TargetParsingPath).ok()?,
        )?);
        (path.is_absolute() && is_executable(&path)).then_some(path)
    }
}

fn image_path(process: HANDLE) -> Option<PathBuf> {
    let mut buffer = [0u16; 1024];
    let mut len = buffer.len() as u32;
    // SAFETY: The process handle allows limited queries and len bounds the UTF-16 output.
    unsafe {
        QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &mut len,
        )
        .ok()?;
    }
    Some(String::from_utf16(&buffer[..len as usize]).ok()?.into())
}

/// Some desktop GSMTC publishers supply only "Player.exe". Require all running
/// processes with that exact filename to resolve to a single executable path.
fn unique_running_image(name: &str) -> Option<PathBuf> {
    let mut found = None;
    // SAFETY: The snapshot and opened processes use RAII handles. The initialized
    // entry advertises its actual size, and each API copies into caller-owned memory.
    unsafe {
        let snapshot = Owned::new(CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).ok()?);
        let mut entry = PROCESSENTRY32W {
            dwSize: size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        Process32FirstW(*snapshot, &mut entry).ok()?;
        loop {
            let end = entry.szExeFile.iter().position(|&c| c == 0)?;
            if String::from_utf16_lossy(&entry.szExeFile[..end]).eq_ignore_ascii_case(name) {
                let handle = Owned::new(
                    OpenProcess(
                        PROCESS_QUERY_LIMITED_INFORMATION,
                        false,
                        entry.th32ProcessID,
                    )
                    .ok()?,
                );
                let path = image_path(*handle)?;
                if !path.file_name()?.eq_ignore_ascii_case(name)
                    || found
                        .as_deref()
                        .is_some_and(|other| !same_path(&path, other))
                {
                    return None;
                }
                found = Some(path);
            }
            if let Err(e) = Process32NextW(*snapshot, &mut entry) {
                return if e.code() == ERROR_NO_MORE_FILES.to_hresult() {
                    found
                } else {
                    None
                };
            }
        }
    }
}
