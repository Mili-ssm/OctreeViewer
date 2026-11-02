//! RF-24 round trip: LAS flags and Extra Bytes survive `--format-version v7.5-pre` exactly
//! (no LAS exporter exists, so converter -> reader is the verification).

use std::{collections::BTreeMap, fs, path::Path};

use las::{Builder, Point, Vlr, Writer, point::Format};
use viewer_core::{AttributeValues, las::LasExtraBytesError};
use viewer_format::{FormatReader, FormatVersion};

use super::{ConvertError, ConvertOptions, convert_file};

fn options(version: FormatVersion) -> ConvertOptions {
    ConvertOptions::new(262_144, 0.01)
        .unwrap()
        .with_format_version(version)
        .unwrap()
}

fn descriptor(
    data_type: u8,
    options: u8,
    name: &str,
    scale: f64,
    offset: f64,
    no_data: i64,
) -> Vec<u8> {
    let mut record = vec![0_u8; 192];
    record[2] = data_type;
    record[3] = options;
    record[4..4 + name.len()].copy_from_slice(name.as_bytes());
    record[40..48].copy_from_slice(&no_data.to_le_bytes());
    for element in 0..3 {
        record[112 + 8 * element..120 + 8 * element].copy_from_slice(&scale.to_le_bytes());
        record[136 + 8 * element..144 + 8 * element].copy_from_slice(&offset.to_le_bytes());
    }
    record
}

/// u16 scaled with no-data (2 B), 2 x i32 array (8 B), f64 unscaled (8 B), i8 (1 B): 19 B.
fn descriptors() -> Vec<u8> {
    let mut data = descriptor(3, 1 | 8 | 16, "amplitude", 0.01, 5.0, 65_535);
    data.extend(descriptor(16, 0, "pair", 1.0, 0.0, 0));
    data.extend(descriptor(10, 0, "key", 1.0, 0.0, 0));
    data.extend(descriptor(2, 0, "small", 1.0, 0.0, 0));
    data
}

struct Expected {
    flags: u8,
    amplitude: u16,
    pair: [i32; 2],
    small: i8,
}

fn expected(index: usize, extended: bool) -> Expected {
    Expected {
        flags: (index % if extended { 16 } else { 8 }) as u8,
        amplitude: (index * 7 % 60_000) as u16,
        pair: [-(index as i32), index as i32 * 3],
        small: ((index % 200) as i16 - 100) as i8,
    }
}

fn extra_record(index: usize, extended: bool) -> Vec<u8> {
    let e = expected(index, extended);
    let mut bytes = e.amplitude.to_le_bytes().to_vec();
    bytes.extend(e.pair[0].to_le_bytes());
    bytes.extend(e.pair[1].to_le_bytes());
    bytes.extend((index as f64 + 0.25).to_le_bytes());
    bytes.push(e.small as u8);
    bytes
}

fn write_las(path: &Path, format_id: u8, count: usize, extra_vlr: Option<Vec<u8>>, extra_len: u16) {
    let extended = format_id >= 6;
    let mut builder = Builder::from(if extended { (1, 4) } else { (1, 2) });
    let mut format = Format::new(format_id).unwrap();
    format.extra_bytes = extra_len;
    builder.point_format = format;
    builder.transforms = las::Vector {
        x: las::Transform {
            scale: 0.001,
            offset: 0.0,
        },
        y: las::Transform {
            scale: 0.001,
            offset: 0.0,
        },
        z: las::Transform {
            scale: 0.001,
            offset: 0.0,
        },
    };
    if let Some(data) = extra_vlr {
        builder.vlrs.push(Vlr {
            user_id: "LASF_Spec".to_owned(),
            record_id: 4,
            description: String::new(),
            data,
        });
    }
    let mut writer = Writer::from_path(path, builder.into_header().unwrap()).unwrap();
    let mut state = 17_u64;
    let mut next = || {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((state >> 33) % 80_000) as f64 / 1000.0
    };
    for index in 0..count {
        let flags = expected(index, extended).flags;
        writer
            .write_point(Point {
                x: next(),
                y: next(),
                z: next(),
                is_synthetic: flags & 1 != 0,
                is_key_point: flags & 2 != 0,
                is_withheld: flags & 4 != 0,
                is_overlap: flags & 8 != 0,
                gps_time: format.has_gps_time.then_some(index as f64),
                color: format.has_color.then(|| las::Color::new(1, 2, 3)),
                extra_bytes: {
                    let mut bytes = extra_record(index, extended);
                    bytes.resize(usize::from(extra_len), 0);
                    bytes
                },
                ..Point::default()
            })
            .unwrap();
    }
    writer.close().unwrap();
}

fn open(output: &Path) -> FormatReader<fs::File> {
    let mut reader = FormatReader::new(fs::File::open(output).unwrap()).unwrap();
    reader
        .attach_attributes(fs::File::open(crate::publish::output_attributes_path(output)).unwrap())
        .unwrap();
    reader
}

fn column_index(reader: &FormatReader<fs::File>, name: &str) -> u32 {
    reader
        .format_metadata()
        .dataset()
        .hierarchy()
        .attributes()
        .iter()
        .position(|descriptor| descriptor.name() == name)
        .unwrap_or_else(|| panic!("column {name}")) as u32
}

/// Every row of every block (leaf and LOD) must carry the values of the source point it came from.
fn round_trip(format_id: u8) {
    let extended = format_id >= 6;
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("in.las");
    let count = 70_000;
    write_las(&input, format_id, count, Some(descriptors()), 19);
    let output = dir.path().join("out.oct");
    convert_file(&input, &output, options(FormatVersion::V7_PRE5)).unwrap();
    let mut reader = open(&output);

    let stored = reader.format_metadata().las_extra_bytes().to_vec();
    assert_eq!(
        stored,
        viewer_core::las::parse_extra_byte_descriptors(&descriptors()).unwrap()
    );
    assert!(stored[0].has_no_data() && stored[0].is_scaled());
    assert_eq!(stored[0].fields().scale[0], 0.01);
    assert_eq!(stored[0].fields().offset[0], 5.0);
    assert_eq!(stored[0].fields().no_data[0], 65_535_i64.to_le_bytes());
    let columns = [
        "las_flags",
        "extra:amplitude",
        "extra:pair",
        "extra:key",
        "extra:small",
    ]
    .map(|name| column_index(&reader, name));

    let hierarchy = reader.format_metadata().dataset().hierarchy();
    let blocks: Vec<_> = hierarchy
        .query(hierarchy.stats().root_bounds)
        .flat_map(|node| node.blocks().to_vec())
        .collect();
    let mut rows = 0;
    let mut sources = BTreeMap::<u64, ()>::new();
    for block in blocks {
        let [flags, amplitude, pair, key, small] =
            columns.map(|index| reader.read_attribute(block, index).unwrap());
        let (AttributeValues::U8(flags), AttributeValues::U16(amplitude)) =
            (flags.values(), amplitude.values())
        else {
            panic!("flags are u8 and amplitude u16");
        };
        let (AttributeValues::I32(pair), AttributeValues::F64(key), AttributeValues::I8(small)) =
            (pair.values(), key.values(), small.values())
        else {
            panic!("pair is i32, key f64, small i8");
        };
        assert_eq!(pair.len(), 2 * key.len());
        for row in 0..key.len() {
            let index = (key[row] - 0.25) as usize;
            assert_eq!(key[row], index as f64 + 0.25);
            let want = expected(index, extended);
            assert_eq!(flags[row], want.flags, "flags of point {index}");
            assert_eq!(amplitude[row], want.amplitude, "amplitude of point {index}");
            assert_eq!(
                pair[2 * row..2 * row + 2],
                want.pair,
                "pair of point {index}"
            );
            assert_eq!(small[row], want.small, "small of point {index}");
            sources.insert(index as u64, ());
            rows += 1;
        }
    }
    assert!(rows >= count, "{rows} rows");
    assert_eq!(
        sources.len(),
        count,
        "every source point is stored once at full resolution"
    );
    assert!(crate::verify_file(&output).is_ok());
}

#[test]
fn legacy_point_format_round_trips_flags_and_extra_bytes() {
    round_trip(3);
}

#[test]
fn extended_point_format_round_trips_flags_and_extra_bytes() {
    round_trip(6);
}

#[test]
fn flags_without_extra_bytes_are_kept_and_older_formats_are_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("in.las");
    write_las(&input, 1, 3000, None, 0);
    let paged = dir.path().join("p.oct");
    convert_file(&input, &paged, options(FormatVersion::V7_PRE5)).unwrap();
    let mut reader = open(&paged);
    assert!(reader.format_metadata().las_extra_bytes().is_empty());
    let hierarchy = reader.format_metadata().dataset().hierarchy();
    let block = hierarchy.node(hierarchy.root_id()).unwrap().blocks()[0];
    let last = hierarchy.attributes().last().unwrap().name().to_owned();
    assert_eq!(last, "las_flags");
    let flags = column_index(&reader, "las_flags");
    let AttributeValues::U8(values) = reader
        .read_attribute(block, flags)
        .unwrap()
        .values()
        .clone()
    else {
        panic!("u8 flags");
    };
    // Format 1 has no overlap bit in the classification byte.
    assert!(values.iter().all(|v| *v < 8));
    assert!(values.iter().any(|v| *v & 4 != 0));

    // The same file with extra bytes in v7.4-pre has the pre-RF-24 schema.
    let extras = dir.path().join("e.las");
    write_las(&extras, 3, 3000, Some(descriptors()), 19);
    for version in [FormatVersion::V7_PRE4] {
        let output = dir.path().join("old.oct");
        convert_file(&extras, &output, options(version)).unwrap();
        let reader = open(&output);
        let names: Vec<_> = reader
            .format_metadata()
            .dataset()
            .hierarchy()
            .attributes()
            .iter()
            .map(|a| a.name().to_owned())
            .collect();
        assert_eq!(
            names,
            viewer_core::las::las_attribute_schema(true, true, false)
                .iter()
                .map(|a| a.name().to_owned())
                .collect::<Vec<_>>()
        );
    }
}

#[test]
fn unsupported_or_inconsistent_extra_bytes_are_explicit_errors() {
    let dir = tempfile::tempdir().unwrap();
    let convert = |name: &str, vlr: Option<Vec<u8>>, extra_len: u16| {
        let input = dir.path().join(format!("{name}.las"));
        write_las(&input, 0, 100, vlr, extra_len);
        convert_file(
            &input,
            &dir.path().join(format!("{name}.oct")),
            options(FormatVersion::V7_PRE5),
        )
    };
    for data_type in [0_u8, 31, 255] {
        assert!(matches!(
            convert("t", Some(descriptor(data_type, 0, "x", 1.0, 0.0, 0)), 1),
            Err(ConvertError::LasExtraBytes(
                LasExtraBytesError::UnsupportedType { .. }
            ))
        ));
    }
    // Extra bytes in the record without descriptors, or descriptors of another size.
    assert!(matches!(
        convert("u", None, 4),
        Err(ConvertError::LasExtraBytes(
            LasExtraBytesError::SizeMismatch { .. }
        ))
    ));
    assert!(matches!(
        convert("m", Some(descriptor(3, 0, "x", 1.0, 0.0, 0)), 1),
        Err(ConvertError::LasExtraBytes(
            LasExtraBytesError::SizeMismatch { .. }
        ))
    ));
    // 8 base columns (7 + flags) + 249 one-byte attributes > 256.
    let many = descriptor(1, 0, "b", 1.0, 0.0, 0).repeat(249);
    assert!(matches!(
        convert("n", Some(many), 249),
        Err(ConvertError::LasExtraBytes(
            LasExtraBytesError::TooManyAttributes { count: 257 }
        ))
    ));
    // The same descriptors are fine for a format that drops Extra Bytes.
    let input = dir.path().join("u.las");
    convert_file(
        &input,
        &dir.path().join("old.oct"),
        options(FormatVersion::V7_PRE4),
    )
    .unwrap();
}

/// The web converter runs the serial core path: its bytes must equal the CLI's for a file with
/// flags and Extra Bytes of both point format families.
#[test]
fn serial_core_matches_native_with_flags_and_extra_bytes() {
    let dir = tempfile::tempdir().unwrap();
    for format_id in [3_u8, 6] {
        let input = dir.path().join(format!("f{format_id}.las"));
        write_las(&input, format_id, 5000, Some(descriptors()), 19);
        let options = options(FormatVersion::V7_PRE5);
        crate::tests::assert_serial_matches_native(
            &input,
            options,
            false,
            &format!("format {format_id}"),
        );
    }
}
