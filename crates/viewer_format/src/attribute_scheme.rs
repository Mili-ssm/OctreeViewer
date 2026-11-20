use std::fmt;
use std::str::FromStr;

use crate::EntropyCoder;

/// Experimental `v8.2-pre` policy: which entropy coder each `.octattrs` payload uses, by column
/// class. The file does not record it (every payload carries its own codec), so readers never
/// need it. Geometry (`.oct`) is always LZ4. The scheme disappears when the format freezes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AttributeCodecScheme {
    /// Every payload in LZ4, like `v8.1-pre`.
    #[default]
    Lz4,
    /// Zstd for the columns the render never reads.
    Cold,
    /// `Cold` plus `rgb`.
    ColdRgb,
    /// Zstd for every `.octattrs` payload, including the LOD display colour.
    All,
}

/// Columns the render reads on its hot path; any other name (the known cold LAS columns and the
/// extra bytes, whose names the converter chooses) counts as cold.
const HOT_COLUMNS: [&str; 2] = ["intensity", "classification"];
const DISPLAY_SOURCE_COLUMN: &str = "rgb";

impl AttributeCodecScheme {
    pub const ALL: [Self; 4] = [Self::Lz4, Self::Cold, Self::ColdRgb, Self::All];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Lz4 => "lz4",
            Self::Cold => "cold",
            Self::ColdRgb => "cold-rgb",
            Self::All => "all",
        }
    }

    /// Coder of one `.octattrs` payload: the stored display colour stream, or the schema column
    /// `column_name`. Cold columns are `return_number`, `number_of_returns`,
    /// `scan_angle_degrees`, `user_data`, `point_source_id`, `gps_time`, `nir` and the LAS extra
    /// bytes; a name not listed here is cold too, unless it is `intensity`, `classification` or
    /// `rgb`.
    pub fn coder_for(self, column_name: &str, is_display_colour: bool) -> EntropyCoder {
        let zstd = match self {
            Self::Lz4 => false,
            Self::All => true,
            Self::Cold | Self::ColdRgb => {
                if is_display_colour || HOT_COLUMNS.contains(&column_name) {
                    false
                } else if column_name == DISPLAY_SOURCE_COLUMN {
                    self == Self::ColdRgb
                } else {
                    true
                }
            }
        };
        if zstd {
            EntropyCoder::Zstd
        } else {
            EntropyCoder::Lz4
        }
    }
}

impl fmt::Display for AttributeCodecScheme {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for AttributeCodecScheme {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|scheme| scheme.as_str() == value)
            .ok_or_else(|| {
                format!("unknown attribute codec {value:?} (expected lz4, cold, cold-rgb or all)")
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const COLD: [&str; 8] = [
        "return_number",
        "number_of_returns",
        "scan_angle_degrees",
        "user_data",
        "point_source_id",
        "gps_time",
        "nir",
        "las_flags",
    ];

    fn zstd_columns(scheme: AttributeCodecScheme) -> Vec<&'static str> {
        let all = ["intensity", "classification", "rgb"]
            .into_iter()
            .chain(COLD);
        all.filter(|name| scheme.coder_for(name, false) == EntropyCoder::Zstd)
            .collect()
    }

    #[test]
    fn scheme_assigns_coders_by_column_class() {
        assert!(zstd_columns(AttributeCodecScheme::Lz4).is_empty());
        assert_eq!(zstd_columns(AttributeCodecScheme::Cold), COLD);
        let cold_rgb: Vec<_> = ["rgb"].into_iter().chain(COLD).collect();
        assert_eq!(zstd_columns(AttributeCodecScheme::ColdRgb), cold_rgb);
        let all: Vec<_> = ["intensity", "classification", "rgb"]
            .into_iter()
            .chain(COLD)
            .collect();
        assert_eq!(zstd_columns(AttributeCodecScheme::All), all);
        // The LOD display colour follows `all` only, and an unknown name counts as cold.
        for scheme in AttributeCodecScheme::ALL {
            assert_eq!(
                scheme.coder_for("", true) == EntropyCoder::Zstd,
                scheme == AttributeCodecScheme::All,
                "{scheme}"
            );
            assert_eq!(
                scheme.coder_for("my_extra_byte", false) == EntropyCoder::Zstd,
                scheme != AttributeCodecScheme::Lz4,
                "{scheme}"
            );
            assert_eq!(scheme.as_str().parse(), Ok(scheme));
        }
        assert!("zstd".parse::<AttributeCodecScheme>().is_err());
    }
}
