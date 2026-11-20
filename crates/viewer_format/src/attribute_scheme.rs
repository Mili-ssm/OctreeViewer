use std::fmt;
use std::str::FromStr;

use viewer_core::las::extra::{EXTRA_COLUMN_PREFIX, LAS_FLAGS_NAME};

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

/// Columns the render reads on its hot path.
const HOT_COLUMNS: [&str; 2] = ["intensity", "classification"];
const DISPLAY_SOURCE_COLUMN: &str = "rgb";
/// Known LAS columns no render path reads. The LAS extra bytes (`extra:` prefix) and `las_flags`
/// are cold too; see [`is_cold_column`].
const COLD_COLUMNS: [&str; 7] = [
    "return_number",
    "number_of_returns",
    "scan_angle_degrees",
    "user_data",
    "point_source_id",
    "gps_time",
    "nir",
];

/// Whether `name` is a cold column: a known cold LAS column, `las_flags` or an extra-bytes column.
/// A name outside every known class (not produced by the LAS schema today) is also cold: the
/// render only reads the hot columns, so zstd is the safe default.
fn is_cold_column(name: &str) -> bool {
    COLD_COLUMNS.contains(&name)
        || name == LAS_FLAGS_NAME
        || name.starts_with(EXTRA_COLUMN_PREFIX)
        || !(HOT_COLUMNS.contains(&name) || name == DISPLAY_SOURCE_COLUMN)
}

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
    /// `column_name`. Hot columns are `intensity` and `classification`, `rgb` is the display
    /// source; every other column (`return_number`, `number_of_returns`, `scan_angle_degrees`,
    /// `user_data`, `point_source_id`, `gps_time`, `nir`, `las_flags`, the `extra:` columns) is cold.
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
                    is_cold_column(column_name)
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
    use viewer_core::las::extra::{
        LasExtraByte, LasExtraByteFields, LasSchemaSpec, las_attribute_schema_for,
    };
    use viewer_core::las::las_attribute_schema;

    use super::*;

    /// Column names of the real LAS schemas (all optional columns, `las_flags`, one extra byte),
    /// so a rename in `viewer_core` breaks this test.
    fn schema_names() -> Vec<String> {
        let mut names: Vec<String> = las_attribute_schema(true, true, true)
            .iter()
            .map(|d| d.name().to_owned())
            .collect();
        let extra = LasExtraByte::new(
            "amplitude",
            LasExtraByteFields {
                las_type: 3,
                options: 0,
                no_data: [[0; 8]; 3],
                scale: [1.0; 3],
                offset: [0.0; 3],
            },
        )
        .unwrap();
        let spec = LasSchemaSpec {
            has_gps_time: true,
            has_color: true,
            has_nir: true,
            las_flags: true,
            extra_bytes: &[extra],
        };
        for d in las_attribute_schema_for(&spec).unwrap() {
            if !names.iter().any(|n| n == d.name()) {
                names.push(d.name().to_owned());
            }
        }
        names
    }

    fn zstd_columns(scheme: AttributeCodecScheme) -> Vec<String> {
        schema_names()
            .into_iter()
            .filter(|name| scheme.coder_for(name, false) == EntropyCoder::Zstd)
            .collect()
    }

    #[test]
    fn scheme_assigns_coders_by_column_class() {
        let names = schema_names();
        assert!(names.iter().any(|n| n == "las_flags"));
        assert!(names.iter().any(|n| n == "extra:amplitude"));
        let cold: Vec<String> = names
            .iter()
            .filter(|n| !["intensity", "classification", "rgb"].contains(&n.as_str()))
            .cloned()
            .collect();
        assert_eq!(cold.len(), 9);
        assert!(zstd_columns(AttributeCodecScheme::Lz4).is_empty());
        assert_eq!(zstd_columns(AttributeCodecScheme::Cold), cold);
        let cold_rgb: Vec<String> = names
            .iter()
            .filter(|n| cold.contains(n) || *n == "rgb")
            .cloned()
            .collect();
        assert_eq!(zstd_columns(AttributeCodecScheme::ColdRgb), cold_rgb);
        assert_eq!(zstd_columns(AttributeCodecScheme::All), names);
        // The LOD display colour follows `all` only, and an unknown name counts as cold.
        for scheme in AttributeCodecScheme::ALL {
            assert_eq!(
                scheme.coder_for("", true) == EntropyCoder::Zstd,
                scheme == AttributeCodecScheme::All,
                "{scheme}"
            );
            assert_eq!(
                scheme.coder_for("unknown_name", false) == EntropyCoder::Zstd,
                scheme != AttributeCodecScheme::Lz4,
                "{scheme}"
            );
            assert_eq!(scheme.as_str().parse(), Ok(scheme));
        }
        assert!("zstd".parse::<AttributeCodecScheme>().is_err());
    }
}
