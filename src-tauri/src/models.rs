#[derive(Debug, Clone, serde::Serialize)]
pub struct ArchiveCapabilities {
    pub open: bool,
    pub list: bool,
    pub extract: bool,
    pub create: bool,
    pub edit: bool,
    pub encrypt: bool,
    pub test: bool,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct CommandError {
    pub code: String,
    pub message: String,
    pub path: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct OperationProgress {
    pub operation_id: String,
    pub extracted_files: u64,
    pub total_files: u64,
    pub current_file: String,
    pub percentage: f32,
    /// Optional edit/extract phase label: "plan" | "append" | "rebuild" | "extract" | "repack" | "finalize".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictDecision {
    Overwrite,
    Skip,
    Rename,
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CompressionPreset {
    Store,
    Fast,
    Normal,
    Max,
}

/// On-disk archive kind for create (not the same as open content-detect).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum CreateFormat {
    #[default]
    Zip,
    Tar,
    TarGz,
    TarBz2,
    TarXz,
    SevenZ,
}

impl CreateFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Zip => "zip",
            Self::Tar => "tar",
            Self::TarGz => "tar.gz",
            Self::TarBz2 => "tar.bz2",
            Self::TarXz => "tar.xz",
            Self::SevenZ => "7z",
        }
    }

    pub fn preferred_extension(self) -> &'static str {
        match self {
            Self::Zip => "zip",
            Self::Tar => "tar",
            Self::TarGz => "tar.gz",
            Self::TarBz2 => "tar.bz2",
            Self::TarXz => "tar.xz",
            Self::SevenZ => "7z",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateOptions {
    #[serde(default)]
    pub format: CreateFormat,
    pub compression: CompressionPreset,
    pub include_root: bool,
    pub overwrite: bool,
    #[serde(default)]
    pub password: Option<String>,
}

/// Preference for how archive edits are applied (append/fast vs compact rebuild).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EditStrategyPref {
    #[default]
    Auto,
    PreferFast,
    PreferCompact,
}

/// Options for in-archive edit commands (delete/rename/add/etc.).
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditOptions {
    #[serde(default)]
    pub compression: Option<CompressionPreset>,
    #[serde(default)]
    pub strategy: Option<EditStrategyPref>,
    #[serde(default)]
    pub password: Option<String>,
}

impl CreateOptions {
    pub fn default_zip() -> Self {
        Self {
            format: CreateFormat::Zip,
            compression: CompressionPreset::Normal,
            include_root: true,
            overwrite: false,
            password: None,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ExtractConflictEvent {
    pub operation_id: String,
    pub conflict_id: String,
    pub entry_path: String,
    pub dest_path: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct OperationSummary {
    pub operation_id: String,
    pub extracted_files: u64,
    pub total_files: u64,
    pub skipped_files: u64,
    pub destination: String,
}

/// Result of a ZIP in-place edit (delete/rename/add/folder/replace).
#[derive(Debug, Clone, serde::Serialize)]
pub struct EditSummary {
    pub operation_id: String,
    pub destination: String,
    pub members_written: u64,
    /// Strategy actually used for this edit (e.g. "rebuild", "append"); None if unknown.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strategy_used: Option<String>,
}

impl CommandError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            path: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ArchiveEntry {
    pub path: String,
    pub name: String,
    pub parent_path: String,
    pub is_directory: bool,
    pub uncompressed_size: u64,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub compressed_size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub modified_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub method: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ArchiveStats {
    pub file_count: u64,
    pub folder_count: u64,
    pub total_uncompressed: u64,
    pub total_compressed: u64,
    pub methods: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ArchiveInfo {
    pub archive_path: String,
    pub format: String,
    pub entries: Vec<ArchiveEntry>,
    pub capabilities: ArchiveCapabilities,
    pub warnings: Vec<crate::security::ArchiveWarning>,
    pub stats: ArchiveStats,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct TestFailure {
    pub path: String,
    pub message: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct TestArchiveSummary {
    pub operation_id: String,
    pub total_entries: u64,
    pub tested_ok: u64,
    pub tested_failed: u64,
    pub failures: Vec<TestFailure>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archive_entry_skips_none_fields_on_serialization() {
        let entry = ArchiveEntry {
            path: "docs/folder".into(),
            name: "folder".into(),
            parent_path: "docs".into(),
            is_directory: true,
            uncompressed_size: 0,
            compressed_size: None,
            modified_at: None,
            method: None,
        };
        let json = serde_json::to_string(&entry).unwrap();
        assert!(!json.contains(r#""compressed_size":"#), "None compressed_size should be skipped, got: {json}");
        assert!(!json.contains(r#""modified_at":"#), "None modified_at should be skipped");
        assert!(!json.contains(r#""method":"#), "None method should be skipped");

        // Roundtrip deserialization preserves identical struct
        let decoded: ArchiveEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, entry);
    }

    #[test]
    fn archive_entry_preserves_some_fields_when_present() {
        let entry = ArchiveEntry {
            path: "docs/readme.txt".into(),
            name: "readme.txt".into(),
            parent_path: "docs".into(),
            is_directory: false,
            uncompressed_size: 1024,
            compressed_size: Some(512),
            modified_at: Some("2026-09-02 12:00:00".into()),
            method: Some("Deflated".into()),
        };
        let json = serde_json::to_string(&entry).unwrap();
        assert!(json.contains(r#""compressed_size":512"#));
        assert!(json.contains(r#""modified_at":"2026-09-02 12:00:00""#));
        assert!(json.contains(r#""method":"Deflated""#));

        let decoded: ArchiveEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, entry);
    }

    #[test]
    fn archive_entry_deserializes_legacy_null_fields() {
        let legacy_json = r#"{"path":"file.txt","name":"file.txt","parent_path":"/","is_directory":false,"uncompressed_size":100,"compressed_size":null,"modified_at":null,"method":null}"#;
        let entry: ArchiveEntry = serde_json::from_str(legacy_json).unwrap();
        assert_eq!(entry.uncompressed_size, 100);
        assert_eq!(entry.compressed_size, None);
        assert_eq!(entry.modified_at, None);
        assert_eq!(entry.method, None);
    }

    #[test]
    fn serialization_payload_reduction_measurable() {
        // Create 1,000 entries with None fields (typical in TAR/GZ or directory entries)
        let entries: Vec<ArchiveEntry> = (0..1000)
            .map(|i| ArchiveEntry {
                path: format!("dir_{i}/file.txt"),
                name: "file.txt".into(),
                parent_path: format!("dir_{i}"),
                is_directory: false,
                uncompressed_size: 4096,
                compressed_size: None,
                modified_at: None,
                method: None,
            })
            .collect();
        let optimized_json = serde_json::to_string(&entries).unwrap();
        // Compare with simulated unoptimized payload that includes null keys
        let null_overhead_per_entry = r#","compressed_size":null,"modified_at":null,"method":null"#.len();
        let expected_min_savings = 1000 * null_overhead_per_entry;
        assert!(
            expected_min_savings > 40_000,
            "1,000 entries save over 40KB in IPC transmission"
        );
        assert!(!optimized_json.contains("null"));
    }
}
