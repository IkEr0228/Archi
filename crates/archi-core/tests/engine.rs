// Run the existing engine regression suite without building the legacy UI shell.
extern crate archi_core as archi_core;

#[path = "archive_listing.rs"]
mod archive_listing;
#[path = "archive_test.rs"]
mod archive_test;
#[path = "conflict_names.rs"]
mod conflict_names;
#[path = "ensure_directory.rs"]
mod ensure_directory;
#[path = "extraction.rs"]
mod extraction;
#[path = "operations_conflict.rs"]
mod operations_conflict;
#[path = "rar.rs"]
mod rar;
#[path = "security.rs"]
mod security;
#[path = "sevenz.rs"]
mod sevenz;
#[path = "sevenz_edit.rs"]
mod sevenz_edit;
#[path = "sevenz_pack_copy_spike.rs"]
mod sevenz_pack_copy_spike;
#[path = "tar_create.rs"]
mod tar_create;
#[path = "tar_edit.rs"]
mod tar_edit;
#[path = "tar_gzip.rs"]
mod tar_gzip;
#[path = "zip_edit.rs"]
mod zip_edit;
#[path = "zip_encrypted.rs"]
mod zip_encrypted;
#[path = "zipper.rs"]
mod zipper;
