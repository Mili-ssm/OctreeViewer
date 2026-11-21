use std::{fs, process::Command};

use las::{Builder, Point, Writer, point::Format};
use viewer_format::{FormatReader, OctreeReader};

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
    assert!(tmp.path().join("out.octattrs").exists());

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

    // A companion that does not belong to the index is refused too.
    let mut attributes = fs::read(tmp.path().join("out.octattrs")).unwrap();
    let crc = attributes.len() - 8;
    attributes[crc] ^= 0xff;
    fs::write(tmp.path().join("corrupt.octattrs"), attributes).unwrap();
    fs::copy(&output, tmp.path().join("other.oct")).unwrap();
    fs::copy(
        tmp.path().join("corrupt.octattrs"),
        tmp.path().join("other.octattrs"),
    )
    .unwrap();
    let mismatched = verify(&tmp.path().join("other.oct"));
    assert_eq!(mismatched.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&mismatched.stderr).contains("companion"));
}

fn small_las(path: &std::path::Path) {
    las_with_points(path, 500);
}

fn las_with_points(path: &std::path::Path, count: u32) {
    let mut builder = Builder::from((1, 2));
    builder.point_format = Format::new(0).unwrap();
    let mut writer = Writer::from_path(path, builder.into_header().unwrap()).unwrap();
    for index in 0..count {
        writer
            .write_point(Point {
                x: f64::from(index % 200),
                y: f64::from(index / 200),
                z: f64::from(index % 5),
                ..Point::default()
            })
            .unwrap();
    }
    writer.close().unwrap();
}

#[test]
fn format_fixed_options_are_checked_independently_of_flag_order() {
    let tmp = tempfile::tempdir().unwrap();
    let input = tmp.path().join("in.las");
    small_las(&input);
    let run = |name: &str, args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_viewer_convert"))
            .arg(&input)
            .arg(tmp.path().join(name))
            .args(args)
            .output()
            .unwrap()
    };
    let rejected = run(
        "v7.oct",
        &["--format-version", "v7", "--row-order", "source"],
    );
    assert!(!rejected.status.success());
    assert!(
        String::from_utf8_lossy(&rejected.stderr)
            .contains("row order is fixed by the output format"),
        "{}",
        String::from_utf8_lossy(&rejected.stderr)
    );
    assert!(!tmp.path().join("v7.oct").exists());

    for (name, args) in [
        (
            "a.oct",
            ["--spatial-divisions", "4", "--format-version", "v5"],
        ),
        (
            "b.oct",
            ["--format-version", "v5", "--spatial-divisions", "4"],
        ),
    ] {
        let ok = run(name, &args);
        assert!(
            ok.status.success(),
            "{}",
            String::from_utf8_lossy(&ok.stderr)
        );
    }
    assert_eq!(
        fs::read(tmp.path().join("a.oct")).unwrap(),
        fs::read(tmp.path().join("b.oct")).unwrap()
    );
}

fn run_cli(args: &[&std::path::Path], flags: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_viewer_convert"))
        .args(args)
        .args(flags)
        .output()
        .unwrap()
}

fn stderr(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

#[test]
fn cli_refuses_the_removed_attr_codec_option() {
    let tmp = tempfile::tempdir().unwrap();
    let input = tmp.path().join("in.las");
    small_las(&input);
    for (name, flags) in [
        ("lz4.oct", vec!["--attr-codec", "lz4"]),
        ("cold.oct", vec!["--attr-codec", "cold"]),
        ("bare.oct", vec!["--attr-codec"]),
    ] {
        let output = tmp.path().join(name);
        let run = run_cli(&[&input, &output], &flags);
        assert!(!run.status.success(), "{name}");
        assert!(!output.exists(), "{name} left an output");
        let message = stderr(&run);
        assert!(
            message.contains("--attr-codec") && message.contains("removed (ADR 0060)"),
            "{name}: {message}"
        );
    }
}

#[test]
fn cli_refuses_the_retired_v8_pre2_version() {
    let tmp = tempfile::tempdir().unwrap();
    let input = tmp.path().join("in.las");
    small_las(&input);
    let output = tmp.path().join("out.oct");
    let run = run_cli(&[&input, &output], &["--format-version", "v8.2-pre"]);
    assert!(!run.status.success());
    assert!(!output.exists());
    assert!(stderr(&run).contains("v8.2-pre"), "{}", stderr(&run));
}

#[test]
fn cli_converts_and_verifies_v8_pre1() {
    let tmp = tempfile::tempdir().unwrap();
    let input = tmp.path().join("in.las");
    small_las(&input);
    let output = tmp.path().join("out.oct");
    let run = run_cli(&[&input, &output], &["--format-version", "v8.1-pre"]);
    assert!(run.status.success(), "{}", stderr(&run));
    let primary = fs::read(&output).unwrap();
    assert_eq!(&primary[primary.len() - 40..][..8], b"OCTVE81P");
    let reader = FormatReader::new(fs::File::open(&output).unwrap()).unwrap();
    assert_eq!(
        reader.format_version(),
        viewer_format::FormatVersion::V8_PRE1
    );
    // The LAS schema starts with intensity and classification: those are the LOD set.
    assert_eq!(
        reader.format_metadata().lod_attribute_columns(),
        Some(&[0, 1][..])
    );
    let verify = Command::new(env!("CARGO_BIN_EXE_viewer_convert"))
        .arg("verify")
        .arg(&output)
        .output()
        .unwrap();
    assert!(verify.status.success(), "{}", stderr(&verify));
}

#[test]
fn default_conversion_writes_v8_pair() {
    let tmp = tempfile::tempdir().unwrap();
    let input = tmp.path().join("in.las");
    small_las(&input);
    let output = tmp.path().join("out.oct");
    let run = run_cli(&[&input, &output], &[]);
    assert!(run.status.success(), "{}", stderr(&run));
    let primary = fs::read(&output).unwrap();
    let attributes = fs::read(tmp.path().join("out.octattrs")).unwrap();
    assert_eq!(&primary[primary.len() - 40..][..8], b"OCTVEND8");
    assert_eq!(&attributes[attributes.len() - 40..][..8], b"OCTVEND8");
    assert!(contains(&primary, b"leaf:64000"));
}

#[test]
fn default_extra_bytes_error_names_version() {
    let tmp = tempfile::tempdir().unwrap();
    let input = tmp.path().join("in.las");
    // Three Extra Bytes per record and no descriptor of that size: inconsistent.
    let mut builder = Builder::from((1, 4));
    builder.point_format = Format::new(0).unwrap();
    builder.point_format.extra_bytes = 3;
    builder.vlrs.push(las::Vlr {
        user_id: "LASF_Spec".to_owned(),
        record_id: 4,
        description: String::new(),
        data: vec![0; 192],
    });
    let mut writer = Writer::from_path(&input, builder.into_header().unwrap()).unwrap();
    writer
        .write_point(Point {
            extra_bytes: vec![0; 3],
            ..Point::default()
        })
        .unwrap();
    writer.close().unwrap();
    let output = tmp.path().join("x.oct");
    let run = run_cli(&[&input, &output], &[]);
    assert!(!run.status.success());
    let message = stderr(&run);
    assert!(message.contains("Extra Bytes"), "{message}");
    assert!(message.contains("v8"), "{message}");
    assert!(!message.contains("v7.5-pre"), "{message}");
    assert!(!output.exists());
    assert_eq!(
        fs::read_dir(tmp.path()).unwrap().count(),
        1,
        "partial output"
    );
}

#[test]
fn v5_without_points_per_block_keeps_262144() {
    let tmp = tempfile::tempdir().unwrap();
    let input = tmp.path().join("in.las");
    // More points than the 64000 a wrong default would cap blocks at.
    las_with_points(&input, 150_000);
    let implicit = tmp.path().join("implicit.oct");
    let explicit = tmp.path().join("explicit.oct");
    let run = run_cli(&[&input, &implicit], &["--format-version", "v5"]);
    assert!(run.status.success(), "{}", stderr(&run));
    let run = run_cli(
        &[&input, &explicit],
        &["--format-version", "v5", "--points-per-block", "262144"],
    );
    assert!(run.status.success(), "{}", stderr(&run));
    // `assert!`, not `assert_eq!`, so a failure does not dump megabytes.
    assert!(
        fs::read(&implicit).unwrap() == fs::read(&explicit).unwrap(),
        ".oct differs"
    );
    assert!(
        fs::read(tmp.path().join("implicit.octattrs")).unwrap()
            == fs::read(tmp.path().join("explicit.octattrs")).unwrap(),
        ".octattrs differs"
    );
}

#[test]
fn cli_refuses_ept_input_before_any_work() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("ept");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("ept.json"), "{}").unwrap();
    for (name, input, flags) in [
        ("dir.oct", root.clone(), vec![]),
        ("json.oct", root.join("ept.json"), vec![]),
        ("v5.oct", root.clone(), vec!["--format-version", "v5"]),
    ] {
        let output = tmp.path().join(name);
        let run = run_cli(&[&input, &output], &flags);
        assert!(!run.status.success(), "{name}");
        let message = stderr(&run);
        assert!(
            message.contains("EPT") && message.contains("removed with V8 (ADR 0061)"),
            "{name}: {message}"
        );
        assert!(message.contains("LAS/LAZ"), "{name}: {message}");
        assert!(!output.exists(), "{name} left an output");
    }
    // Only the EPT directory remains next to the missing outputs.
    assert_eq!(
        fs::read_dir(tmp.path()).unwrap().count(),
        1,
        "partial output"
    );
}

#[test]
fn cli_does_not_call_a_plain_directory_an_ept_input() {
    let tmp = tempfile::tempdir().unwrap();
    let plain = tmp.path().join("plain");
    fs::create_dir_all(&plain).unwrap();
    let output = tmp.path().join("plain.oct");
    let run = run_cli(&[&plain, &output], &[]);
    assert!(!run.status.success());
    let message = stderr(&run);
    assert!(!message.contains("EPT"), "{message}");
    assert!(message.contains("is a directory"), "{message}");
    assert!(!output.exists());
}

#[test]
fn cli_refuses_checkpoint_and_resume_before_any_work() {
    let tmp = tempfile::tempdir().unwrap();
    let input = tmp.path().join("in.las");
    small_las(&input);
    for (name, flags) in [
        ("c.oct", vec!["--checkpoint"]),
        ("r.oct", vec!["--resume"]),
        ("cv5.oct", vec!["--checkpoint", "--format-version", "v5"]),
        ("rv5.oct", vec!["--resume", "--format-version", "v5"]),
    ] {
        let output = tmp.path().join(name);
        let run = run_cli(&[&input, &output], &flags);
        assert!(!run.status.success(), "{name}");
        let message = stderr(&run);
        assert!(
            message.contains(flags[0]) && message.contains("removed with V8 (ADR 0061)"),
            "{name}: {message}"
        );
        assert!(!output.exists(), "{name} left an output");
        assert!(
            !output.with_extension("octattrs").exists(),
            "{name} left attributes"
        );
    }
    assert_eq!(
        fs::read_dir(tmp.path()).unwrap().count(),
        1,
        "partial output"
    );
}
