#![no_main]

use std::io::Cursor;

use libfuzzer_sys::fuzz_target;
use viewer_format::PagedReader;

/// Pages visited per input: a hostile file can chain far more pages than any real one.
const MAX_PAGES: usize = 256;

/// Opens a `v7.5-pre` file from its tail and follows every child pointer, reading the stored
/// bytes of every block. Errors are fine; panics and over-allocation are the findings.
fn walk(bytes: &[u8]) {
    let Ok(mut reader) = PagedReader::open(Cursor::new(bytes)) else {
        return;
    };
    let mut pending: Vec<_> = reader.root_page().child_pages().collect();
    for node in reader.root_page().nodes().to_vec() {
        for block in node.blocks() {
            let _ = reader.read_block_bytes(block);
        }
    }
    let mut visited = 0;
    while let Some((anchor, pointer)) = pending.pop() {
        visited += 1;
        if visited > MAX_PAGES {
            return;
        }
        let Ok(page) = reader.read_child_page(anchor, pointer) else {
            continue;
        };
        for node in page.nodes() {
            for block in node.blocks() {
                let _ = reader.read_block_bytes(block);
            }
        }
        pending.extend(page.child_pages());
        if visited % 8 == 0 {
            reader.release_page(pointer);
        }
    }
}

fuzz_target!(|data: &[u8]| walk(data));
