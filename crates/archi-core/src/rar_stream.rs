//! Windows UnRAR callback adapter: CRC-checked chunks go to pinned staging handles,
//! never to a destination path owned by the native library.
use crate::models::CommandError;
use crate::rar_format::map_unrar_error;
use std::ffi::CString;
use std::io::Write;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, Ordering};
use unrar_sys as native;

pub(crate) struct Reader(NonNull<native::Handle>);
pub(crate) struct Header {
    pub name: String,
    pub is_directory: bool,
    pub is_special: bool,
}

impl Drop for Reader {
    fn drop(&mut self) {
        // SAFETY: Reader exclusively owns this live UnRAR handle.
        unsafe {
            native::RARCloseArchive(self.0.as_ptr());
        }
    }
}

fn check(code: i32, when: unrar::error::When) -> Result<(), CommandError> {
    match unrar::error::Code::from(code) {
        Some(unrar::error::Code::Success) => Ok(()),
        Some(code) => Err(map_unrar_error(unrar::error::UnrarError::from(code, when))),
        None => Err(CommandError::new(
            "read_failed",
            format!("Unknown UnRAR error: {code}"),
        )),
    }
}

impl Reader {
    pub fn open(path: &Path, password: Option<&str>) -> Result<Self, CommandError> {
        let name: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        if name[..name.len() - 1].contains(&0) {
            return Err(CommandError::new(
                "invalid_archive",
                "Archive path contains NUL.",
            ));
        }
        let password = password
            .map(CString::new)
            .transpose()
            .map_err(|_| CommandError::new("invalid_password", "Password contains NUL."))?;
        let mut data = native::OpenArchiveDataEx::new(name.as_ptr(), native::RAR_OM_EXTRACT);
        // SAFETY: path and data remain live for the synchronous open call.
        let handle = unsafe { native::RAROpenArchiveEx(&mut data) };
        let reader = NonNull::new(handle.cast_mut()).map(Self);
        check(data.open_result as i32, unrar::error::When::Open)?;
        let reader = reader.ok_or_else(|| {
            CommandError::new("invalid_archive", "UnRAR returned no archive handle.")
        })?;
        if let Some(password) = password {
            // SAFETY: live handle; UnRAR copies password during this call.
            unsafe {
                native::RARSetPassword(reader.0.as_ptr(), password.as_ptr());
            }
        }
        Ok(reader)
    }

    pub fn next(&mut self) -> Result<Option<Header>, CommandError> {
        let mut header = native::HeaderDataEx::default();
        // SAFETY: exclusive live handle and initialized output buffer.
        let code = unsafe { native::RARReadHeaderEx(self.0.as_ptr(), &mut header) };
        if code == native::ERAR_END_ARCHIVE {
            return Ok(None);
        }
        check(code, unrar::error::When::Read)?;
        let length = header
            .filename_w
            .iter()
            .position(|c| *c == 0)
            .ok_or_else(|| CommandError::new("invalid_entry", "RAR filename is not terminated."))?;
        let name = String::from_utf16(&header.filename_w[..length])
            .map_err(|_| CommandError::new("invalid_entry", "RAR filename is not valid UTF-16."))?;
        Ok(Some(Header {
            name,
            is_directory: header.flags & native::RHDF_DIRECTORY != 0,
            is_special: header.redir_type != 0
                || (header.host_os == 2 && header.file_attr & 0x400 != 0)
                || (header.host_os == 3 && header.file_attr & 0o170000 == 0o120000),
        }))
    }

    pub fn skip(&mut self) -> Result<(), CommandError> {
        // SAFETY: live handle positioned immediately before file data.
        check(
            unsafe {
                native::RARProcessFileW(
                    self.0.as_ptr(),
                    native::RAR_SKIP,
                    std::ptr::null(),
                    std::ptr::null(),
                )
            },
            unrar::error::When::Process,
        )
    }

    pub fn write(
        &mut self,
        writer: &mut dyn Write,
        cancelled: &AtomicBool,
    ) -> Result<(), CommandError> {
        let mut state = CallbackState {
            writer,
            cancelled,
            error: None,
        };
        // SAFETY: synchronous processing keeps stack userdata alive. Reset callback before return.
        let code = unsafe {
            native::RARSetCallback(
                self.0.as_ptr(),
                Some(callback),
                &mut state as *mut _ as native::LPARAM,
            );
            let code = native::RARProcessFileW(
                self.0.as_ptr(),
                native::RAR_TEST,
                std::ptr::null(),
                std::ptr::null(),
            );
            native::RARSetCallback(self.0.as_ptr(), None, 0);
            code
        };
        if let Some(error) = state.error {
            return Err(error);
        }
        if cancelled.load(Ordering::Relaxed) {
            return Err(CommandError::new(
                "cancelled",
                "RAR extraction was cancelled.",
            ));
        }
        check(code, unrar::error::When::Process)
    }
}

struct CallbackState<'a> {
    writer: &'a mut dyn Write,
    cancelled: &'a AtomicBool,
    error: Option<CommandError>,
}

#[cfg(test)]
#[path = "../tests/common/rar_fixture.rs"]
mod fixture;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn large_file_cancels_inside_native_callback_and_writer_errors_propagate() {
        let path =
            std::env::temp_dir().join(format!("archi-rar-callback-{}.rar", std::process::id()));
        let payload = vec![b'x'; 8 * 1024 * 1024];
        super::fixture::stored(&path, "large", &payload, false);
        let cancelled = AtomicBool::new(false);
        struct CancelWriter<'a> {
            flag: &'a AtomicBool,
            bytes: usize,
        }
        impl Write for CancelWriter<'_> {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.bytes += bytes.len();
                self.flag.store(true, Ordering::Relaxed);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut writer = CancelWriter {
            flag: &cancelled,
            bytes: 0,
        };
        let mut reader = Reader::open(&path, None).unwrap();
        reader.next().unwrap().unwrap();
        assert_eq!(
            reader.write(&mut writer, &cancelled).unwrap_err().code,
            "cancelled"
        );
        assert!(writer.bytes > 0 && writer.bytes < payload.len());
        drop(reader);
        cancelled.store(false, Ordering::Relaxed);
        let mut reader = Reader::open(&path, None).unwrap();
        reader.next().unwrap().unwrap();
        let mut output = [0_u8; 1];
        assert_eq!(
            reader
                .write(&mut output.as_mut_slice(), &cancelled)
                .unwrap_err()
                .code,
            "write_failed"
        );
        drop(reader);
        std::fs::remove_file(path).unwrap();
    }
}

extern "C" fn callback(
    message: native::UINT,
    userdata: native::LPARAM,
    p1: native::LPARAM,
    p2: native::LPARAM,
) -> i32 {
    if userdata == 0 {
        return -1;
    }
    // SAFETY: only installed by Reader::write, for that synchronous call's lifetime.
    let state = unsafe { &mut *(userdata as *mut CallbackState<'_>) };
    // Never allow a Rust writer panic to unwind across the C ABI.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if state.cancelled.load(Ordering::Relaxed) {
            return Err(CommandError::new(
                "cancelled",
                "RAR extraction was cancelled.",
            ));
        }
        match message {
            native::UCM_PROCESSDATA => {
                if p2 < 0 || (p1 == 0 && p2 != 0) {
                    return Err(CommandError::new(
                        "read_failed",
                        "Invalid UnRAR callback buffer.",
                    ));
                }
                if p2 > 0 {
                    // SAFETY: UnRAR provides a buffer valid for p2 bytes during this callback.
                    let bytes = unsafe { std::slice::from_raw_parts(p1 as *const u8, p2 as usize) };
                    state
                        .writer
                        .write_all(bytes)
                        .map_err(|e| CommandError::new("write_failed", e.to_string()))?;
                }
            }
            native::UCM_CHANGEVOLUMEW | native::UCM_CHANGEVOLUME if p2 == native::RAR_VOL_ASK => {
                return Err(CommandError::new(
                    "read_failed",
                    "Next RAR volume is unavailable.",
                ))
            }
            native::UCM_NEEDPASSWORD | native::UCM_NEEDPASSWORDW => {
                return Err(CommandError::new(
                    "password_required",
                    "Archive requires a password.",
                ))
            }
            _ => {}
        }
        Ok(())
    }));
    match result {
        Ok(Ok(())) => 0,
        Ok(Err(error)) => {
            state.error = Some(error);
            -1
        }
        Err(_) => {
            state.error = Some(CommandError::new(
                "write_failed",
                "RAR output writer panicked.",
            ));
            -1
        }
    }
}
