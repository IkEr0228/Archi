//! Windows Explorer cascading context menu integration (HKCU only, reversible).
//!
//! Registers "Archi" cascading submenus:
//! 1. Archives (.zip, .7z, .rar, etc.):
//!    - "Открыть в Archi" (Open in Archi)
//!    - "Извлечь сюда" (Extract here)
//!    - "Извлечь в отдельную папку" (Extract to dedicated folder)
//! 2. Files & Folders (*, Directory):
//!    - "Добавить в архив..." (Add to archive...)
//!    - "Добавить в <имя>.zip" (Add to <name>.zip)
//!    - "Добавить в <имя>.7z" (Add to <name>.7z)

use crate::file_assoc::ASSOCIATED_EXTENSIONS;
use crate::models::CommandError;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

const PROGID: &str = "Archi.Archive";
const APP_KEY: &str = r"Software\Archi";
const APP_ARCHIVE_MENU_VALUE: &str = "ArchiveContextMenuEnabled";
const APP_FILES_MENU_VALUE: &str = "FilesContextMenuEnabled";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextMenuStatus {
    pub supported: bool,
    pub archive_menu_enabled: bool,
    pub files_menu_enabled: bool,
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
fn set_app_menu_flag(name: &str, enabled: bool) -> Result<(), CommandError> {
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
    key.set_value(name, &v).map_err(|error| {
        menu_error(
            "menu_failed",
            format!("Cannot write context menu flag {name}: {error}"),
        )
    })
}

#[cfg(windows)]
fn app_menu_flag_enabled(name: &str) -> bool {
    use winreg::enums::*;
    use winreg::RegKey;
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let Ok(key) = hkcu.open_subkey(APP_KEY) else {
        return false;
    };
    let Ok(v) = key.get_value::<u32, _>(name) else {
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

#[cfg(windows)]
fn setup_files_cascade_menu(
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

    let create_cmd = format!("\"{}\" --create \"%1\"", exe_str);
    let add_zip_cmd = format!("\"{}\" --add-zip \"%1\"", exe_str);
    let add_7z_cmd = format!("\"{}\" --add-7z \"%1\"", exe_str);

    create_menu_item(
        root,
        base_path,
        "create",
        "Добавить в архив...",
        &icon,
        &create_cmd,
    )?;
    create_menu_item(
        root,
        base_path,
        "add_zip",
        "Добавить в <имя>.zip",
        &icon,
        &add_zip_cmd,
    )?;
    create_menu_item(
        root,
        base_path,
        "add_7z",
        "Добавить в <имя>.7z",
        &icon,
        &add_7z_cmd,
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
            files_menu_enabled: false,
            message: "Windows Explorer context menu is only supported on Windows.".into(),
        }
    }

    #[cfg(windows)]
    {
        use winreg::enums::*;
        use winreg::RegKey;

        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let archive_test_path = r"Software\Classes\SystemFileAssociations\.zip\shell\Archi";
        let files_test_path = r"Software\Classes\*\shell\Archi";

        let archive_key_exists = hkcu.open_subkey(archive_test_path).is_ok();
        let files_key_exists = hkcu.open_subkey(files_test_path).is_ok();

        let archive_flag = app_menu_flag_enabled(APP_ARCHIVE_MENU_VALUE);
        let files_flag = app_menu_flag_enabled(APP_FILES_MENU_VALUE);

        let archive_enabled = archive_flag && archive_key_exists;
        let files_enabled = files_flag && files_key_exists;

        let message = if archive_enabled && files_enabled {
            "Контекстное меню Archi активно для всех архивов, файлов и папок.".into()
        } else if archive_enabled {
            "Контекстное меню Archi активно для архивов.".into()
        } else if files_enabled {
            "Контекстное меню Archi активно для файлов и папок.".into()
        } else {
            "Контекстное меню Archi не настроено.".into()
        };

        ContextMenuStatus {
            supported: true,
            archive_menu_enabled: archive_enabled,
            files_menu_enabled: files_enabled,
            message,
        }
    }
}

/// Register cascading context menu for archives.
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

        set_app_menu_flag(APP_ARCHIVE_MENU_VALUE, true)?;
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

        set_app_menu_flag(APP_ARCHIVE_MENU_VALUE, false)?;
        notify_shell();
        Ok(get_context_menu_status())
    }
}

/// Register cascading context menu for regular files and folders (* and Directory).
pub fn register_files_context_menu() -> Result<ContextMenuStatus, CommandError> {
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

        setup_files_cascade_menu(&hkcu, r"Software\Classes\*\shell\Archi", &exe_str)?;
        setup_files_cascade_menu(&hkcu, r"Software\Classes\Directory\shell\Archi", &exe_str)?;

        set_app_menu_flag(APP_FILES_MENU_VALUE, true)?;
        notify_shell();
        Ok(get_context_menu_status())
    }
}

/// Remove right-click context menu for regular files and folders (* and Directory).
pub fn unregister_files_context_menu() -> Result<ContextMenuStatus, CommandError> {
    #[cfg(not(windows))]
    {
        return Err(menu_error(
            "unsupported_platform",
            "Context menu is only supported on Windows.",
        ));
    }

    #[cfg(windows)]
    {
        let _ = delete_tree(r"Software\Classes\*\shell\Archi");
        let _ = delete_tree(r"Software\Classes\Directory\shell\Archi");

        set_app_menu_flag(APP_FILES_MENU_VALUE, false)?;
        notify_shell();
        Ok(get_context_menu_status())
    }
}

/// Register both archive and file/directory context menus.
pub fn register_all_context_menus() -> Result<ContextMenuStatus, CommandError> {
    register_archive_context_menu()?;
    register_files_context_menu()
}

/// Unregister both archive and file/directory context menus.
pub fn unregister_all_context_menus() -> Result<ContextMenuStatus, CommandError> {
    unregister_archive_context_menu()?;
    unregister_files_context_menu()
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
