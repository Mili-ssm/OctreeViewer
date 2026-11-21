use std::{
    fmt,
    io::{Read, Seek, SeekFrom},
};

use crate::{
    FormatError, crc32,
    crc32::checksum_matches,
    layout::{HEADER_MAGIC, HEADER_SIZE, PROVISIONAL_HEADER_MAGIC},
};

/// Binary compatibility identity, independent of crate/build versions and checksums.
/// V8 is the only format this crate reads or writes; every other header identity is `Unknown`
/// (the retired families are named by `crate::retired`, not here).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FormatVersion {
    V8,
    Unknown { major: u16, revision: u32 },
}

impl FormatVersion {
    /// Revision written when the caller does not choose one.
    pub const DEFAULT_OUTPUT: Self = Self::V8;

    /// Every revision the writer implements, in user-facing listing order.
    pub const WRITABLE: [Self; 1] = [Self::V8];

    /// Row order the format fixes: `v8` always writes hybrid (ADR 0044).
    pub fn fixed_row_order(self) -> Option<crate::RowOrder> {
        (self == Self::V8).then_some(crate::RowOrder::Hybrid)
    }

    /// Reject unimplemented output formats before touching any output stream.
    pub fn ensure_writable(self) -> Result<(), FormatError> {
        crate::layout::StorageLayout::from_version(self).map(|_| ())
    }

    pub const fn major(self) -> u16 {
        match self {
            Self::V8 => 8,
            Self::Unknown { major, .. } => major,
        }
    }

    pub const fn revision(self) -> u32 {
        match self {
            Self::Unknown { revision, .. } => revision,
            Self::V8 => 0,
        }
    }
}

impl fmt::Display for FormatVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::V8 => f.write_str("v8"),
            Self::Unknown { major, revision } => write!(f, "unknown({major}, {revision})"),
        }
    }
}

impl std::str::FromStr for FormatVersion {
    type Err = &'static str;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "v8" => Ok(Self::V8),
            _ => Err("expected v8, the only format this version reads and writes"),
        }
    }
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
    {
        return Err(FormatError::InvalidHeader);
    }
    let version = match (major, revision) {
        (8, 0) => FormatVersion::V8,
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
            FormatVersion::Unknown {
                major: 6,
                revision: 0,
            },
            FormatVersion::Unknown {
                major: 8,
                revision: 1,
            },
        ] {
            assert!(version.ensure_writable().is_err(), "{version}");
        }
        assert!(FormatVersion::DEFAULT_OUTPUT.ensure_writable().is_ok());
        let names: Vec<String> = FormatVersion::WRITABLE
            .iter()
            .map(ToString::to_string)
            .collect();
        assert_eq!(names.join("|"), "v8");
    }

    #[test]
    fn v8_parses_prints_and_is_the_default_output() {
        let version: FormatVersion = "v8".parse().unwrap();
        assert_eq!(version, FormatVersion::V8);
        assert_eq!(version.to_string(), "v8");
        assert_eq!((version.major(), version.revision()), (8, 0));
        assert_eq!(FormatVersion::DEFAULT_OUTPUT, FormatVersion::V8);
        assert!(FormatVersion::WRITABLE.contains(&FormatVersion::V8));
        assert!(FormatVersion::V8.ensure_writable().is_ok());
    }

    #[test]
    fn retired_names_do_not_parse() {
        for name in ["v5", "v6", "v7", "v7.5-pre", "v8.1-pre", "v8.2-pre"] {
            assert!(name.parse::<FormatVersion>().is_err(), "{name}");
        }
    }

    #[test]
    fn v8_fixes_the_hybrid_row_order_and_unknown_identities_fix_none() {
        assert_eq!(
            FormatVersion::V8.fixed_row_order(),
            Some(crate::RowOrder::Hybrid)
        );
        let unknown = FormatVersion::Unknown {
            major: 8,
            revision: 3,
        };
        assert_eq!(unknown.fixed_row_order(), None);
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
    fn identifies_only_v8_and_reports_every_other_pair_as_unknown() {
        for (major, revision, label) in [
            (8, 0, "v8"),
            (5, 0, "unknown(5, 0)"),
            (6, 1, "unknown(6, 1)"),
            (7, 0, "unknown(7, 0)"),
            (7, 5, "unknown(7, 5)"),
            (8, 1, "unknown(8, 1)"),
            (8, 2, "unknown(8, 2)"),
            (9, 9, "unknown(9, 9)"),
        ] {
            let mut input = Cursor::new(header(major, revision));
            input.set_position(11);
            let identity = probe_format(&mut input).unwrap();
            assert_eq!(identity.version.to_string(), label);
            assert_eq!(
                label.parse::<FormatVersion>().is_ok(),
                identity.version == FormatVersion::V8
            );
            assert_eq!(identity.version.major(), major);
            assert_eq!(identity.version.revision(), revision);
            assert_eq!(input.position(), 11);
        }
    }

    #[test]
    fn rejects_corrupt_and_truncated_headers_and_restores_cursor() {
        let mut corrupt = header(8, 0);
        corrupt[16] ^= 1;
        // The reserved flag values and a header length other than 24 are refused even when the
        // checksum is valid.
        let resealed = |edit: fn(&mut Vec<u8>)| {
            let mut bytes = header(8, 0);
            edit(&mut bytes);
            let checksum = crc32(&bytes[..20]);
            bytes[20..24].copy_from_slice(&checksum.to_le_bytes());
            bytes
        };
        let reserved_flags = resealed(|bytes| bytes[12..16].copy_from_slice(&3_u32.to_le_bytes()));
        let wrong_length = resealed(|bytes| bytes[10..12].copy_from_slice(&32_u16.to_le_bytes()));
        for bytes in [corrupt, reserved_flags, wrong_length, vec![0; 10]] {
            let mut input = Cursor::new(bytes);
            input.set_position(3);
            assert!(probe_format(&mut input).is_err());
            assert_eq!(input.position(), 3);
        }
    }

    #[test]
    fn recognizes_roles_and_encodes_the_same_v8_header() {
        for (role, encoded) in [
            (FileRole::Monolithic, 0_u32),
            (FileRole::SplitPrimary, 1),
            (FileRole::Attributes, 2),
        ] {
            let identity = FormatIdentity {
                version: FormatVersion::V8,
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
        let mut bytes = header(8, 0);
        bytes[..8].copy_from_slice(PROVISIONAL_HEADER_MAGIC);
        let checksum = crc32(&bytes[..20]);
        bytes[20..24].copy_from_slice(&checksum.to_le_bytes());
        let error = probe_format(&mut Cursor::new(bytes)).unwrap_err();
        assert!(matches!(error, FormatError::ProvisionalFormat));
        assert!(error.to_string().contains("regenerate from LAS/LAZ"));
    }

    #[test]
    fn detection_never_enables_an_unimplemented_decoder() {
        let mut bytes = header(6, 5);
        bytes.resize(64, 0);
        assert!(matches!(
            crate::FormatReader::new(Cursor::new(bytes)),
            Err(FormatError::UnsupportedFormatVersion(
                FormatVersion::Unknown {
                    major: 6,
                    revision: 5
                }
            ))
        ));
    }
}
