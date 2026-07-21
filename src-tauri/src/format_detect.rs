use crate::models::CommandError;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// Supported on-disk archive kinds for open/list/extract dispatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveFormat {
    Zip,
    Tar,
    TarGz,
    Gzip,
    TarBz2,
    Bzip2,
    TarXz,
    Xz,
    SevenZ,
}

impl ArchiveFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Zip => "zip",
            Self::Tar => "tar",
            Self::TarGz => "tar.gz",
            Self::Gzip => "gzip",
            Self::TarBz2 => "tar.bz2",
            Self::Bzip2 => "bzip2",
            Self::TarXz => "tar.xz",
            Self::Xz => "xz",
            Self::SevenZ => "7z",
        }
    }
}

fn detect_error(message: impl Into<String>) -> CommandError {
    CommandError::new("unsupported_format", message)
}

fn read_prefix(path: &Path, len: usize) -> Result<Vec<u8>, CommandError> {
    let mut file = File::open(path).map_err(|error| {
        CommandError::new(
            if error.kind() == std::io::ErrorKind::NotFound {
                "not_found"
            } else {
                "invalid_archive"
            },
            format!("Cannot open archive: {error}"),
        )
    })?;
    let mut buf = vec![0_u8; len];
    let n = file.read(&mut buf).map_err(|error| {
        CommandError::new("invalid_archive", format!("Cannot read archive: {error}"))
    })?;
    buf.truncate(n);
    Ok(buf)
}

fn looks_like_zip(prefix: &[u8]) -> bool {
    prefix.len() >= 4
        && (prefix.starts_with(b"PK\x03\x04")
            || prefix.starts_with(b"PK\x05\x06")
            || prefix.starts_with(b"PK\x07\x08"))
}

fn looks_like_gzip(prefix: &[u8]) -> bool {
    prefix.len() >= 2 && prefix[0] == 0x1f && prefix[1] == 0x8b
}

/// bzip2 stream magic: "BZh" + digit block size.
fn looks_like_bzip2(prefix: &[u8]) -> bool {
    prefix.len() >= 3 && prefix[0] == b'B' && prefix[1] == b'Z' && prefix[2] == b'h'
}

/// XZ stream magic: FD 37 7A 58 5A 00
fn looks_like_xz(prefix: &[u8]) -> bool {
    prefix.len() >= 6 && prefix.starts_with(&[0xFD, 0x37, 0x7A, 0x58, 0x5A, 0x00])
}

/// 7z signature: "7z" BC AF 27 1C
fn looks_like_sevenz(prefix: &[u8]) -> bool {
    prefix.len() >= 6 && prefix.starts_with(&[0x37, 0x7A, 0xBC, 0xAF, 0x27, 0x1C])
}

/// POSIX/ustar magic at header offset 257, or empty block start of classic tar.
fn looks_like_tar_header(header_block: &[u8]) -> bool {
    if header_block.len() < 262 {
        return false;
    }
    // ustar\0 or ustar  (GNU)
    let magic = &header_block[257..262];
    magic == b"ustar"
        || header_block[257..263] == *b"ustar\0"
        || header_block[257..263] == *b"ustar "
}

fn gzip_payload_looks_like_tar(path: &Path) -> Result<bool, CommandError> {
    use flate2::read::GzDecoder;

    let file = File::open(path).map_err(|error| {
        CommandError::new(
            "invalid_archive",
            format!("Cannot open gzip stream: {error}"),
        )
    })?;
    let mut decoder = GzDecoder::new(file);
    let mut block = [0_u8; 512];
    let n = decoder.read(&mut block).map_err(|error| {
        CommandError::new(
            "invalid_archive",
            format!("Cannot decompress gzip for format detect: {error}"),
        )
    })?;
    if n == 0 {
        return Ok(false);
    }
    if n < 512 {
        // Tiny payload: not a full tar header block → raw gzip.
        return Ok(false);
    }
    Ok(looks_like_tar_header(&block[..n]))
}

fn bzip2_payload_looks_like_tar(path: &Path) -> Result<bool, CommandError> {
    use bzip2::read::BzDecoder;

    let file = File::open(path).map_err(|error| {
        CommandError::new(
            "invalid_archive",
            format!("Cannot open bzip2 stream: {error}"),
        )
    })?;
    let mut decoder = BzDecoder::new(file);
    let mut block = [0_u8; 512];
    let n = decoder.read(&mut block).map_err(|error| {
        CommandError::new(
            "invalid_archive",
            format!("Cannot decompress bzip2 for format detect: {error}"),
        )
    })?;
    if n == 0 {
        return Ok(false);
    }
    if n < 512 {
        return Ok(false);
    }
    Ok(looks_like_tar_header(&block[..n]))
}

fn xz_payload_looks_like_tar(path: &Path) -> Result<bool, CommandError> {
    use xz2::read::XzDecoder;

    let file = File::open(path).map_err(|error| {
        CommandError::new("invalid_archive", format!("Cannot open xz stream: {error}"))
    })?;
    let mut decoder = XzDecoder::new(file);
    let mut block = [0_u8; 512];
    let n = decoder.read(&mut block).map_err(|error| {
        CommandError::new(
            "invalid_archive",
            format!("Cannot decompress xz for format detect: {error}"),
        )
    })?;
    if n == 0 {
        return Ok(false);
    }
    if n < 512 {
        return Ok(false);
    }
    Ok(looks_like_tar_header(&block[..n]))
}

fn looks_like_tar_file(path: &Path) -> Result<bool, CommandError> {
    let mut file = File::open(path).map_err(|error| {
        CommandError::new("invalid_archive", format!("Cannot open file: {error}"))
    })?;
    let mut block = [0_u8; 512];
    let n = file.read(&mut block).map_err(|error| {
        CommandError::new("invalid_archive", format!("Cannot read file: {error}"))
    })?;
    if n < 512 {
        return Ok(false);
    }
    if looks_like_tar_header(&block) {
        return Ok(true);
    }
    // Old tar: name field non-empty, size octal field somewhat plausible, checksum area present.
    // Prefer trying archive parse only when extension hints tar.
    let _ = file.seek(SeekFrom::Start(0));
    let mut archive = tar::Archive::new(file);
    match archive.entries() {
        Ok(mut entries) => match entries.next() {
            Some(Ok(_)) => Ok(true),
            Some(Err(_)) => Ok(false),
            None => Ok(true), // empty tar is still tar
        },
        Err(_) => Ok(false),
    }
}

/// Detect archive format using content signatures (extension is not authoritative).
pub fn detect_format(path: &Path) -> Result<ArchiveFormat, CommandError> {
    if !path.is_file() {
        return Err(CommandError::new(
            "not_found",
            "File not found or is not a file.",
        ));
    }

    let prefix = read_prefix(path, 512)?;
    if prefix.is_empty() {
        return Err(detect_error("Archive file is empty."));
    }

    if looks_like_zip(&prefix) {
        return Ok(ArchiveFormat::Zip);
    }

    if looks_like_sevenz(&prefix) {
        return Ok(ArchiveFormat::SevenZ);
    }

    if looks_like_gzip(&prefix) {
        return if gzip_payload_looks_like_tar(path)? {
            Ok(ArchiveFormat::TarGz)
        } else {
            Ok(ArchiveFormat::Gzip)
        };
    }

    if looks_like_bzip2(&prefix) {
        return if bzip2_payload_looks_like_tar(path)? {
            Ok(ArchiveFormat::TarBz2)
        } else {
            Ok(ArchiveFormat::Bzip2)
        };
    }

    if looks_like_xz(&prefix) {
        return if xz_payload_looks_like_tar(path)? {
            Ok(ArchiveFormat::TarXz)
        } else {
            Ok(ArchiveFormat::Xz)
        };
    }

    if looks_like_tar_file(path)? {
        return Ok(ArchiveFormat::Tar);
    }

    Err(detect_error("Unsupported or unrecognized archive format."))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("archi-detect-{label}-{nanos}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn detects_gzip_magic_as_gzip_when_not_tar() {
        use flate2::write::GzEncoder;
        use flate2::Compression;

        let root = temp_dir("gz");
        let path = root.join("blob.gz");
        let file = File::create(&path).unwrap();
        let mut enc = GzEncoder::new(file, Compression::default());
        enc.write_all(b"hello raw gzip payload").unwrap();
        enc.finish().unwrap();

        assert_eq!(detect_format(&path).unwrap(), ArchiveFormat::Gzip);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn detects_tar_gz() {
        use flate2::write::GzEncoder;
        use flate2::Compression;
        use tar::Builder;

        let root = temp_dir("tgz");
        let path = root.join("a.tar.gz");
        let file = File::create(&path).unwrap();
        let enc = GzEncoder::new(file, Compression::default());
        let mut builder = Builder::new(enc);
        let mut header = tar::Header::new_gnu();
        header.set_path("hi.txt").unwrap();
        header.set_size(5);
        header.set_mode(0o644);
        header.set_cksum();
        builder.append(&header, &b"hello"[..]).unwrap();
        builder.into_inner().unwrap().finish().unwrap();

        assert_eq!(detect_format(&path).unwrap(), ArchiveFormat::TarGz);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn detects_plain_tar() {
        use tar::Builder;

        let root = temp_dir("tar");
        let path = root.join("a.tar");
        let file = File::create(&path).unwrap();
        let mut builder = Builder::new(file);
        let mut header = tar::Header::new_gnu();
        header.set_path("hi.txt").unwrap();
        header.set_size(5);
        header.set_mode(0o644);
        header.set_cksum();
        builder.append(&header, &b"hello"[..]).unwrap();
        builder.into_inner().unwrap();

        assert_eq!(detect_format(&path).unwrap(), ArchiveFormat::Tar);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn detects_bzip2_magic_as_bzip2_when_not_tar() {
        use bzip2::write::BzEncoder;
        use bzip2::Compression;

        let root = temp_dir("bz2");
        let path = root.join("blob.bz2");
        let file = File::create(&path).unwrap();
        let mut enc = BzEncoder::new(file, Compression::default());
        enc.write_all(b"hello raw bzip2 payload").unwrap();
        enc.finish().unwrap();

        assert_eq!(detect_format(&path).unwrap(), ArchiveFormat::Bzip2);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn detects_tar_bz2() {
        use bzip2::write::BzEncoder;
        use bzip2::Compression;
        use tar::Builder;

        let root = temp_dir("tbz");
        let path = root.join("a.tar.bz2");
        let file = File::create(&path).unwrap();
        let enc = BzEncoder::new(file, Compression::default());
        let mut builder = Builder::new(enc);
        let mut header = tar::Header::new_gnu();
        header.set_path("hi.txt").unwrap();
        header.set_size(5);
        header.set_mode(0o644);
        header.set_cksum();
        builder.append(&header, &b"hello"[..]).unwrap();
        builder.into_inner().unwrap().finish().unwrap();

        assert_eq!(detect_format(&path).unwrap(), ArchiveFormat::TarBz2);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn detects_xz_magic_as_xz_when_not_tar() {
        use xz2::write::XzEncoder;

        let root = temp_dir("xz");
        let path = root.join("blob.xz");
        let file = File::create(&path).unwrap();
        let mut enc = XzEncoder::new(file, 6);
        enc.write_all(b"hello raw xz payload").unwrap();
        enc.finish().unwrap();

        assert_eq!(detect_format(&path).unwrap(), ArchiveFormat::Xz);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn detects_tar_xz() {
        use tar::Builder;
        use xz2::write::XzEncoder;

        let root = temp_dir("txz");
        let path = root.join("a.tar.xz");
        let file = File::create(&path).unwrap();
        let enc = XzEncoder::new(file, 6);
        let mut builder = Builder::new(enc);
        let mut header = tar::Header::new_gnu();
        header.set_path("hi.txt").unwrap();
        header.set_size(5);
        header.set_mode(0o644);
        header.set_cksum();
        builder.append(&header, &b"hello"[..]).unwrap();
        builder.into_inner().unwrap().finish().unwrap();

        assert_eq!(detect_format(&path).unwrap(), ArchiveFormat::TarXz);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn detects_zip_local_header() {
        use zip::write::FileOptions;
        use zip::ZipWriter;

        let root = temp_dir("zip");
        let path = root.join("a.zip");
        let file = File::create(&path).unwrap();
        let mut zip = ZipWriter::new(file);
        zip.start_file("a.txt", FileOptions::default()).unwrap();
        zip.write_all(b"x").unwrap();
        zip.finish().unwrap();

        assert_eq!(detect_format(&path).unwrap(), ArchiveFormat::Zip);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_random_bytes() {
        let root = temp_dir("junk");
        let path = root.join("nope.bin");
        std::fs::write(&path, b"not an archive at all!!").unwrap();
        let err = detect_format(&path).unwrap_err();
        assert_eq!(err.code, "unsupported_format");
        std::fs::remove_dir_all(root).unwrap();
    }
}
