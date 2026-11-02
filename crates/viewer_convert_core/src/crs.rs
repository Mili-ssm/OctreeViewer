//! CRS identity and raw bytes from the LAS projection records (`v7.4-pre`, RF-02, RF-24).
//!
//! Identity is authority plus code: the WKT1 root `AUTHORITY`, else GeoKey 3072 (projected) then
//! 2048 (geographic). A file with neither is local.

use viewer_format::{CrsRaw, CrsRecord};

const PROJECTED_CS_TYPE_KEY: u16 = 3072;
const GEOGRAPHIC_TYPE_KEY: u16 = 2048;

/// Reads the CRS of `header` from its VLRs and EVLRs; a WKT record wins over GeoTIFF keys.
pub fn read_crs(header: &las::Header) -> CrsRecord {
    let mut wkt = None;
    let mut key_directory = None;
    let mut doubles = None;
    let mut ascii = None;
    for vlr in header.all_vlrs() {
        if !vlr.user_id.eq_ignore_ascii_case("LASF_Projection") {
            continue;
        }
        match vlr.record_id {
            2112 if wkt.is_none() => wkt = String::from_utf8(vlr.data.clone()).ok(),
            34735 if key_directory.is_none() => key_directory = Some(vlr.data.clone()),
            34736 if doubles.is_none() => doubles = Some(vlr.data.clone()),
            34737 if ascii.is_none() => ascii = Some(vlr.data.clone()),
            _ => {}
        }
    }
    if let Some(text) = wkt.filter(|text| !text.trim_matches('\0').trim().is_empty()) {
        let (authority, code) = wkt_root_authority(&text).unwrap_or_default();
        return CrsRecord::new(authority, code, CrsRaw::Wkt(text))
            .unwrap_or_else(|_| CrsRecord::local());
    }
    if let Some(key_directory) = key_directory.filter(|bytes| !bytes.is_empty()) {
        let (authority, code) = geotiff_code(&key_directory)
            .map(|code| ("EPSG".to_owned(), code))
            .unwrap_or_default();
        let raw = CrsRaw::GeoTiff {
            key_directory,
            doubles: doubles.unwrap_or_default(),
            ascii: ascii.unwrap_or_default(),
        };
        return CrsRecord::new(authority, code, raw).unwrap_or_else(|_| CrsRecord::local());
    }
    CrsRecord::local()
}

/// `(authority, code)` of the root node's own `AUTHORITY["EPSG","32632"]`, not a nested one.
pub fn wkt_root_authority(wkt: &str) -> Option<(String, u32)> {
    const TAG: &str = "AUTHORITY[";
    let bytes = wkt.as_bytes();
    let mut depth = 0_u32;
    let mut in_quote = false;
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if in_quote {
            in_quote = byte != b'"';
        } else {
            match byte {
                b'"' => in_quote = true,
                b'[' | b'(' => depth += 1,
                b']' | b')' => depth = depth.saturating_sub(1),
                b'A' if depth == 1 && wkt[index..].starts_with(TAG) => {
                    return parse_authority(&wkt[index + TAG.len()..]);
                }
                _ => {}
            }
        }
        index += 1;
    }
    None
}

fn parse_authority(rest: &str) -> Option<(String, u32)> {
    let mut quoted = rest.split('"').skip(1).step_by(2);
    let authority = quoted.next()?.trim().to_ascii_uppercase();
    let code: u32 = quoted.next()?.trim().parse().ok()?;
    (code != 0 && !authority.is_empty()).then_some((authority, code))
}

/// EPSG code from GeoKey 3072, else 2048, stored inline; 0 and 32767 are no code.
pub fn geotiff_code(directory: &[u8]) -> Option<u32> {
    let words: Vec<u16> = directory
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_le_bytes(*pair))
        .collect();
    let key_count = usize::from(*words.get(3)?);
    let lookup = |wanted: u16| {
        words
            .get(4..)?
            .as_chunks::<4>()
            .0
            .iter()
            .take(key_count)
            .find(|entry| entry[0] == wanted && entry[1] == 0 && entry[2] == 1)
            .map(|entry| u32::from(entry[3]))
            .filter(|code| *code != 0 && *code != 32767)
    };
    lookup(PROJECTED_CS_TYPE_KEY).or_else(|| lookup(GEOGRAPHIC_TYPE_KEY))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wkt_identity_uses_the_root_authority_not_a_nested_one() {
        let wkt = "PROJCS[\"WGS 84 / UTM zone 32N\",GEOGCS[\"WGS 84\",AUTHORITY[\"EPSG\",\"4326\"]],UNIT[\"metre\",1],AUTHORITY[\"EPSG\",\"32632\"]]";
        assert_eq!(wkt_root_authority(wkt), Some(("EPSG".to_owned(), 32632)));
        assert_eq!(
            wkt_root_authority("PROJCS[\"x\",GEOGCS[\"y\",AUTHORITY[\"EPSG\",\"4326\"]]]"),
            None
        );
    }

    #[test]
    fn geokeys_prefer_projected_and_ignore_undefined_values() {
        let directory = |keys: &[[u16; 4]]| -> Vec<u8> {
            let mut words = vec![1, 1, 0, keys.len() as u16];
            words.extend(keys.iter().flatten());
            words.iter().flat_map(|word| word.to_le_bytes()).collect()
        };
        let both = directory(&[[2048, 0, 1, 4326], [3072, 0, 1, 32632]]);
        assert_eq!(geotiff_code(&both), Some(32632));
        let user_defined = directory(&[[3072, 0, 1, 32767], [2048, 0, 1, 4258]]);
        assert_eq!(geotiff_code(&user_defined), Some(4258));
        assert_eq!(geotiff_code(&directory(&[[3072, 0, 1, 0]])), None);
        assert_eq!(geotiff_code(&[1, 0]), None);
    }
}
