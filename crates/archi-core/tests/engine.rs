// Run the existing engine regression suite without building the legacy UI shell.
extern crate archi_core as archi_backend_lib;

#[path = "../../../src-tauri/tests/archive_listing.rs"]
mod archive_listing;
#[path = "../../../src-tauri/tests/archive_test.rs"]
mod archive_test;
#[path = "../../../src-tauri/tests/conflict_names.rs"]
mod conflict_names;
#[path = "../../../src-tauri/tests/ensure_directory.rs"]
mod ensure_directory;
#[path = "../../../src-tauri/tests/extraction.rs"]
mod extraction;
#[path = "../../../src-tauri/tests/operations_conflict.rs"]
mod operations_conflict;
#[path = "../../../src-tauri/tests/rar.rs"]
mod rar;
#[path = "../../../src-tauri/tests/security.rs"]
mod security;
#[path = "../../../src-tauri/tests/sevenz.rs"]
mod sevenz;
#[path = "../../../src-tauri/tests/sevenz_edit.rs"]
mod sevenz_edit;
#[path = "../../../src-tauri/tests/sevenz_pack_copy_spike.rs"]
mod sevenz_pack_copy_spike;
#[path = "../../../src-tauri/tests/tar_create.rs"]
mod tar_create;
#[path = "../../../src-tauri/tests/tar_edit.rs"]
mod tar_edit;
#[path = "../../../src-tauri/tests/tar_gzip.rs"]
mod tar_gzip;
#[path = "../../../src-tauri/tests/zip_edit.rs"]
mod zip_edit;
#[path = "../../../src-tauri/tests/zip_encrypted.rs"]
mod zip_encrypted;
#[path = "../../../src-tauri/tests/zipper.rs"]
mod zipper;
