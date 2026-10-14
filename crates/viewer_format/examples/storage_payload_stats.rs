//! Metadata-only attribution of persisted geometry/attribute bytes and codecs.
use std::{collections::BTreeMap, fs::File};
use viewer_format::{BlockPayloadKind, FormatReader};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .ok_or("usage: storage_payload_stats INPUT.oct")?;
    if args.next().is_some() {
        return Err("no extra arguments".into());
    }
    let reader = FormatReader::new(File::open(path)?)?;
    let mut groups = BTreeMap::<String, (u64, u64, u64)>::new();
    for block in reader.block_storage_stats()? {
        let key = format!("geometry/{:?}", block.geometry_encoding);
        let group = groups.entry(key).or_default();
        group.0 += 1;
    }
    for payload in reader.block_payload_storage_stats()? {
        let name = match payload.kind {
            BlockPayloadKind::Geometry => "geometry_payload".to_owned(),
            BlockPayloadKind::DisplayColor => "display_rgb".to_owned(),
            BlockPayloadKind::Attribute { schema_index, .. } => format!("attribute_{schema_index}"),
        };
        let key = format!("{name}/{:?}", payload.codec);
        let group = groups.entry(key).or_default();
        group.0 += 1;
        group.1 += payload.raw_bytes;
        group.2 += payload.stored_bytes;
    }
    for (group, (count, raw, stored)) in groups {
        println!(
            "{{\"group\":\"{group}\",\"count\":{count},\"raw_bytes\":{raw},\"stored_bytes\":{stored}}}"
        );
    }
    Ok(())
}
