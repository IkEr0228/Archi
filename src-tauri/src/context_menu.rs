//! Windows Explorer cascading context menu integration (HKCU only, reversible).
//!
//! Registers "Archi" cascading submenu with:
//! - "Открыть в Archi" (Open in Archi)
//! - "Извлечь сюда" (Extract here)
//! - "Извлечь в отдельную папку" (Extract to dedicated folder)

use crate::file_assoc::ASSOCIATED_EXTENSIONS;
use crate::models::CommandError;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

const PROGID: &str = "Archi.Archive";
const APP_KEY: &str = r"Software\Archi";
const APP_ARCHIVE_MENU_VALUE: &str = "ArchiveContextMenuEnabled";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextMenuStatus {
    pub supported: bool,
    pub archive_menu_enabled: bool,
    pub message: String,
}

fn menu_error(code: &str, message: impl Into<String>) -> CommandError {
    CommandError::new(code, message)
}

#[cfg(windows)]
fn current_exe_path() -> Result<PathBuf, CommandError> {
    std::env::current_exe().map_err(|error| {
        menu_error(
            "menu_failed",
            format!("Cannot resolve application executable path: {error}"),
        )
    })
}

#[cfg(windows)]
fn notify_shell() {
    #[link(name = "shell32")]
    extern "system" {
        fn SHChangeNotify(
            event: i32,
            flags: u32,
            item1: *const std::ffi::c_void,
            item2: *const std::ffi::c_void,
        );
    }
    // SHCNE_ASSOCCHANGED = 0x08000000, SHCNF_IDLIST = 0x0000
    unsafe {
        SHChangeNotify(0x0800_0000, 0, std::ptr::null(), std::ptr::null());
    }
}

#[cfg(windows)]
fn delete_tree(key_path: &str) -> Result<(), CommandError> {
    use winreg::enums::*;
    use winreg::RegKey;
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    match hkcu.delete_subkey_all(key_path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(menu_error(
            "menu_failed",
            format!("Cannot remove registry key {key_path}: {error}"),
        )),
    }
}

#[cfg(windows)]
fn set_app_menu_flag(enabled: bool) -> Result<(), CommandError> {
    use winreg::enums::*;
    use winreg::RegKey;
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let (key, _) = hkcu.create_subkey(APP_KEY).map_err(|error| {
        menu_error(
            "menu_failed",
            format!("Cannot create app registry key: {error}"),
        )
    })?;
    let v: u32 = if enabled { 1 } else { 0 };
    key.set_value(APP_ARCHIVE_MENU_VALUE, &v).map_err(|error| {
        menu_error(
            "menu_failed",
            format!("Cannot write context menu flag: {error}"),
        )
    })
}

#[cfg(windows)]
fn app_menu_flag_enabled() -> bool {
    use winreg::enums::*;
    use winreg::RegKey;
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let Ok(key) = hkcu.open_subkey(APP_KEY) else {
        return false;
    };
    let Ok(v) = key.get_value::<u32, _>(APP_ARCHIVE_MENU_VALUE) else {
        return false;
    };
    v != 0
}

#[cfg(windows)]
fn create_menu_item(
    root: &winreg::RegKey,
    base_path: &str,
    verb: &str,
    title: &str,
    icon: &str,
    command: &str,
) -> Result<(), CommandError> {
    let item_path = format!(r"{}\shell\{}", base_path, verb);
    let (item_key, _) = root.create_subkey(&item_path).map_err(|err| {
        menu_error(
            "menu_failed",
            format!("Failed to create {item_path}: {err}"),
        )
    })?;
    let _ = item_key.set_value("MUIVerb", &title);
    let _ = item_key.set_value("Icon", &icon);

    let (cmd_key, _) = root
        .create_subkey(&format!(r"{}\command", item_path))
        .map_err(|err| {
            menu_error(
                "menu_failed",
                format!("Failed to create command for {verb}: {err}"),
            )
        })?;
    let _ = cmd_key.set_value("", &command);
    Ok(())
}

#[cfg(windows)]
fn setup_archive_cascade_menu(
    root: &winreg::RegKey,
    base_path: &str,
    exe_str: &str,
) -> Result<(), CommandError> {
    let (menu_key, _) = root.create_subkey(base_path).map_err(|err| {
        menu_error(
            "menu_failed",
            format!("Failed to create {base_path}: {err}"),
        )
    })?;

    let icon = format!("{},0", exe_str);
    let _ = menu_key.set_value("MUIVerb", &"Archi");
    let _ = menu_key.set_value("Icon", &icon);
    let _ = menu_key.set_value("SubCommands", &"");

    let open_cmd = format!("\"{}\" \"%1\"", exe_str);
    let extract_here_cmd = format!("\"{}\" --extract-here \"%1\"", exe_str);
    let extract_to_cmd = format!("\"{}\" --extract-to \"%1\"", exe_str);

    create_menu_item(root, base_path, "open", "Открыть в Archi", &icon, &open_cmd)?;
    create_menu_item(
        root,
        base_path,
        "extract_here",
        "Извлечь сюда",
        &icon,
        &extract_here_cmd,
    )?;
    create_menu_item(
        root,
        base_path,
        "extract_to",
        "Извлечь в отдельную папку",
        &icon,
        &extract_to_cmd,
    )?;

    Ok(())
}

/// Query context menu registration status.
pub fn get_context_menu_status() -> ContextMenuStatus {
    #[cfg(not(windows))]
    {
        ContextMenuStatus {
            supported: false,
            archive_menu_enabled: false,
            message: "Windows Explorer context menu is only supported on Windows.".into(),
        }
    }

    #[cfg(windows)]
    {
        use winreg::enums::*;
        use winreg::RegKey;

        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let test_path = r"Software\Classes\SystemFileAssociations\.zip\shell\Archi";
        let key_exists = hkcu.open_subkey(test_path).is_ok();
        let flag = app_menu_flag_enabled();
        let enabled = flag && key_exists;

        let message = if enabled {
            "Контекстное меню Archi активно для архивов.".into()
        } else {
            "Контекстное меню Archi не настроено.".into()
        };

        ContextMenuStatus {
            supported: true,
            archive_menu_enabled: enabled,
            message,
        }
    }
}

/// Register cascading right-click context menu for all supported archive formats.
pub fn register_archive_context_menu() -> Result<ContextMenuStatus, CommandError> {
    #[cfg(not(windows))]
    {
        return Err(menu_error(
            "unsupported_platform",
            "Context menu is only supported on Windows.",
        ));
    }

    #[cfg(windows)]
    {
        use winreg::enums::*;
        use winreg::RegKey;

        let exe = current_exe_path()?;
        let exe_str = exe.to_string_lossy();
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);

        for ext in ASSOCIATED_EXTENSIONS {
            let base_path = format!(
                r"Software\Classes\SystemFileAssociations\.{}\shell\Archi",
                ext
            );
            setup_archive_cascade_menu(&hkcu, &base_path, &exe_str)?;
        }

        let progid_path = format!(r"Software\Classes\{}\shell\Archi", PROGID);
        setup_archive_cascade_menu(&hkcu, &progid_path, &exe_str)?;

        set_app_menu_flag(true)?;
        notify_shell();
        Ok(get_context_menu_status())
    }
}

/// Remove right-click context menu for all archive formats.
pub fn unregister_archive_context_menu() -> Result<ContextMenuStatus, CommandError> {
    #[cfg(not(windows))]
    {
        return Err(menu_error(
            "unsupported_platform",
            "Context menu is only supported on Windows.",
        ));
    }

    #[cfg(windows)]
    {
        for ext in ASSOCIATED_EXTENSIONS {
            let base_path = format!(
                r"Software\Classes\SystemFileAssociations\.{}\shell\Archi",
                ext
            );
            let _ = delete_tree(&base_path);
        }

        let progid_path = format!(r"Software\Classes\{}\shell\Archi", PROGID);
        let _ = delete_tree(&progid_path);

        set_app_menu_flag(false)?;
        notify_shell();
        Ok(get_context_menu_status())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_context_menu_status_reports_platform() {
        let status = get_context_menu_status();
        #[cfg(windows)]
        assert!(status.supported);
        #[cfg(not(windows))]
        assert!(!status.supported);
    }
}
