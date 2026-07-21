//! Opt-in Windows Explorer file associations (HKCU only, reversible).
//!
//! Does **not** write HKLM / machine-wide defaults. Unregister only removes
//! ProgIds we own and extension defaults that still point at Archi.

use crate::models::CommandError;
use serde::Serialize;
use std::path::PathBuf;

/// Extensions Archi can open (Windows associates the final segment; multi-dot
/// names like `.tar.gz` are covered by `.gz` + content detection).
pub const ASSOCIATED_EXTENSIONS: &[&str] = &[
    "zip", "tar", "gz", "tgz", "bz2", "tbz2", "tbz", "xz", "txz", "7z",
];

const PROGID: &str = "Archi.Archive";
const PROGID_DESC: &str = "Archi Archive";
const APP_KEY: &str = r"Software\Archi";
const APP_ASSOC_VALUE: &str = "FileAssociationsEnabled";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileAssociationStatus {
    /// Platform supports associations (Windows).
    pub supported: bool,
    /// User opted in and ProgId + open command look correct.
    pub enabled: bool,
    /// Extensions currently defaulting to our ProgId.
    pub associated_extensions: Vec<String>,
    /// Absolute path used in the open command, if registered.
    pub exe_path: Option<String>,
    pub message: String,
}

fn assoc_error(code: &str, message: impl Into<String>) -> CommandError {
    CommandError::new(code, message)
}

/// Quote a Windows path for use inside `"path" "%1"`.
pub fn quote_windows_path(path: &str) -> String {
    format!("\"{}\"", path.replace('"', ""))
}

pub fn build_open_command(exe: &str) -> String {
    format!("{} \"%1\"", quote_windows_path(exe))
}

#[cfg(windows)]
fn current_exe_path() -> Result<PathBuf, CommandError> {
    std::env::current_exe().map_err(|error| {
        assoc_error(
            "assoc_failed",
            format!("Cannot resolve application path: {error}"),
        )
    })
}

#[cfg(windows)]
fn notify_shell() {
    #[link(name = "shell32")]
    extern "system" {
        fn SHChangeNotify(event: i32, flags: u32, item1: isize, item2: isize);
    }
    // SHCNE_ASSOCCHANGED = 0x08000000, SHCNF_IDLIST = 0x0000
    unsafe {
        SHChangeNotify(0x0800_0000, 0, 0, 0);
    }
}

#[cfg(windows)]
fn read_default(key_path: &str) -> Option<String> {
    use winreg::enums::*;
    use winreg::RegKey;
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let key = hkcu.open_subkey(key_path).ok()?;
    key.get_value::<String, _>("").ok()
}

#[cfg(windows)]
fn write_default(key_path: &str, value: &str) -> Result<(), CommandError> {
    use winreg::enums::*;
    use winreg::RegKey;
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let (key, _) = hkcu.create_subkey(key_path).map_err(|error| {
        assoc_error(
            "assoc_failed",
            format!("Cannot create registry key {key_path}: {error}"),
        )
    })?;
    key.set_value("", &value).map_err(|error| {
        assoc_error(
            "assoc_failed",
            format!("Cannot write registry value {key_path}: {error}"),
        )
    })
}

#[cfg(windows)]
fn delete_tree(key_path: &str) -> Result<(), CommandError> {
    use winreg::enums::*;
    use winreg::RegKey;
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    match hkcu.delete_subkey_all(key_path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(assoc_error(
            "assoc_failed",
            format!("Cannot remove registry key {key_path}: {error}"),
        )),
    }
}

#[cfg(windows)]
fn set_app_flag(enabled: bool) -> Result<(), CommandError> {
    use winreg::enums::*;
    use winreg::RegKey;
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let (key, _) = hkcu.create_subkey(APP_KEY).map_err(|error| {
        assoc_error(
            "assoc_failed",
            format!("Cannot create app registry key: {error}"),
        )
    })?;
    let v: u32 = if enabled { 1 } else { 0 };
    key.set_value(APP_ASSOC_VALUE, &v).map_err(|error| {
        assoc_error(
            "assoc_failed",
            format!("Cannot write association flag: {error}"),
        )
    })
}

#[cfg(windows)]
fn app_flag_enabled() -> bool {
    use winreg::enums::*;
    use winreg::RegKey;
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let Ok(key) = hkcu.open_subkey(APP_KEY) else {
        return false;
    };
    let Ok(v) = key.get_value::<u32, _>(APP_ASSOC_VALUE) else {
        return false;
    };
    v != 0
}

/// Query current association state (Windows: HKCU; other OS: unsupported).
pub fn get_file_association_status() -> FileAssociationStatus {
    #[cfg(not(windows))]
    {
        return FileAssociationStatus {
            supported: false,
            enabled: false,
            associated_extensions: Vec::new(),
            exe_path: None,
            message: "File associations are only available on Windows.".into(),
        };
    }

    #[cfg(windows)]
    {
        let mut associated = Vec::new();
        for ext in ASSOCIATED_EXTENSIONS {
            let path = format!(r"Software\Classes\.{}", ext);
            if read_default(&path).as_deref() == Some(PROGID) {
                associated.push((*ext).to_string());
            }
        }

        let command = read_default(&format!(r"Software\Classes\{}\shell\open\command", PROGID));
        let exe_from_cmd = command.as_ref().and_then(|cmd| {
            // Expect `"C:\path\archi.exe" "%1"`
            let trimmed = cmd.trim();
            if let Some(rest) = trimmed.strip_prefix('"') {
                rest.split('"').next().map(|s| s.to_string())
            } else {
                None
            }
        });

        let current_exe = current_exe_path()
            .ok()
            .map(|p| p.to_string_lossy().into_owned());
        let command_matches = match (&exe_from_cmd, &current_exe) {
            (Some(reg), Some(cur)) => {
                reg.eq_ignore_ascii_case(cur)
                    || std::fs::canonicalize(reg)
                        .ok()
                        .zip(std::fs::canonicalize(cur).ok())
                        .map(|(a, b)| a == b)
                        .unwrap_or(false)
            }
            _ => false,
        };

        let progid_ok = command.is_some() && command_matches;
        let flag = app_flag_enabled();
        let enabled = flag && progid_ok && !associated.is_empty();

        let message = if enabled {
            format!(
                "Archi is associated with {} extension(s) for this user.",
                associated.len()
            )
        } else if !associated.is_empty() || command.is_some() {
            "Partial or stale associations detected. Re-enable to repair, or disable to clear."
                .into()
        } else {
            "Not associated. Enable to open archives from Explorer with Archi.".into()
        };

        FileAssociationStatus {
            supported: true,
            enabled,
            associated_extensions: associated,
            exe_path: current_exe.or(exe_from_cmd),
            message,
        }
    }
}

/// Register Archi as the default open handler for supported extensions (HKCU).
pub fn register_file_associations() -> Result<FileAssociationStatus, CommandError> {
    #[cfg(not(windows))]
    {
        return Err(assoc_error(
            "unsupported_platform",
            "File associations are only available on Windows.",
        ));
    }

    #[cfg(windows)]
    {
        let exe = current_exe_path()?;
        let exe_str = exe.to_string_lossy();
        let open_cmd = build_open_command(&exe_str);

        write_default(&format!(r"Software\Classes\{}", PROGID), PROGID_DESC)?;
        write_default(
            &format!(r"Software\Classes\{}\DefaultIcon", PROGID),
            &format!("{},0", exe_str),
        )?;
        write_default(
            &format!(r"Software\Classes\{}\shell\open\command", PROGID),
            &open_cmd,
        )?;

        for ext in ASSOCIATED_EXTENSIONS {
            write_default(&format!(r"Software\Classes\.{}", ext), PROGID)?;
        }

        set_app_flag(true)?;
        notify_shell();
        Ok(get_file_association_status())
    }
}

/// Remove Archi ProgId and clear extensions that still point at it.
pub fn unregister_file_associations() -> Result<FileAssociationStatus, CommandError> {
    #[cfg(not(windows))]
    {
        return Err(assoc_error(
            "unsupported_platform",
            "File associations are only available on Windows.",
        ));
    }

    #[cfg(windows)]
    {
        for ext in ASSOCIATED_EXTENSIONS {
            let path = format!(r"Software\Classes\.{}", ext);
            if read_default(&path).as_deref() == Some(PROGID) {
                // Clear default only; leave other values under .ext if any.
                use winreg::enums::*;
                use winreg::RegKey;
                let hkcu = RegKey::predef(HKEY_CURRENT_USER);
                if let Ok(key) = hkcu.open_subkey_with_flags(&path, KEY_SET_VALUE) {
                    let _ = key.delete_value("");
                }
            }
        }
        delete_tree(&format!(r"Software\Classes\{}", PROGID))?;
        set_app_flag(false)?;
        notify_shell();
        Ok(get_file_association_status())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_paths_for_command() {
        assert_eq!(
            build_open_command(r"C:\Program Files\archi\archi.exe"),
            r#""C:\Program Files\archi\archi.exe" "%1""#
        );
    }

    #[test]
    fn extension_list_covers_product_formats() {
        assert!(ASSOCIATED_EXTENSIONS.contains(&"zip"));
        assert!(ASSOCIATED_EXTENSIONS.contains(&"7z"));
        assert!(ASSOCIATED_EXTENSIONS.contains(&"tar"));
        assert!(ASSOCIATED_EXTENSIONS.contains(&"gz"));
    }

    #[test]
    fn status_supported_flag_matches_platform() {
        let status = get_file_association_status();
        #[cfg(windows)]
        assert!(status.supported);
        #[cfg(not(windows))]
        assert!(!status.supported);
    }
}
