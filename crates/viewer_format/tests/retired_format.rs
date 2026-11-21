//! Opening a file of a retired format family says which one it is and to reconvert it (A4 of the
//! V8 freeze). The 13 header/footer pairs in `fixtures/retired/` are the real envelopes the old
//! writers produced; a minimal file is the header, padding and the footer.

use std::io::Cursor;

use viewer_format::{FormatError, FormatReader, retired_format_name, retired_header_name};

const FAMILIES: [(&str, &[u8], &[u8]); 13] = [
    (
        "v5",
        include_bytes!("fixtures/retired/v5.head"),
        include_bytes!("fixtures/retired/v5.foot"),
    ),
    (
        "v6.1-pre",
        include_bytes!("fixtures/retired/v6.1-pre.head"),
        include_bytes!("fixtures/retired/v6.1-pre.foot"),
    ),
    (
        "v6.2-pre",
        include_bytes!("fixtures/retired/v6.2-pre.head"),
        include_bytes!("fixtures/retired/v6.2-pre.foot"),
    ),
    (
        "v6.3-pre",
        include_bytes!("fixtures/retired/v6.3-pre.head"),
        include_bytes!("fixtures/retired/v6.3-pre.foot"),
    ),
    (
        "v6.4-pre",
        include_bytes!("fixtures/retired/v6.4-pre.head"),
        include_bytes!("fixtures/retired/v6.4-pre.foot"),
    ),
    (
        "v7.1-pre",
        include_bytes!("fixtures/retired/v7.1-pre.head"),
        include_bytes!("fixtures/retired/v7.1-pre.foot"),
    ),
    (
        "v7.2-pre",
        include_bytes!("fixtures/retired/v7.2-pre.head"),
        include_bytes!("fixtures/retired/v7.2-pre.foot"),
    ),
    (
        "v7.3-pre",
        include_bytes!("fixtures/retired/v7.3-pre.head"),
        include_bytes!("fixtures/retired/v7.3-pre.foot"),
    ),
    (
        "v7.4-pre",
        include_bytes!("fixtures/retired/v7.4-pre.head"),
        include_bytes!("fixtures/retired/v7.4-pre.foot"),
    ),
    (
        "v7.5-pre",
        include_bytes!("fixtures/retired/v7.5-pre.head"),
        include_bytes!("fixtures/retired/v7.5-pre.foot"),
    ),
    (
        "v7",
        include_bytes!("fixtures/retired/v7.head"),
        include_bytes!("fixtures/retired/v7.foot"),
    ),
    (
        "v8.1-pre",
        include_bytes!("fixtures/retired/v8.1-pre.head"),
        include_bytes!("fixtures/retired/v8.1-pre.foot"),
    ),
    (
        "v8.2-pre",
        include_bytes!("fixtures/retired/v8.2-pre.head"),
        include_bytes!("fixtures/retired/v8.2-pre.foot"),
    ),
];

fn minimal_file(head: &[u8], foot: &[u8]) -> Vec<u8> {
    let mut bytes = head.to_vec();
    bytes.resize(head.len() + 200, 0);
    bytes.extend_from_slice(foot);
    bytes
}

#[test]
fn every_retired_family_is_recognised_by_its_footer_and_by_its_header() {
    for (name, head, foot) in FAMILIES {
        assert_eq!(retired_format_name(foot), Some(name), "footer of {name}");
        assert_eq!(retired_header_name(head), Some(name), "header of {name}");
    }
}

#[test]
fn the_final_v8_envelope_is_not_retired() {
    assert_eq!(
        retired_format_name(b"OCTVEND8................................"),
        None
    );
    let mut head = FAMILIES[11].1.to_vec();
    head[16..20].copy_from_slice(&0_u32.to_le_bytes());
    head[8..10].copy_from_slice(&8_u16.to_le_bytes());
    assert_eq!(retired_header_name(&head), None);
    assert_eq!(retired_format_name(b"short"), None);
    assert_eq!(retired_header_name(b"short"), None);
}

#[test]
fn opening_a_retired_file_names_the_version_and_says_to_reconvert() {
    for (name, head, foot) in FAMILIES {
        let error = FormatReader::new(Cursor::new(minimal_file(head, foot)))
            .err()
            .unwrap_or_else(|| panic!("{name} opened"));
        assert!(
            matches!(error, FormatError::RetiredFormat(found) if found == name),
            "{name}: {error:?}"
        );
        let message = error.to_string();
        assert!(
            message.contains(name) && message.contains("reconvert"),
            "{message}"
        );
    }
}

#[test]
fn opening_a_retired_file_lazily_says_the_same() {
    for (name, head, foot) in FAMILIES {
        let error = FormatReader::open_lazy(
            Cursor::new(minimal_file(head, foot)),
            viewer_format::FormatReaderLimits::default(),
        )
        .err()
        .unwrap_or_else(|| panic!("{name} opened"));
        assert!(
            matches!(error, FormatError::RetiredFormat(found) if found == name),
            "{name}: {error:?}"
        );
    }
}

#[test]
fn a_retired_header_alone_is_enough_when_the_footer_is_lost() {
    for (name, head, _) in FAMILIES {
        let mut bytes = head.to_vec();
        bytes.resize(300, 0);
        let error = FormatReader::new(Cursor::new(bytes))
            .err()
            .expect("not a file");
        assert!(
            matches!(error, FormatError::RetiredFormat(found) if found == name),
            "{name}: {error:?}"
        );
    }
}

#[test]
fn an_io_failure_is_not_reported_as_a_retired_format() {
    let error = FormatReader::new(Cursor::new(Vec::<u8>::new()))
        .err()
        .expect("empty");
    assert!(!matches!(error, FormatError::RetiredFormat(_)), "{error:?}");
}
