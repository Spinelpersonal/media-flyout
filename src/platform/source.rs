use windows::{
    Win32::{
        System::Com::CoTaskMemFree,
        UI::Shell::{
            FOLDERID_AppsFolder, IShellItem, KF_FLAG_DEFAULT, SHCreateItemInKnownFolder,
            SIGDN_NORMALDISPLAY,
        },
    },
    core::HSTRING,
};

/// Resolve an AUMID using Windows' installed application names. The caller must
/// initialize COM; the media worker does so before discovering sessions.
pub(crate) fn app_display_name(app_id: &str) -> Option<String> {
    if app_id.is_empty() || app_id.contains('\0') {
        return None;
    }
    let app_id = HSTRING::from(app_id);
    // SAFETY: COM is initialized on the media worker. The input is a terminated
    // UTF-16 string. The shell item is released here, and its allocated name is
    // copied before being freed with the allocator required by GetDisplayName.
    unsafe {
        let item: IShellItem =
            SHCreateItemInKnownFolder(&FOLDERID_AppsFolder, KF_FLAG_DEFAULT, &app_id).ok()?;
        let name = item.GetDisplayName(SIGDN_NORMALDISPLAY).ok()?;
        let result = name.to_string().ok();
        CoTaskMemFree(Some(name.0.cast()));
        result
    }
}
