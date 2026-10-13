use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

use crate::ConvertOptions;
use thiserror::Error;
use viewer_format::{FormatVersion, HEADER_ENVELOPE_VERSION};

// V5 binds resumable state to the exact output header identity and envelope.
const MANIFEST_VERSION: &str = "OCTREE_CONVERSION_CHECKPOINT_V5";
const HASH_OFFSET: u128 = 0x6c62272e07bb014262b821756295c58d;
const HASH_PRIME: u128 = 0x0000000001000000000000000000013b;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckpointMode {
    Create,
    Resume,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FileFingerprint {
    pub bytes: u64,
    pub hash: u128,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CheckpointStage {
    Started,
    SourceReady(FileFingerprint),
    OutputReady {
        fingerprint: FileFingerprint,
        attributes_fingerprint: FileFingerprint,
        point_count: u64,
        block_count: u64,
        estimated_memory_bytes: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Manifest {
    output_version: FormatVersion,
    source: FileFingerprint,
    points_per_block: u64,
    max_quantization_error_bits: u64,
    max_memory_bytes: u64,
    spatial_divisions: u8,
    stage: CheckpointStage,
}

pub(crate) struct Checkpoint {
    root: PathBuf,
    manifest: Manifest,
    lock: Option<File>,
}

#[derive(Debug, Error)]
pub enum CheckpointError {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error("checkpoint already exists at {0}; use --resume to continue it")]
    AlreadyExists(PathBuf),
    #[error("no checkpoint exists at {0}; use --checkpoint to start a resumable conversion")]
    Missing(PathBuf),
    #[error("checkpoint is invalid: {0}")]
    Invalid(String),
    #[error("checkpoint does not match the source or conversion options")]
    Mismatch,
    #[error("checkpoint is already being used by another converter")]
    Locked,
    #[error("checkpoint cleanup failed at {path}: {source}")]
    Cleanup { path: PathBuf, source: io::Error },
}

impl Checkpoint {
    pub fn open(
        output_path: &Path,
        mode: CheckpointMode,
        source: FileFingerprint,
        options: ConvertOptions,
    ) -> Result<Self, CheckpointError> {
        let root = checkpoint_path(output_path);
        match mode {
            CheckpointMode::Create => match fs::create_dir(&root) {
                Ok(()) => sync_parent(&root)?,
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    return Err(CheckpointError::AlreadyExists(root));
                }
                Err(error) => return Err(CheckpointError::Io(error)),
            },
            CheckpointMode::Resume if !root.is_dir() => {
                return Err(CheckpointError::Missing(root));
            }
            CheckpointMode::Resume => {}
        }

        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(root.join("lock"))?;
        match lock.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => return Err(CheckpointError::Locked),
            Err(std::fs::TryLockError::Error(error)) => return Err(error.into()),
        }

        let expected = Manifest {
            output_version: options.format_version(),
            source,
            points_per_block: options.points_per_block(),
            max_quantization_error_bits: options.max_quantization_error().to_bits(),
            max_memory_bytes: options.max_memory_budget_bytes(),
            spatial_divisions: options.spatial_divisions(),
            stage: CheckpointStage::Started,
        };
        let manifest_path = root.join("manifest.txt");
        let manifest = match mode {
            CheckpointMode::Create => {
                let _ = fs::remove_file(&manifest_path);
                let _ = fs::remove_file(root.join("manifest.txt.tmp"));
                let _ = fs::remove_file(root.join("source.spool"));
                let _ = fs::remove_file(root.join("candidate.oct"));
                let _ = fs::remove_file(root.join("candidate.octattrs"));
                expected
            }
            CheckpointMode::Resume => {
                let existing = read_manifest(&manifest_path, expected.output_version)?;
                if existing.output_version != expected.output_version
                    || existing.source != expected.source
                    || existing.points_per_block != expected.points_per_block
                    || existing.max_quantization_error_bits != expected.max_quantization_error_bits
                    || existing.max_memory_bytes != expected.max_memory_bytes
                    || existing.spatial_divisions != expected.spatial_divisions
                {
                    return Err(CheckpointError::Mismatch);
                }
                let _ = fs::remove_file(root.join("manifest.txt.tmp"));
                existing
            }
        };
        let mut checkpoint = Self {
            root,
            manifest,
            lock: Some(lock),
        };
        if mode == CheckpointMode::Create {
            checkpoint.persist_manifest()?;
        }
        Ok(checkpoint)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn source_path(&self) -> PathBuf {
        self.root.join("source.spool")
    }

    pub fn candidate_path(&self) -> PathBuf {
        self.root.join("candidate.oct")
    }

    pub fn candidate_attributes_path(&self) -> PathBuf {
        self.root.join("candidate.octattrs")
    }

    pub fn stage(&self) -> &CheckpointStage {
        &self.manifest.stage
    }

    pub fn output_ready_details(
        &self,
    ) -> Option<(FileFingerprint, FileFingerprint, u64, u64, u64)> {
        match self.manifest.stage {
            CheckpointStage::OutputReady {
                fingerprint,
                attributes_fingerprint,
                point_count,
                block_count,
                estimated_memory_bytes,
            } => Some((
                fingerprint,
                attributes_fingerprint,
                point_count,
                block_count,
                estimated_memory_bytes,
            )),
            _ => None,
        }
    }

    pub fn commit_source(&mut self) -> Result<(), CheckpointError> {
        let fingerprint = fingerprint_file(&self.source_path())?;
        self.manifest.stage = CheckpointStage::SourceReady(fingerprint);
        self.persist_manifest()
    }

    pub fn verify_source(&self) -> Result<(), CheckpointError> {
        let CheckpointStage::SourceReady(expected) = self.manifest.stage else {
            return Err(CheckpointError::Invalid(
                "source spool is not marked complete".to_owned(),
            ));
        };
        if fingerprint_file(&self.source_path())? != expected {
            return Err(CheckpointError::Invalid(
                "completed source spool is missing or corrupt".to_owned(),
            ));
        }
        Ok(())
    }

    pub fn verify_input(&self, path: &Path) -> Result<(), CheckpointError> {
        if fingerprint_file(path)? != self.manifest.source {
            return Err(CheckpointError::Mismatch);
        }
        Ok(())
    }

    pub fn commit_output(
        &mut self,
        point_count: u64,
        block_count: u64,
        estimated_memory_bytes: u64,
    ) -> Result<(), CheckpointError> {
        let fingerprint = fingerprint_file(&self.candidate_path())?;
        let attributes_fingerprint = fingerprint_file(&self.candidate_attributes_path())?;
        self.manifest.stage = CheckpointStage::OutputReady {
            fingerprint,
            attributes_fingerprint,
            point_count,
            block_count,
            estimated_memory_bytes,
        };
        self.persist_manifest()
    }

    pub fn verify_output(&self) -> Result<(), CheckpointError> {
        let CheckpointStage::OutputReady {
            fingerprint,
            attributes_fingerprint,
            ..
        } = self.manifest.stage
        else {
            return Err(CheckpointError::Invalid(
                "output candidate is not marked complete".to_owned(),
            ));
        };
        if fingerprint_file(&self.candidate_path())? != fingerprint {
            return Err(CheckpointError::Invalid(
                "completed output candidate is missing or corrupt".to_owned(),
            ));
        }
        if fingerprint_file(&self.candidate_attributes_path())? != attributes_fingerprint {
            return Err(CheckpointError::Invalid(
                "completed attribute candidate is missing or corrupt".to_owned(),
            ));
        }
        Ok(())
    }

    pub fn complete(mut self) -> Result<(), CheckpointError> {
        self.lock.take();
        fs::remove_dir_all(&self.root).map_err(|source| CheckpointError::Cleanup {
            path: self.root.clone(),
            source,
        })?;
        sync_parent(&self.root)?;
        Ok(())
    }

    fn persist_manifest(&mut self) -> Result<(), CheckpointError> {
        let path = self.root.join("manifest.txt");
        let temporary = self.root.join("manifest.txt.tmp");
        let bytes = encode_manifest(&self.manifest);
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&temporary)?;
        file.write_all(bytes.as_bytes())?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, &path)?;
        sync_parent(&path)?;
        Ok(())
    }
}

impl Drop for Checkpoint {
    fn drop(&mut self) {
        self.lock.take();
    }
}

pub(crate) fn fingerprint_file(path: &Path) -> Result<FileFingerprint, CheckpointError> {
    let mut file = File::open(path)?;
    let mut bytes = [0_u8; 64 * 1024];
    let mut fingerprint = HASH_OFFSET;
    let mut length = 0_u64;
    loop {
        let read = file.read(&mut bytes)?;
        if read == 0 {
            break;
        }
        length = length
            .checked_add(read as u64)
            .ok_or_else(|| CheckpointError::Invalid("file size overflow".to_owned()))?;
        for byte in &bytes[..read] {
            fingerprint ^= u128::from(*byte);
            fingerprint = fingerprint.wrapping_mul(HASH_PRIME);
        }
    }
    Ok(FileFingerprint {
        bytes: length,
        hash: fingerprint,
    })
}

fn checkpoint_path(output_path: &Path) -> PathBuf {
    let mut path = output_path.as_os_str().to_owned();
    path.push(".checkpoint");
    PathBuf::from(path)
}

fn encode_manifest(manifest: &Manifest) -> String {
    let (stage, artifact) = match manifest.stage {
        CheckpointStage::Started => ("started", None),
        CheckpointStage::SourceReady(source) => ("source-ready", Some(source)),
        CheckpointStage::OutputReady { fingerprint, .. } => ("output-ready", Some(fingerprint)),
    };
    let (point_count, block_count, estimated_memory_bytes) = match manifest.stage {
        CheckpointStage::OutputReady {
            point_count,
            block_count,
            estimated_memory_bytes,
            ..
        } => (point_count, block_count, estimated_memory_bytes),
        _ => (0, 0, 0),
    };
    format!(
        "{MANIFEST_VERSION}\noutput_major={}\noutput_revision={}\nheader_envelope={HEADER_ENVELOPE_VERSION}\nstage={stage}\nsource_bytes={}\nsource_hash={:032x}\npoints_per_block={}\nmax_error_bits={:016x}\nmax_memory_bytes={}\nspatial_divisions={}\nartifact_bytes={}\nartifact_hash={:032x}\nattributes_bytes={}\nattributes_hash={:032x}\npoint_count={point_count}\nblock_count={block_count}\nestimated_memory_bytes={estimated_memory_bytes}\n",
        manifest.output_version.major(),
        manifest.output_version.revision(),
        manifest.source.bytes,
        manifest.source.hash,
        manifest.points_per_block,
        manifest.max_quantization_error_bits,
        manifest.max_memory_bytes,
        manifest.spatial_divisions,
        artifact.map_or(0, |fingerprint| fingerprint.bytes),
        artifact.map_or(0, |fingerprint| fingerprint.hash),
        match manifest.stage {
            CheckpointStage::OutputReady {
                attributes_fingerprint,
                ..
            } => attributes_fingerprint.bytes,
            _ => 0,
        },
        match manifest.stage {
            CheckpointStage::OutputReady {
                attributes_fingerprint,
                ..
            } => attributes_fingerprint.hash,
            _ => 0,
        },
    )
}

fn read_manifest(
    path: &Path,
    expected_output_version: FormatVersion,
) -> Result<Manifest, CheckpointError> {
    let content = fs::read_to_string(path).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            CheckpointError::Invalid("manifest is missing".to_owned())
        } else {
            CheckpointError::Io(error)
        }
    })?;
    let mut lines = content.lines();
    let manifest_version = lines.next();
    if manifest_version != Some(MANIFEST_VERSION) {
        return Err(CheckpointError::Invalid(
            "unsupported manifest version".to_owned(),
        ));
    }
    let mut values = BTreeMap::new();
    for line in lines {
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| CheckpointError::Invalid("malformed manifest entry".to_owned()))?;
        if values.insert(key, value).is_some() {
            return Err(CheckpointError::Invalid(
                "duplicate manifest entry".to_owned(),
            ));
        }
    }
    let parse = |key: &str| {
        values
            .get(key)
            .ok_or_else(|| CheckpointError::Invalid(format!("missing {key}")))
    };
    let output_major: u16 = parse("output_major")?
        .parse()
        .map_err(|_| CheckpointError::Invalid("invalid output major version".to_owned()))?;
    let output_revision: u32 = parse("output_revision")?
        .parse()
        .map_err(|_| CheckpointError::Invalid("invalid output revision".to_owned()))?;
    let header_envelope: u16 = parse("header_envelope")?
        .parse()
        .map_err(|_| CheckpointError::Invalid("invalid header envelope".to_owned()))?;
    if output_major != expected_output_version.major() {
        return Err(CheckpointError::Invalid(
            "unsupported output major version".to_owned(),
        ));
    }
    if output_revision != expected_output_version.revision() {
        return Err(CheckpointError::Invalid(
            "unsupported output revision".to_owned(),
        ));
    }
    if header_envelope != HEADER_ENVELOPE_VERSION {
        return Err(CheckpointError::Invalid(
            "unsupported output header envelope".to_owned(),
        ));
    }
    let source = FileFingerprint {
        bytes: parse("source_bytes")?
            .parse()
            .map_err(|_| CheckpointError::Invalid("invalid source size".to_owned()))?,
        hash: u128::from_str_radix(parse("source_hash")?, 16)
            .map_err(|_| CheckpointError::Invalid("invalid source hash".to_owned()))?,
    };
    let stage_name = *parse("stage")?;
    let artifact = FileFingerprint {
        bytes: parse("artifact_bytes")?
            .parse()
            .map_err(|_| CheckpointError::Invalid("invalid artifact size".to_owned()))?,
        hash: u128::from_str_radix(parse("artifact_hash")?, 16)
            .map_err(|_| CheckpointError::Invalid("invalid artifact hash".to_owned()))?,
    };
    let stage = match stage_name {
        "started" => CheckpointStage::Started,
        "source-ready" => CheckpointStage::SourceReady(artifact),
        "output-ready" => CheckpointStage::OutputReady {
            fingerprint: artifact,
            attributes_fingerprint: FileFingerprint {
                bytes: parse("attributes_bytes")?
                    .parse()
                    .map_err(|_| CheckpointError::Invalid("invalid attribute size".to_owned()))?,
                hash: u128::from_str_radix(parse("attributes_hash")?, 16)
                    .map_err(|_| CheckpointError::Invalid("invalid attribute hash".to_owned()))?,
            },
            point_count: parse("point_count")?
                .parse()
                .map_err(|_| CheckpointError::Invalid("invalid point count".to_owned()))?,
            block_count: parse("block_count")?
                .parse()
                .map_err(|_| CheckpointError::Invalid("invalid block count".to_owned()))?,
            estimated_memory_bytes: parse("estimated_memory_bytes")?
                .parse()
                .map_err(|_| CheckpointError::Invalid("invalid memory estimate".to_owned()))?,
        },
        _ => {
            return Err(CheckpointError::Invalid(
                "unknown checkpoint stage".to_owned(),
            ));
        }
    };
    Ok(Manifest {
        output_version: expected_output_version,
        source,
        points_per_block: parse("points_per_block")?
            .parse()
            .map_err(|_| CheckpointError::Invalid("invalid block size".to_owned()))?,
        max_quantization_error_bits: u64::from_str_radix(parse("max_error_bits")?, 16)
            .map_err(|_| CheckpointError::Invalid("invalid quantization limit".to_owned()))?,
        max_memory_bytes: parse("max_memory_bytes")?
            .parse()
            .map_err(|_| CheckpointError::Invalid("invalid memory budget".to_owned()))?,
        spatial_divisions: parse("spatial_divisions")?
            .parse()
            .map_err(|_| CheckpointError::Invalid("invalid spatial divisions".to_owned()))?,
        stage,
    })
}

fn sync_parent(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        let parent = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        File::open(parent)?.sync_all()
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };

    use crate::ConvertOptions;

    use super::{
        Checkpoint, CheckpointError, CheckpointMode, CheckpointStage, FileFingerprint,
        MANIFEST_VERSION,
    };

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn output() -> PathBuf {
        std::env::temp_dir().join(format!(
            "octree-checkpoint-{}-{}.oct",
            std::process::id(),
            TEST_COUNTER.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn options(points_per_block: u64, spatial_divisions: u8) -> ConvertOptions {
        ConvertOptions::with_memory_budget(points_per_block, 0.01, 1024)
            .unwrap()
            .with_spatial_divisions(spatial_divisions)
            .unwrap()
    }

    #[test]
    fn persists_phase_and_rejects_changed_options_without_mutating_checkpoint() {
        let output = output();
        let source = FileFingerprint {
            bytes: 123,
            hash: 456,
        };
        let checkpoint =
            Checkpoint::open(&output, CheckpointMode::Create, source, options(32, 2)).unwrap();
        let root = checkpoint.root().to_owned();
        assert_eq!(checkpoint.stage(), &CheckpointStage::Started);
        drop(checkpoint);

        assert!(matches!(
            Checkpoint::open(&output, CheckpointMode::Resume, source, options(64, 2)),
            Err(CheckpointError::Mismatch)
        ));
        assert!(root.join("manifest.txt").is_file());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_a_second_writer_while_checkpoint_is_open() {
        let output = output();
        let source = FileFingerprint {
            bytes: 123,
            hash: 456,
        };
        let checkpoint =
            Checkpoint::open(&output, CheckpointMode::Create, source, options(32, 2)).unwrap();
        assert!(matches!(
            Checkpoint::open(&output, CheckpointMode::Resume, source, options(32, 2)),
            Err(CheckpointError::Locked)
        ));
        let root = checkpoint.root().to_owned();
        drop(checkpoint);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn resume_rejects_a_different_spatial_division() {
        let output = output();
        let source = FileFingerprint {
            bytes: 123,
            hash: 456,
        };
        let checkpoint =
            Checkpoint::open(&output, CheckpointMode::Create, source, options(32, 2)).unwrap();
        let root = checkpoint.root().to_owned();
        drop(checkpoint);

        assert!(matches!(
            Checkpoint::open(&output, CheckpointMode::Resume, source, options(32, 4)),
            Err(CheckpointError::Mismatch)
        ));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn older_manifest_versions_cannot_resume_incompatible_spools() {
        let output = output();
        let source = FileFingerprint {
            bytes: 123,
            hash: 456,
        };
        let checkpoint =
            Checkpoint::open(&output, CheckpointMode::Create, source, options(32, 2)).unwrap();
        let root = checkpoint.root().to_owned();
        drop(checkpoint);

        let manifest_path = root.join("manifest.txt");
        let content = fs::read_to_string(&manifest_path).unwrap();
        for version in [
            "OCTREE_CONVERSION_CHECKPOINT_V1",
            "OCTREE_CONVERSION_CHECKPOINT_V2",
            "OCTREE_CONVERSION_CHECKPOINT_V3",
            "OCTREE_CONVERSION_CHECKPOINT_V4",
        ] {
            fs::write(
                &manifest_path,
                content.replacen(MANIFEST_VERSION, version, 1),
            )
            .unwrap();
            assert!(matches!(
                Checkpoint::open(&output, CheckpointMode::Resume, source, options(32, 2)),
                Err(CheckpointError::Invalid(reason)) if reason == "unsupported manifest version"
            ));
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn resume_rejects_missing_or_different_output_identity() {
        let output = output();
        let source = FileFingerprint {
            bytes: 123,
            hash: 456,
        };
        let checkpoint =
            Checkpoint::open(&output, CheckpointMode::Create, source, options(32, 2)).unwrap();
        let root = checkpoint.root().to_owned();
        drop(checkpoint);

        let manifest_path = root.join("manifest.txt");
        let valid = fs::read_to_string(&manifest_path).unwrap();
        let invalid_manifests = [
            valid.replace("output_major=5\n", ""),
            valid.replace("output_major=5", "output_major=6"),
            valid.replace("output_revision=0", "output_revision=1"),
            valid.replace("header_envelope=1", "header_envelope=2"),
        ];
        for content in invalid_manifests {
            fs::write(&manifest_path, content).unwrap();
            assert!(matches!(
                Checkpoint::open(&output, CheckpointMode::Resume, source, options(32, 2)),
                Err(CheckpointError::Invalid(_))
            ));
        }
        fs::remove_dir_all(root).unwrap();
    }
}
