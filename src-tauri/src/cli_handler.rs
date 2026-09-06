use crate::extraction::{extract_any, AutoOverwriteConflictResolver};
use crate::models::{CommandError, CreateOptions};
use crate::sevenz_format::create_sevenz_archive;
use crate::zipper::create_zip_archive;
use sevenz_rust2::Password;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum CliAction {
    Open(PathBuf),
    ExtractHere(PathBuf),
    ExtractTo(PathBuf),
    Create(Vec<PathBuf>),
    AddZip(PathBuf),
    Add7z(PathBuf),
    Blank,
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum CliExtractTarget {
    Here,
    ToSubfolder,
}

/// Compute folder stem for archives, handling compound extensions cleanly (.tar.gz, etc.).
pub fn archive_stem(path: &Path) -> String {
    let filename = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("archive");
    let lower = filename.to_lowercase();

    for compound in &[".tar.gz", ".tar.bz2", ".tar.xz", ".tgz", ".tbz2", ".txz"] {
        if lower.ends_with(compound) {
            let stem = &filename[..filename.len() - compound.len()];
            if !stem.is_empty() {
                return stem.to_string();
            }
        }
    }

    if let Some(pos) = filename.rfind('.') {
        if pos > 0 {
            return filename[..pos].to_string();
        }
    }

    filename.to_string()
}

/// Compute archive base stem from any input source (file or directory).
pub fn source_stem(path: &Path) -> String {
    if path.is_dir() {
        path.file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("archive")
            .to_string()
    } else {
        path.file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("archive")
            .to_string()
    }
}

/// Ensure archive path does not overwrite an existing file by finding stem (n).ext.
pub fn unique_archive_path(base_path: &Path) -> PathBuf {
    if !base_path.exists() {
        return base_path.to_path_buf();
    }
    let parent = base_path.parent().unwrap_or_else(|| Path::new("."));
    let stem = base_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("archive");
    let ext = base_path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("zip");

    for i in 1..1000 {
        let candidate = parent.join(format!("{stem} ({i}).{ext}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    base_path.to_path_buf()
}

/// Parse command line arguments into structured actions.
pub fn parse_cli_action(args: &[String], cwd: &Path) -> CliAction {
    if args.len() <= 1 {
        return CliAction::Blank;
    }

    let mut extract_here = false;
    let mut extract_to = false;
    let mut create = false;
    let mut add_zip = false;
    let mut add_7z = false;
    let mut target_paths: Vec<PathBuf> = Vec::new();

    for arg in args.iter().skip(1) {
        if arg == "--extract-here" {
            extract_here = true;
        } else if arg == "--extract-to" {
            extract_to = true;
        } else if arg == "--create" {
            create = true;
        } else if arg == "--add-zip" {
            add_zip = true;
        } else if arg == "--add-7z" {
            add_7z = true;
        } else if !arg.starts_with('-') {
            let p = PathBuf::from(arg);
            let resolved = if p.is_absolute() { p } else { cwd.join(p) };
            let canonical = resolved.canonicalize().unwrap_or(resolved);
            target_paths.push(canonical);
        }
    }

    if extract_here {
        if let Some(path) = target_paths.into_iter().next() {
            return CliAction::ExtractHere(path);
        }
    } else if extract_to {
        if let Some(path) = target_paths.into_iter().next() {
            return CliAction::ExtractTo(path);
        }
    } else if create {
        if !target_paths.is_empty() {
            return CliAction::Create(target_paths);
        }
    } else if add_zip {
        if let Some(path) = target_paths.into_iter().next() {
            return CliAction::AddZip(path);
        }
    } else if add_7z {
        if let Some(path) = target_paths.into_iter().next() {
            return CliAction::Add7z(path);
        }
    } else if let Some(path) = target_paths.into_iter().next() {
        return CliAction::Open(path);
    }

    CliAction::Blank
}

/// Execute headless extraction directly from CLI.
pub fn execute_cli_extraction(
    archive_path: &Path,
    target: CliExtractTarget,
) -> Result<PathBuf, CommandError> {
    if !archive_path.is_file() {
        return Err(CommandError::new(
            "not_found",
            format!("Archive does not exist: {}", archive_path.display()),
        ));
    }

    let parent = archive_path.parent().unwrap_or_else(|| Path::new("."));

    let destination = match target {
        CliExtractTarget::Here => parent.to_path_buf(),
        CliExtractTarget::ToSubfolder => {
            let stem = archive_stem(archive_path);
            let subfolder = parent.join(&stem);
            if !subfolder.exists() {
                std::fs::create_dir_all(&subfolder).map_err(|err| {
                    CommandError::new(
                        "create_dir_failed",
                        format!(
                            "Failed to create destination folder {}: {err}",
                            subfolder.display()
                        ),
                    )
                })?;
            }
            subfolder
        }
    };

    let cancelled = AtomicBool::new(false);
    let resolver = AutoOverwriteConflictResolver;
    let op_id = format!(
        "cli-extract-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    );

    extract_any(
        archive_path,
        &destination,
        &op_id,
        &cancelled,
        None,
        None,
        &resolver,
        |_progress| {},
    )?;

    notify_shell_folder_updated(&destination);
    Ok(destination)
}

/// Execute 1-click compression to .zip.
pub fn execute_cli_add_zip(source_path: &Path) -> Result<PathBuf, CommandError> {
    if !source_path.exists() {
        return Err(CommandError::new(
            "not_found",
            format!("Source path does not exist: {}", source_path.display()),
        ));
    }

    let parent = source_path.parent().unwrap_or_else(|| Path::new("."));
    let stem = source_stem(source_path);
    let base_output = parent.join(format!("{stem}.zip"));
    let output_path = unique_archive_path(&base_output);

    let cancelled = AtomicBool::new(false);
    let options = CreateOptions::default_zip();
    let op_id = format!(
        "cli-add-zip-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    );

    let source_str = source_path.to_string_lossy().into_owned();
    create_zip_archive(
        &[source_str],
        &output_path,
        &op_id,
        &cancelled,
        &options,
        |_| {},
    )?;

    notify_shell_folder_updated(parent);
    Ok(output_path)
}

/// Execute 1-click compression to .7z.
pub fn execute_cli_add_7z(source_path: &Path) -> Result<PathBuf, CommandError> {
    if !source_path.exists() {
        return Err(CommandError::new(
            "not_found",
            format!("Source path does not exist: {}", source_path.display()),
        ));
    }

    let parent = source_path.parent().unwrap_or_else(|| Path::new("."));
    let stem = source_stem(source_path);
    let base_output = parent.join(format!("{stem}.7z"));
    let output_path = unique_archive_path(&base_output);

    let cancelled = AtomicBool::new(false);
    let options = CreateOptions::default_7z();
    let op_id = format!(
        "cli-add-7z-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    );

    let source_str = source_path.to_string_lossy().into_owned();
    create_sevenz_archive(
        &[source_str],
        &output_path,
        &op_id,
        &cancelled,
        Password::empty(),
        &options,
        |_| {},
    )?;

    notify_shell_folder_updated(parent);
    Ok(output_path)
}

#[cfg(windows)]
pub fn notify_shell_folder_updated(path: &Path) {
    use std::os::windows::ffi::OsStrExt;
    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    #[link(name = "shell32")]
    extern "system" {
        fn SHChangeNotify(
            event: i32,
            flags: u32,
            item1: *const std::ffi::c_void,
            item2: *const std::ffi::c_void,
        );
    }
    // SHCNE_UPDATEDIR = 0x00001000, SHCNF_PATHW = 0x0005
    unsafe {
        SHChangeNotify(
            0x0000_1000,
            0x0005,
            wide.as_ptr() as *const std::ffi::c_void,
            std::ptr::null(),
        );
    }
}

#[cfg(not(windows))]
pub fn notify_shell_folder_updated(_path: &Path) {}

#[cfg(windows)]
pub fn show_native_error_box(title: &str, message: &str) {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    let title_w: Vec<u16> = OsStr::new(title)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let msg_w: Vec<u16> = OsStr::new(message)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    #[link(name = "user32")]
    extern "system" {
        fn MessageBoxW(hwnd: isize, text: *const u16, caption: *const u16, utype: u32) -> i32;
    }
    // MB_OK | MB_ICONERROR = 0x00000000 | 0x00000010
    unsafe {
        MessageBoxW(0, msg_w.as_ptr(), title_w.as_ptr(), 0x10);
    }
}

#[cfg(not(windows))]
pub fn show_native_error_box(_title: &str, _message: &str) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_archive_stem_simple_and_compound() {
        assert_eq!(archive_stem(Path::new(r"C:\work\project.zip")), "project");
        assert_eq!(archive_stem(Path::new(r"C:\work\backup.7z")), "backup");
        assert_eq!(archive_stem(Path::new(r"C:\work\data.tar.gz")), "data");
        assert_eq!(archive_stem(Path::new(r"C:\work\docs.tar.bz2")), "docs");
        assert_eq!(
            archive_stem(Path::new(r"C:\work\archive.tar.xz")),
            "archive"
        );
        assert_eq!(archive_stem(Path::new(r"C:\work\images.tgz")), "images");
        assert_eq!(archive_stem(Path::new(r"C:\work\no_ext")), "no_ext");
    }

    #[test]
    fn test_source_stem_file_and_dir() {
        assert_eq!(source_stem(Path::new(r"C:\work\report.docx")), "report");
        assert_eq!(
            source_stem(Path::new(r"C:\work\folder_name")),
            "folder_name"
        );
    }

    #[test]
    fn test_unique_archive_path() {
        let base = PathBuf::from(r"C:\non_existent_folder_xyz\test.zip");
        assert_eq!(unique_archive_path(&base), base);
    }

    #[test]
    fn test_parse_cli_actions() {
        let cwd = PathBuf::from(r"C:\work");

        assert_eq!(parse_cli_action(&[], &cwd), CliAction::Blank);
        assert_eq!(
            parse_cli_action(&[String::from("archi.exe")], &cwd),
            CliAction::Blank
        );

        let open_args = vec![String::from("archi.exe"), String::from(r"C:\work\file.zip")];
        assert_eq!(
            parse_cli_action(&open_args, &cwd),
            CliAction::Open(PathBuf::from(r"C:\work\file.zip"))
        );

        let extract_here_args = vec![
            String::from("archi.exe"),
            String::from("--extract-here"),
            String::from(r"C:\work\file.zip"),
        ];
        assert_eq!(
            parse_cli_action(&extract_here_args, &cwd),
            CliAction::ExtractHere(PathBuf::from(r"C:\work\file.zip"))
        );

        let extract_to_args = vec![
            String::from("archi.exe"),
            String::from("--extract-to"),
            String::from(r"C:\work\file.zip"),
        ];
        assert_eq!(
            parse_cli_action(&extract_to_args, &cwd),
            CliAction::ExtractTo(PathBuf::from(r"C:\work\file.zip"))
        );

        let create_args = vec![
            String::from("archi.exe"),
            String::from("--create"),
            String::from(r"C:\work\document.pdf"),
        ];
        assert_eq!(
            parse_cli_action(&create_args, &cwd),
            CliAction::Create(vec![PathBuf::from(r"C:\work\document.pdf")])
        );

        let add_zip_args = vec![
            String::from("archi.exe"),
            String::from("--add-zip"),
            String::from(r"C:\work\my_folder"),
        ];
        assert_eq!(
            parse_cli_action(&add_zip_args, &cwd),
            CliAction::AddZip(PathBuf::from(r"C:\work\my_folder"))
        );

        let add_7z_args = vec![
            String::from("archi.exe"),
            String::from("--add-7z"),
            String::from(r"C:\work\my_folder"),
        ];
        assert_eq!(
            parse_cli_action(&add_7z_args, &cwd),
            CliAction::Add7z(PathBuf::from(r"C:\work\my_folder"))
        );
    }
}
