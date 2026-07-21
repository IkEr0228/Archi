# Vendored sevenz-rust2 (Archi)

- **Upstream:** https://github.com/hasenbanck/sevenz-rust2
- **Version:** 0.21.3
- **License:** Apache-2.0 (see `LICENSE`)

## Archi patch (pack-stream copy spike)

Added APIs for byte-copy of already-compressed non-solid pack streams:

- `ArchiveWriter::push_packed_entry` — write precompressed pack bytes + folder metadata without LZMA re-encode
- `Coder::properties` / stream-count accessors
- `Block::num_unpack_sub_streams` / `packed_streams_count` / `unpack_sizes`
- Internal `UnpackInfo::add_raw` + raw coder property emission

Do not upgrade this vendor tree casually; re-apply the patch when bumping.

Archi (MIT) depends on this crate via path/`[patch.crates-io]`. Combined work must respect Apache-2.0 attribution for this dependency.
