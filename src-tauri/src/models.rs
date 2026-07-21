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
}

impl CreateOptions {
    pub fn default_zip() -> Self {
        Self {
            format: CreateFormat::Zip,
            compression: CompressionPreset::Normal,
            include_root: true,
            overwrite: false,
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

#[derive(Debug, Clone, serde::Serialize)]
pub struct ArchiveEntry {
    pub path: String,
    pub name: String,
    pub parent_path: String,
    pub is_directory: bool,
    pub uncompressed_size: u64,
    pub compressed_size: Option<u64>,
    pub modified_at: Option<String>,
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
