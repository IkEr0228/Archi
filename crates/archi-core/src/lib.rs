//! Archive operations shared by desktop adapters. No UI runtime dependency.
pub mod archive;
pub mod archive_edit;
pub mod bzip2_format;
pub mod conflict;
pub mod create_common;
pub mod destination;
mod edit_plan;
mod edit_sources;
pub mod extraction;
pub mod format_detect;
pub mod gzip_format;
pub mod io_perf;
pub mod models;
pub mod operations;
pub mod rar_format;
#[cfg(windows)]
mod rar_stream;
pub mod security;
pub mod sevenz_edit;
pub mod sevenz_format;
pub mod sevenz_pack_copy;
mod stream_extract;
pub mod tar_create;
pub mod tar_edit;
pub mod tar_format;
pub mod testing;
pub mod xz_format;
pub mod zip_cd;
pub mod zip_edit;
pub mod zipper;

#[cfg(windows)]
mod windows_fs;
