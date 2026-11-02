use std::{fs, process::Command};

use las::{Builder, Point, Writer, point::Format};
use viewer_core::HierarchySemantics;
use viewer_format::FormatReader;

#[test]
fn ept_cli_preserves_implicit_workers_and_rejects_explicit_repartitioning() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_owned();
    fs::create_dir_all(root.join("ept-data")).unwrap();
    fs::create_dir_all(root.join("ept-hierarchy")).unwrap();
    fs::write(
        root.join("ept.json"),
        r#"{"dataType":"laszip","hierarchyType":"json","points":1,"bounds":[0,0,0,8,8,8],"schema":[{"name":"X","type":"signed","size":4},{"name":"Y","type":"signed","size":4},{"name":"Z","type":"signed","size":4}]}"#,
    )
    .unwrap();
    fs::write(root.join("ept-hierarchy/0-0-0-0.json"), r#"{"0-0-0-0":1}"#).unwrap();
    let mut builder = Builder::from((1, 2));
    builder.point_format = Format::new(0).unwrap();
    builder.point_format.is_compressed = true;
    let mut writer = Writer::from_path(
        root.join("ept-data/0-0-0-0.laz"),
        builder.into_header().unwrap(),
    )
    .unwrap();
    writer
        .write_point(Point {
            x: 1.0,
            y: 2.0,
            z: 3.0,
            ..Point::default()
        })
        .unwrap();
    writer.close().unwrap();

    let output_path = root.join("result.oct");
    let output = Command::new(env!("CARGO_BIN_EXE_viewer_convert"))
        .arg(&root)
        .arg(&output_path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let reader = FormatReader::new(fs::File::open(&output_path).unwrap()).unwrap();
    assert_eq!(
        reader.format_metadata().dataset().hierarchy().semantics(),
        HierarchySemantics::Additive
    );
    drop(reader);

    let rejected_path = root.join("rejected.oct");
    let rejected = Command::new(env!("CARGO_BIN_EXE_viewer_convert"))
        .arg(&root)
        .arg(&rejected_path)
        .args(["--spatial-workers", "2"])
        .output()
        .unwrap();
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("EPT topology is preserved"));
    assert!(!rejected_path.exists());
}

#[test]
fn verify_subcommand_accepts_a_paged_file_and_rejects_a_corrupt_page() {
    let tmp = tempfile::tempdir().unwrap();
    let input = tmp.path().join("in.las");
    let mut builder = Builder::from((1, 2));
    builder.point_format = Format::new(0).unwrap();
    let mut writer = Writer::from_path(&input, builder.into_header().unwrap()).unwrap();
    for index in 0..2000_u32 {
        writer
            .write_point(Point {
                x: f64::from(index % 40),
                y: f64::from(index / 40),
                z: f64::from(index % 7),
                ..Point::default()
            })
            .unwrap();
    }
    writer.close().unwrap();
    let output = tmp.path().join("out.oct");
    let converted = Command::new(env!("CARGO_BIN_EXE_viewer_convert"))
        .arg(&input)
        .arg(&output)
        .args(["--format-version", "v7.5-pre"])
        .output()
        .unwrap();
    assert!(
        converted.status.success(),
        "{}",
        String::from_utf8_lossy(&converted.stderr)
    );
    assert!(!tmp.path().join("out.octattrs").exists());

    let verify = |path: &std::path::Path| {
        Command::new(env!("CARGO_BIN_EXE_viewer_convert"))
            .arg("verify")
            .arg(path)
            .output()
            .unwrap()
    };
    let ok = verify(&output);
    assert_eq!(ok.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&ok.stdout).starts_with("OK v7.5-pre"));

    // The root page sits just before the 40-byte footer: damaging it breaks the index CRC.
    let mut bytes = fs::read(&output).unwrap();
    let at = bytes.len() - 60;
    bytes[at] ^= 0xff;
    let corrupt = tmp.path().join("corrupt.oct");
    fs::write(&corrupt, bytes).unwrap();
    let bad = verify(&corrupt);
    assert_eq!(bad.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&bad.stderr).contains("verification of"));
}
