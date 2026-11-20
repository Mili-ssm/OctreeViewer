use std::{
    fmt,
    io::{Read, Seek, SeekFrom},
    num::NonZeroU32,
};

use crate::{
    FormatError, crc32,
    crc32::checksum_matches,
    layout::{HEADER_MAGIC, HEADER_SIZE, PROVISIONAL_HEADER_MAGIC},
};

/// Binary compatibility identity, independent of crate/build versions and checksums.
/// Pre-release revisions are exact identities, never an implicit compatibility range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FormatVersion {
    V5,
    V6Pre(NonZeroU32),
    V6,
    V7Pre(NonZeroU32),
    V7,
    /// Experimental v8 revisions (`v8.N-pre`).
    V8Pre(NonZeroU32),
    Unknown {
        major: u16,
        revision: u32,
    },
}

impl FormatVersion {
    /// First experimental v6 payload layout. Final v6 remains unsupported.
    pub const V6_PRE1: Self = Self::V6Pre(NonZeroU32::new(1).expect("nonzero revision"));

    pub const V6_PRE2: Self = Self::V6Pre(NonZeroU32::new(2).expect("nonzero revision"));

    pub const V6_PRE3: Self = Self::V6Pre(NonZeroU32::new(3).expect("nonzero revision"));
    pub const V6_PRE4: Self = Self::V6Pre(NonZeroU32::new(4).expect("nonzero revision"));
    pub const V7_PRE1: Self = Self::V7Pre(NonZeroU32::new(1).expect("nonzero revision"));
    pub const V7_PRE2: Self = Self::V7Pre(NonZeroU32::new(2).expect("nonzero revision"));
    pub const V7_PRE3: Self = Self::V7Pre(NonZeroU32::new(3).expect("nonzero revision"));
    /// Global integer octree frame, node prefixes and CRS record (R4).
    pub const V7_PRE4: Self = Self::V7Pre(NonZeroU32::new(4).expect("nonzero revision"));
    /// Paged hierarchy: root page next to the footer, child pages on demand (R5).
    pub const V7_PRE5: Self = Self::V7Pre(NonZeroU32::new(5).expect("nonzero revision"));

    /// First experimental v8 revision: the v7 layout plus the planned v8 payload changes.
    pub const V8_PRE1: Self = Self::V8Pre(NonZeroU32::new(1).expect("nonzero revision"));

    /// Revision written when the caller does not choose one.
    pub const DEFAULT_OUTPUT: Self = Self::V7;

    /// Every revision the writer implements, in user-facing listing order.
    pub const WRITABLE: [Self; 12] = [
        Self::V7_PRE1,
        Self::V7_PRE2,
        Self::V7_PRE3,
        Self::V5,
        Self::V6_PRE1,
        Self::V6_PRE2,
        Self::V6_PRE3,
        Self::V6_PRE4,
        Self::V7_PRE4,
        Self::V7_PRE5,
        Self::V7,
        Self::V8_PRE1,
    ];

    /// Whether block-level EPT checkpoint/resume can target this revision.
    pub fn supports_block_checkpoint(self) -> bool {
        crate::layout::StorageLayout::from_version(self)
            .is_ok_and(crate::layout::StorageLayout::supports_block_checkpoint)
    }

    /// Whether the writer needs a [`crate::GlobalFrame`] in the metadata (the global integer
    /// octree of `v7.4-pre`) and `i64` block origins instead of per-block `f64` transforms.
    pub fn uses_global_frame(self) -> bool {
        crate::layout::StorageLayout::from_version(self)
            .is_ok_and(crate::layout::StorageLayout::uses_global_frame)
    }

    /// Whether the index is a metadata block plus a root page with child pages (`v7.5-pre` and `v7`).
    pub fn uses_pages(self) -> bool {
        crate::layout::StorageLayout::from_version(self)
            .is_ok_and(crate::layout::StorageLayout::uses_pages)
    }

    /// Whether the writer can produce the hybrid row order (Morton re-sort) for this revision.
    pub fn supports_hybrid_rows(self) -> bool {
        crate::layout::StorageLayout::from_version(self)
            .is_ok_and(crate::layout::StorageLayout::supports_hybrid_rows)
    }

    /// Whether the display colour is derived from stored data instead of stored twice (`v8.N-pre`).
    pub fn derives_display_attribute(self) -> bool {
        crate::layout::StorageLayout::from_version(self)
            .is_ok_and(crate::layout::StorageLayout::derives_display_attribute)
    }

    /// Whether interior LOD nodes omit the attribute columns the viewer does not draw (`v8.N-pre`).
    pub fn omits_lod_attributes(self) -> bool {
        crate::layout::StorageLayout::from_version(self)
            .is_ok_and(crate::layout::StorageLayout::omits_lod_attributes)
    }

    /// Whether nodes store nearest-neighbour distance quantiles (`v8.N-pre`).
    pub fn stores_distance_quantiles(self) -> bool {
        crate::layout::StorageLayout::from_version(self)
            .is_ok_and(crate::layout::StorageLayout::stores_distance_quantiles)
    }

    /// Row order the format fixes: `v7` and `v8.N-pre` always write hybrid (ADR 0044); every other revision
    /// leaves the choice to the caller (`Source` by default).
    pub const fn fixed_row_order(self) -> Option<crate::RowOrder> {
        match self {
            Self::V7 | Self::V8Pre(_) => Some(crate::RowOrder::Hybrid),
            _ => None,
        }
    }

    /// Reject unimplemented output formats before touching any output stream.
    pub fn ensure_writable(self) -> Result<(), FormatError> {
        crate::layout::StorageLayout::from_version(self).map(|_| ())
    }

    pub const fn major(self) -> u16 {
        match self {
            Self::V5 => 5,
            Self::V6Pre(_) | Self::V6 => 6,
            Self::V7Pre(_) | Self::V7 => 7,
            Self::V8Pre(_) => 8,
            Self::Unknown { major, .. } => major,
        }
    }

    pub const fn revision(self) -> u32 {
        match self {
            Self::V6Pre(revision) | Self::V7Pre(revision) | Self::V8Pre(revision) => revision.get(),
            Self::Unknown { revision, .. } => revision,
            Self::V5 | Self::V6 | Self::V7 => 0,
        }
    }
}

impl fmt::Display for FormatVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::V5 => f.write_str("v5"),
            Self::V6 => f.write_str("v6"),
            Self::V6Pre(revision) => write!(f, "v6.{revision}-pre"),
            Self::V7 => f.write_str("v7"),
            Self::V7Pre(revision) => write!(f, "v7.{revision}-pre"),
            Self::V8Pre(revision) => write!(f, "v8.{revision}-pre"),
            Self::Unknown { major, revision } => write!(f, "unknown({major}, {revision})"),
        }
    }
}

impl std::str::FromStr for FormatVersion {
    type Err = &'static str;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "v5" => Ok(Self::V5),
            "v6" => Ok(Self::V6),
            "v7" => Ok(Self::V7),
            _ => value
                .strip_prefix("v6.")
                .and_then(|v| v.strip_suffix("-pre"))
                .and_then(|v| v.parse::<NonZeroU32>().ok())
                .map(Self::V6Pre)
                .or_else(|| {
                    value
                        .strip_prefix("v7.")
                        .and_then(|v| v.strip_suffix("-pre"))
                        .and_then(|v| v.parse::<NonZeroU32>().ok())
                        .map(Self::V7Pre)
                })
                .or_else(|| {
                    value
                        .strip_prefix("v8.")
                        .and_then(|v| v.strip_suffix("-pre"))
                        .and_then(|v| v.parse::<NonZeroU32>().ok())
                        .map(Self::V8Pre)
                })
                .ok_or("expected v5, v6, v7 or a positive pre-release revision"),
        }
    }
}

/// Writable revisions that support block-level EPT checkpoint/resume, in listing order.
pub fn block_checkpoint_versions() -> impl Iterator<Item = FormatVersion> {
    FormatVersion::WRITABLE
        .into_iter()
        .filter(|version| version.supports_block_checkpoint())
}

/// Recognized header identity does not imply that a payload decoder is available.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileRole {
    Monolithic,
    SplitPrimary,
    Attributes,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FormatIdentity {
    pub version: FormatVersion,
    pub role: FileRole,
}

impl FormatIdentity {
    /// Encode the common format header envelope. This does not encode a payload.
    pub fn to_header_bytes(self) -> Result<[u8; HEADER_SIZE as usize], FormatError> {
        let mut header = [0; HEADER_SIZE as usize];
        header[..8].copy_from_slice(HEADER_MAGIC);
        header[8..10].copy_from_slice(&self.version.major().to_le_bytes());
        header[10..12].copy_from_slice(&(HEADER_SIZE as u16).to_le_bytes());
        let role: u32 = match self.role {
            FileRole::Monolithic => 0,
            FileRole::SplitPrimary => 1,
            FileRole::Attributes => 2,
        };
        header[12..16].copy_from_slice(&role.to_le_bytes());
        header[16..20].copy_from_slice(&self.version.revision().to_le_bytes());
        let checksum = crc32(&header[..20]);
        header[20..24].copy_from_slice(&checksum.to_le_bytes());
        if matches!(self.version, FormatVersion::Unknown { .. }) {
            return Err(FormatError::UnsupportedFormatVersion(self.version));
        }
        Ok(header)
    }
}

/// Inspect only the common 24-byte envelope, restoring the cursor even on parse failure.
/// No index/payload is read. A recognized identity can still lack a payload decoder.
pub fn probe_format<R: Read + Seek>(reader: &mut R) -> Result<FormatIdentity, FormatError> {
    let position = reader.stream_position()?;
    let result = (|| {
        reader.seek(SeekFrom::Start(0))?;
        let mut header = [0; HEADER_SIZE as usize];
        reader.read_exact(&mut header)?;
        parse_identity(&header)
    })();
    reader.seek(SeekFrom::Start(position))?;
    result
}

pub(crate) fn parse_identity(
    header: &[u8; HEADER_SIZE as usize],
) -> Result<FormatIdentity, FormatError> {
    if &header[..8] == PROVISIONAL_HEADER_MAGIC {
        return Err(FormatError::ProvisionalFormat);
    }
    let major = u16::from_le_bytes(header[8..10].try_into().expect("fixed width"));
    let length = u16::from_le_bytes(header[10..12].try_into().expect("fixed width"));
    let flags = u32::from_le_bytes(header[12..16].try_into().expect("fixed width"));
    let revision = u32::from_le_bytes(header[16..20].try_into().expect("fixed width"));
    let checksum = u32::from_le_bytes(header[20..24].try_into().expect("fixed width"));
    if &header[..8] != HEADER_MAGIC
        || u64::from(length) != HEADER_SIZE
        || !matches!(flags, 0..=2)
        || !checksum_matches(&header[..20], checksum)
        || (major == 5 && revision != 0)
    {
        return Err(FormatError::InvalidHeader);
    }
    let version = match (major, NonZeroU32::new(revision)) {
        (5, None) => FormatVersion::V5,
        (6, None) => FormatVersion::V6,
        (6, Some(revision)) => FormatVersion::V6Pre(revision),
        (7, None) => FormatVersion::V7,
        (7, Some(revision)) => FormatVersion::V7Pre(revision),
        (8, Some(revision)) => FormatVersion::V8Pre(revision),
        _ => FormatVersion::Unknown { major, revision },
    };
    let role = match flags {
        0 => FileRole::Monolithic,
        1 => FileRole::SplitPrimary,
        2 => FileRole::Attributes,
        _ => unreachable!(),
    };
    Ok(FormatIdentity { version, role })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn header(major: u16, revision: u32) -> Vec<u8> {
        let mut bytes = vec![0; 24];
        bytes[..8].copy_from_slice(HEADER_MAGIC);
        bytes[8..10].copy_from_slice(&major.to_le_bytes());
        bytes[10..12].copy_from_slice(&24_u16.to_le_bytes());
        bytes[16..20].copy_from_slice(&revision.to_le_bytes());
        let checksum = crc32(&bytes[..20]);
        bytes[20..24].copy_from_slice(&checksum.to_le_bytes());
        bytes
    }

    #[test]
    fn writable_versions_are_exactly_the_implemented_layouts() {
        for version in FormatVersion::WRITABLE {
            assert!(version.ensure_writable().is_ok(), "{version}");
        }
        for version in [
            FormatVersion::V6,
            FormatVersion::V6Pre(NonZeroU32::new(5).unwrap()),
            FormatVersion::V7Pre(NonZeroU32::new(6).unwrap()),
        ] {
            assert!(version.ensure_writable().is_err(), "{version}");
        }
        assert!(FormatVersion::DEFAULT_OUTPUT.ensure_writable().is_ok());
        let names: Vec<String> = FormatVersion::WRITABLE
            .iter()
            .map(ToString::to_string)
            .collect();
        assert_eq!(
            names.join("|"),
            "v7.1-pre|v7.2-pre|v7.3-pre|v5|v6.1-pre|v6.2-pre|v6.3-pre|v6.4-pre|v7.4-pre|v7.5-pre|v7|v8.1-pre"
        );
    }

    #[test]
    fn v8_pre1_parses_and_prints() {
        let version: FormatVersion = "v8.1-pre".parse().unwrap();
        assert_eq!(version, FormatVersion::V8_PRE1);
        assert_eq!(version.to_string(), "v8.1-pre");
        assert!("v8".parse::<FormatVersion>().is_err());
    }

    #[test]
    fn v8_pre1_is_writable() {
        assert!(FormatVersion::WRITABLE.contains(&FormatVersion::V8_PRE1));
        assert!(FormatVersion::V8_PRE1.ensure_writable().is_ok());
        assert!(
            FormatVersion::V8Pre(NonZeroU32::new(2).unwrap())
                .ensure_writable()
                .is_err()
        );
    }

    #[test]
    fn unknown_major_nine_is_rejected() {
        let identity = probe_format(&mut Cursor::new(header(9, 1))).unwrap();
        assert_eq!(
            identity.version,
            FormatVersion::Unknown {
                major: 9,
                revision: 1
            }
        );
        assert!(identity.version.ensure_writable().is_err());
    }

    #[test]
    fn block_checkpoint_is_supported_only_by_v7_pre1_to_3() {
        let supported: Vec<FormatVersion> = FormatVersion::WRITABLE
            .into_iter()
            .filter(|version| version.supports_block_checkpoint())
            .collect();
        assert_eq!(
            supported,
            [
                FormatVersion::V7_PRE1,
                FormatVersion::V7_PRE2,
                FormatVersion::V7_PRE3
            ]
        );
        assert!(!FormatVersion::V7.supports_block_checkpoint());
    }

    #[test]
    fn identifies_exact_revisions_without_consuming_the_stream() {
        for (major, revision, label) in [
            (5, 0, "v5"),
            (6, 1, "v6.1-pre"),
            (6, 2, "v6.2-pre"),
            (6, 0, "v6"),
            (7, 1, "v7.1-pre"),
            (7, 2, "v7.2-pre"),
            (7, 3, "v7.3-pre"),
            (7, 4, "v7.4-pre"),
            (7, 5, "v7.5-pre"),
            (7, 0, "v7"),
            (8, 1, "v8.1-pre"),
            (9, 9, "unknown(9, 9)"),
        ] {
            let mut input = Cursor::new(header(major, revision));
            input.set_position(11);
            let identity = probe_format(&mut input).unwrap();
            assert_eq!(identity.version.to_string(), label);
            if major <= 8 {
                assert_eq!(label.parse::<FormatVersion>().unwrap(), identity.version);
            }
            assert_eq!(identity.version.major(), major);
            assert_eq!(identity.version.revision(), revision);
            assert_eq!(input.position(), 11);
        }
    }

    #[test]
    fn rejects_corrupt_reserved_and_truncated_headers_and_restores_cursor() {
        let mut corrupt = header(6, 1);
        corrupt[16] ^= 1;
        for bytes in [corrupt, header(5, 1), vec![0; 10]] {
            let mut input = Cursor::new(bytes);
            input.set_position(3);
            assert!(probe_format(&mut input).is_err());
            assert_eq!(input.position(), 3);
        }
    }

    #[test]
    fn recognizes_roles_and_encodes_the_same_v5_header() {
        for (role, encoded) in [
            (FileRole::Monolithic, 0_u32),
            (FileRole::SplitPrimary, 1),
            (FileRole::Attributes, 2),
        ] {
            let identity = FormatIdentity {
                version: FormatVersion::V5,
                role,
            };
            let bytes = identity.to_header_bytes().unwrap();
            assert_eq!(
                u32::from_le_bytes(bytes[12..16].try_into().unwrap()),
                encoded
            );
            assert_eq!(parse_identity(&bytes).unwrap(), identity);
        }
    }

    #[test]
    fn identifies_provisional_magic_for_regeneration() {
        let mut bytes = header(5, 0);
        bytes[..8].copy_from_slice(PROVISIONAL_HEADER_MAGIC);
        let checksum = crc32(&bytes[..20]);
        bytes[20..24].copy_from_slice(&checksum.to_le_bytes());
        let error = probe_format(&mut Cursor::new(bytes)).unwrap_err();
        assert!(matches!(error, FormatError::ProvisionalFormat));
        assert!(error.to_string().contains("regenerate from LAS/LAZ/EPT"));
    }

    #[test]
    fn detection_never_enables_an_unimplemented_decoder() {
        let mut bytes = header(6, 5);
        bytes.resize(64, 0);
        assert!(matches!(crate::FormatReader::new(Cursor::new(bytes)),
            Err(FormatError::UnsupportedFormatVersion(FormatVersion::V6Pre(revision)))
                if revision.get() == 5));
    }
}
