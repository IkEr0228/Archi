use std::path::{Path, PathBuf};

/// Skip argv[0] and flags (-*). First remaining arg joined with cwd if relative.
pub fn resolve_cli_archive_path(args: &[String], cwd: &Path) -> Option<PathBuf> {
    let candidate = args
        .iter()
        .skip(1)
        .find(|arg| !arg.is_empty() && !arg.starts_with('-'))?;

    let path = PathBuf::from(candidate);
    let resolved = if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    };

    match resolved.canonicalize() {
        Ok(canonical) => Some(canonical),
        Err(_) => Some(resolved),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_cwd(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("archi-cli-open-{label}-{nanos}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn empty_args_returns_none() {
        let cwd = PathBuf::from(r"C:\work");
        assert_eq!(resolve_cli_archive_path(&[], &cwd), None);
    }

    #[test]
    fn only_exe_returns_none() {
        let cwd = PathBuf::from(r"C:\work");
        let args = vec![String::from(r"C:\apps\archi.exe")];
        assert_eq!(resolve_cli_archive_path(&args, &cwd), None);
    }

    #[test]
    fn skips_flags_then_takes_path() {
        let cwd = PathBuf::from(r"C:\work");
        let args = vec![
            String::from(r"C:\apps\archi.exe"),
            String::from("--verbose"),
            String::from("-f"),
            String::from(r"C:\data\archive.zip"),
            String::from("ignored.zip"),
        ];
        let resolved = resolve_cli_archive_path(&args, &cwd).expect("path");
        assert_eq!(resolved, PathBuf::from(r"C:\data\archive.zip"));
    }

    #[test]
    fn relative_joins_cwd() {
        let cwd = PathBuf::from(r"C:\work");
        let args = vec![
            String::from(r"C:\apps\archi.exe"),
            String::from(r"nested\file.zip"),
        ];
        let resolved = resolve_cli_archive_path(&args, &cwd).expect("path");
        assert_eq!(resolved, cwd.join(r"nested\file.zip"));
    }

    #[test]
    fn absolute_returned_as_is() {
        let cwd = PathBuf::from(r"C:\work");
        let absolute = PathBuf::from(r"D:\archives\sample.zip");
        let args = vec![
            String::from(r"C:\apps\archi.exe"),
            absolute.display().to_string(),
        ];
        let resolved = resolve_cli_archive_path(&args, &cwd).expect("path");
        assert_eq!(resolved, absolute);
    }

    #[test]
    fn skips_empty_args() {
        let cwd = PathBuf::from(r"C:\work");
        let args = vec![
            String::from(r"C:\apps\archi.exe"),
            String::from(""),
            String::from(r"C:\data\a.zip"),
        ];
        let resolved = resolve_cli_archive_path(&args, &cwd).expect("path");
        assert_eq!(resolved, PathBuf::from(r"C:\data\a.zip"));
    }

    #[test]
    fn existing_path_is_canonicalized() {
        let cwd = temp_cwd("exist");
        let file = cwd.join("present.zip");
        fs::write(&file, b"pk").unwrap();

        let args = vec![String::from("archi.exe"), String::from("present.zip")];
        let resolved = resolve_cli_archive_path(&args, &cwd).expect("path");
        let expected = file.canonicalize().unwrap();
        assert_eq!(resolved, expected);

        fs::remove_dir_all(&cwd).unwrap();
    }
}
