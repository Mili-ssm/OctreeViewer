#![no_main]

use std::io::Cursor;

use libfuzzer_sys::fuzz_target;
use viewer_core::BlockId;
use viewer_format::FormatReader;

/// Opens the file and decodes XYZ, colors and every attribute column of every block.
/// Errors are fine; panics and over-allocation are the findings.
fn read_all(primary: &[u8], attributes: Option<&[u8]>) {
    let Ok(mut reader) = FormatReader::new(Cursor::new(primary)) else {
        return;
    };
    if let Some(attributes) = attributes
        && reader.attach_attributes(Cursor::new(attributes)).is_err()
    {
        return;
    }
    let columns = reader
        .format_metadata()
        .dataset()
        .hierarchy()
        .attributes()
        .len() as u32;
    let Ok(stats) = reader.block_storage_stats() else {
        return;
    };
    let ids: Vec<BlockId> = stats.iter().map(|block| block.id).collect();
    for id in ids {
        let _ = reader.read_positions(id);
        let _ = reader.read_display_colors(id);
        for column in 0..columns {
            let _ = reader.read_attribute(id, column);
        }
    }
}

fuzz_target!(|data: &[u8]| {
    // Variant 1: monolithic file.
    read_all(data, None);
    // Variant 2: first 4 bytes (LE) are the split point between `.oct` and `.octattrs`.
    if let Some((head, rest)) = data.split_first_chunk::<4>() {
        let split = (u32::from_le_bytes(*head) as usize).min(rest.len());
        let (primary, attributes) = rest.split_at(split);
        read_all(primary, Some(attributes));
    }
});
