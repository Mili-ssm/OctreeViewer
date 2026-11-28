//! Pinned SHA-256 digests of the V8 output.

use super::*;

/// SHA-256 of the `.oct` and `.octattrs` files of a default conversion (format V8) with an explicit
/// worker count, so the pins do not depend on the machine's parallelism.
fn v8_output_shas_with(
    input: &std::path::Path,
    dir: &std::path::Path,
    name: &str,
    workers: usize,
) -> (String, String) {
    let output = dir.join(format!("{name}.oct"));
    convert_file(
        input,
        &output,
        ConvertOptions::default()
            .with_spatial_workers(workers)
            .unwrap(),
    )
    .unwrap();
    (
        sha256_hex(&fs::read(&output).unwrap()),
        sha256_hex(&fs::read(output_attributes_path(&output)).unwrap()),
    )
}

fn v8_output_shas(input: &std::path::Path, dir: &std::path::Path, name: &str) -> (String, String) {
    v8_output_shas_with(input, dir, name, 2)
}

#[test]
fn v8_output_is_identical_for_1_2_and_4_workers() {
    let tmp = test_dir();
    let input = workspace_path("data/samples/fs_e430_n4008_18.las");
    let reference = v8_output_shas_with(&input, tmp.path(), "w1", 1);
    for workers in [2, 4] {
        let name = format!("w{workers}");
        assert_eq!(
            v8_output_shas_with(&input, tmp.path(), &name, workers),
            reference,
            "workers={workers}"
        );
    }
}

#[test]
fn v8_output_of_the_sample_las_files_is_pinned() {
    let tmp = test_dir();
    // V8 output pin (D4 plan, A3): later tasks must not change these bytes.
    let expected = [
        (
            "fs_e430_n4008_18",
            "bf05ba27f080bbb0d59bcc59d60c1f306dc4041e9dbb578ca574a136dcf6a640",
            "88290810d348f45432e25c6e52d2e7901abf6ffc227b8b5b391783a365f3befc",
        ),
        (
            "fs_e432_n4002_18",
            "b39acec544853f99898a4d8c29fb975daa1cd4578e4fd5142ddbf0e504f36af9",
            "cae9edbb98ad143870e747c5327f4becfa5b4136b1e20d0e3206a7f2ab6cd90e",
        ),
    ];
    for (name, oct, attrs) in expected {
        let input = workspace_path(&format!("data/samples/{name}.las"));
        let (actual_oct, actual_attrs) = v8_output_shas(&input, tmp.path(), name);
        println!("{name} oct={actual_oct} attrs={actual_attrs}");
        assert_eq!(actual_oct, oct, "{name} .oct");
        assert_eq!(actual_attrs, attrs, "{name} .octattrs");
    }
}

/// Run with `FORMAT_PARITY_REAL_INPUT=<Belvedere LAZ> cargo test --release -p viewer_convert --
/// --ignored v8_output_of_belvedere_is_pinned --nocapture`.
#[test]
#[ignore = "needs the Belvedere dataset (FORMAT_PARITY_REAL_INPUT) and several minutes"]
fn v8_output_of_belvedere_is_pinned() {
    let Some(input) = std::env::var_os("FORMAT_PARITY_REAL_INPUT") else {
        eprintln!("skipped: FORMAT_PARITY_REAL_INPUT is not set");
        return;
    };
    let tmp = test_dir();
    let (oct, attrs) = v8_output_shas(std::path::Path::new(&input), tmp.path(), "belvedere");
    println!("belvedere oct={oct} attrs={attrs}");
    // V8 output pin (D4 plan, A3): later tasks must not change these bytes.
    assert_eq!(
        oct, "8518efc04b4a052172350014525fc4981b1b12cd92a80774817836e861b91cfc",
        "belvedere .oct"
    );
    assert_eq!(
        attrs, "20d08de06a7c4c02851ea7d1078f80f65b0ebb423f4f102a05e26922cec5b8f3",
        "belvedere .octattrs"
    );
}
