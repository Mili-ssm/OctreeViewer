use std::{fs, process::Command};

use las::{Builder, Point, Writer, point::Format};
use viewer_core::HierarchySemantics;
use viewer_format::{FormatReader, OctreeReader};

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
        .args(["--format-version", "v5"])
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
        .args(["--format-version", "v5", "--spatial-workers", "2"])
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
fn cli_accepts_v8_pre2() {
    let tmp = tempfile::tempdir().unwrap();
    let input = tmp.path().join("in.las");
    small_las(&input);
    let output = tmp.path().join("out.oct");
    let run = run_cli(&[&input, &output], &["--format-version", "v8.2-pre"]);
    assert!(run.status.success(), "{}", stderr(&run));
    let primary = fs::read(&output).unwrap();
    assert_eq!(&primary[primary.len() - 40..][..8], b"OCTVE82P");
    let reader = FormatReader::new(fs::File::open(&output).unwrap()).unwrap();
    assert_eq!(
        reader.format_version(),
        viewer_format::FormatVersion::V8_PRE2
    );
    let verify = Command::new(env!("CARGO_BIN_EXE_viewer_convert"))
        .arg("verify")
        .arg(&output)
        .output()
        .unwrap();
    assert!(verify.status.success(), "{}", stderr(&verify));
}

#[test]
fn cli_converts_and_verifies_each_scheme() {
    let tmp = tempfile::tempdir().unwrap();
    let input = tmp.path().join("in.las");
    las_with_points(&input, 5000);
    for scheme in ["lz4", "cold", "cold-rgb", "all"] {
        let output = tmp.path().join(format!("{scheme}.oct"));
        let run = run_cli(
            &[&input, &output],
            &["--format-version", "v8.2-pre", "--attr-codec", scheme],
        );
        assert!(run.status.success(), "{scheme}: {}", stderr(&run));
        let verify = Command::new(env!("CARGO_BIN_EXE_viewer_convert"))
            .arg("verify")
            .arg(&output)
            .output()
            .unwrap();
        assert!(verify.status.success(), "{scheme}: {}", stderr(&verify));
    }
    // The scheme is not part of the file identity: the same input with another scheme still
    // writes a v8.2-pre pair, and an unknown scheme or a format without zstd is refused.
    let refused = |name: &str, flags: &[&str]| {
        let output = tmp.path().join(name);
        let run = run_cli(&[&input, &output], flags);
        assert!(!run.status.success(), "{name}");
        assert!(!output.exists(), "{name} left an output");
        stderr(&run)
    };
    assert!(
        refused(
            "bad.oct",
            &["--format-version", "v8.2-pre", "--attr-codec", "zstd"]
        )
        .contains("unknown attribute codec")
    );
    for version in ["v8.1-pre", "v7"] {
        assert!(
            refused(
                &format!("{version}.oct"),
                &["--format-version", version, "--attr-codec", "cold"]
            )
            .contains("v8.2-pre"),
            "{version}"
        );
    }
    assert!(refused("default.oct", &["--attr-codec", "all"]).contains("v8.2-pre"));
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
fn default_conversion_writes_v7_pair() {
    let tmp = tempfile::tempdir().unwrap();
    let input = tmp.path().join("in.las");
    small_las(&input);
    let output = tmp.path().join("out.oct");
    let run = run_cli(&[&input, &output], &[]);
    assert!(run.status.success(), "{}", stderr(&run));
    let primary = fs::read(&output).unwrap();
    let attributes = fs::read(tmp.path().join("out.octattrs")).unwrap();
    assert_eq!(&primary[primary.len() - 40..][..8], b"OCTVEND7");
    assert_eq!(&attributes[attributes.len() - 40..][..8], b"OCTVEND7");
    assert!(contains(&primary, b"leaf:64000"));
}

#[test]
fn default_checkpoint_points_to_v5() {
    let tmp = tempfile::tempdir().unwrap();
    let input = tmp.path().join("in.las");
    small_las(&input);
    let output = tmp.path().join("c.oct");
    let run = run_cli(&[&input, &output], &["--checkpoint"]);
    assert!(!run.status.success());
    let message = stderr(&run);
    assert!(message.contains("v7"), "{message}");
    assert!(message.contains("--format-version v5"), "{message}");
    assert!(!output.exists());
    assert!(!tmp.path().join("c.octattrs").exists());
    assert_eq!(
        fs::read_dir(tmp.path()).unwrap().count(),
        1,
        "partial output"
    );
}

#[test]
fn default_ept_points_to_v5() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("ept");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("ept.json"), "{}").unwrap();
    let output = tmp.path().join("e.oct");
    let run = run_cli(&[&root, &output], &[]);
    assert!(!run.status.success());
    let message = stderr(&run);
    assert!(message.contains("EPT"), "{message}");
    assert!(message.contains("--format-version v5"), "{message}");
    assert!(!output.exists());
    assert!(!tmp.path().join("e.octattrs").exists());
    // Only the EPT directory remains next to the missing output.
    assert_eq!(
        fs::read_dir(tmp.path()).unwrap().count(),
        1,
        "partial output"
    );
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
    assert!(message.contains("v7"), "{message}");
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
fn resume_v5_checkpoint_without_format_is_rejected() {
    use viewer_convert::{
        CheckpointMode, ConversionStage, ConvertError, convert_file_with_checkpoint,
    };
    use viewer_convert_core::ConvertOptions;

    let tmp = tempfile::tempdir().unwrap();
    let input = tmp.path().join("in.las");
    small_las(&input);
    let output = tmp.path().join("r.oct");
    // Built like the CLI does, so the positive control below matches the checkpoint identity.
    let defaults = ConvertOptions::default();
    let v5 = ConvertOptions::with_requested_points_per_block(
        None,
        defaults.max_quantization_error(),
        defaults.max_memory_budget_bytes(),
    )
    .unwrap()
    .with_format_version("v5".parse().unwrap())
    .unwrap();
    // Cancel while writing blocks so the checkpoint stays on disk.
    let cancelled =
        convert_file_with_checkpoint(&input, &output, v5, CheckpointMode::Create, |progress| {
            !(progress.stage == ConversionStage::ReadingAndWritingBlocks
                && progress.points_processed > 0)
        });
    assert!(matches!(cancelled, Err(ConvertError::Cancelled { .. })));
    let files = |dir: &std::path::Path| {
        let mut names: Vec<_> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        names.sort();
        names
    };
    let before = files(tmp.path());
    assert!(before.len() > 1, "a checkpoint remains: {before:?}");

    // Without --format-version the resume resolves to v7: it must not mix formats.
    let resumed = run_cli(&[&input, &output], &["--resume"]);
    assert!(!resumed.status.success());
    assert!(
        stderr(&resumed).contains("checkpoint"),
        "{}",
        stderr(&resumed)
    );
    assert!(!output.exists());
    assert_eq!(files(tmp.path()), before);

    // Positive control: the same checkpoint resumes when the format matches.
    let resumed = run_cli(&[&input, &output], &["--resume", "--format-version", "v5"]);
    assert!(resumed.status.success(), "{}", stderr(&resumed));
}

#[test]
fn resume_v5_checkpoint_with_other_options_is_rejected_by_identity() {
    use viewer_convert::{
        CheckpointMode, ConversionStage, ConvertError, convert_file_with_checkpoint,
    };
    use viewer_convert_core::ConvertOptions;

    let tmp = tempfile::tempdir().unwrap();
    let input = tmp.path().join("in.las");
    small_las(&input);
    let output = tmp.path().join("r.oct");
    // Built like the CLI does, so the positive control below matches the checkpoint identity.
    let defaults = ConvertOptions::default();
    let v5 = ConvertOptions::with_requested_points_per_block(
        None,
        defaults.max_quantization_error(),
        defaults.max_memory_budget_bytes(),
    )
    .unwrap()
    .with_format_version("v5".parse().unwrap())
    .unwrap();
    // Cancel while writing blocks so the checkpoint stays on disk.
    let cancelled =
        convert_file_with_checkpoint(&input, &output, v5, CheckpointMode::Create, |progress| {
            !(progress.stage == ConversionStage::ReadingAndWritingBlocks
                && progress.points_processed > 0)
        });
    assert!(matches!(cancelled, Err(ConvertError::Cancelled { .. })));
    let files = |dir: &std::path::Path| {
        let mut names: Vec<_> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        names.sort();
        names
    };
    let before = files(tmp.path());
    assert!(before.len() > 1, "a checkpoint remains: {before:?}");

    // Another block-checkpoint format fails the manifest version check, before the identity
    // check; the same format with another option value reaches the identity check.
    let resumed = run_cli(
        &[&input, &output],
        &[
            "--resume",
            "--format-version",
            "v5",
            "--points-per-block",
            "4096",
        ],
    );
    assert!(!resumed.status.success());
    assert!(
        stderr(&resumed).contains("checkpoint does not match the source or conversion options"),
        "{}",
        stderr(&resumed)
    );
    assert!(!output.exists());
    assert_eq!(files(tmp.path()), before);
}
