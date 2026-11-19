//! Linear unit of a stored CRS (ADR 0049, D6): what one dataset unit measures.
//!
//! Only the top-level unit of the projected (or horizontal) component counts: a `UNIT["degree"]`
//! nested in the base geographic CRS of a projected CRS says nothing about the coordinates.
//! Parsing is defensive: garbage, deep nesting or oversize text fall back to metres, flagged assumed.

use crate::global_frame::{CrsRaw, CrsRecord};

const MAX_WKT_BYTES: usize = 1 << 16;
const MAX_WKT_DEPTH: usize = 24;
const MAX_WKT_NODES: usize = 4096;
const MAX_GEOKEYS: usize = 4096;

/// What one dataset unit measures.
#[derive(Debug, Clone, PartialEq)]
pub enum CrsLinearUnit {
    /// Exactly one metre.
    Metres,
    /// A linear unit other than the metre (`metres_per_unit` is finite and positive).
    Linear { name: String, metres_per_unit: f64 },
    /// Angular coordinates (degrees): no length in metres applies.
    Geographic,
}

/// The unit and whether the CRS declared it (`assumed`: metres were taken for lack of a unit).
#[derive(Debug, Clone, PartialEq)]
pub struct CrsUnit {
    pub unit: CrsLinearUnit,
    pub assumed: bool,
}

impl CrsUnit {
    const fn declared(unit: CrsLinearUnit) -> Self {
        Self {
            unit,
            assumed: false,
        }
    }

    const fn assumed_metres() -> Self {
        Self {
            unit: CrsLinearUnit::Metres,
            assumed: true,
        }
    }

    fn linear(name: &str, metres_per_unit: f64) -> Self {
        if !(metres_per_unit.is_finite() && metres_per_unit > 0.0) {
            return Self::assumed_metres();
        }
        if metres_per_unit == 1.0 {
            return Self::declared(CrsLinearUnit::Metres);
        }
        Self::declared(CrsLinearUnit::Linear {
            name: name.to_owned(),
            metres_per_unit,
        })
    }
}

impl CrsRecord {
    /// The linear unit of the stored CRS; metres, flagged assumed, when there is none to read.
    pub fn linear_unit(&self) -> CrsUnit {
        match self.raw() {
            CrsRaw::None => CrsUnit::assumed_metres(),
            CrsRaw::Wkt(text) => wkt_unit(text),
            CrsRaw::GeoTiff { key_directory, .. } => geotiff_unit(key_directory),
        }
    }
}

#[derive(Default)]
struct Node {
    keyword: String,
    name: Option<String>,
    number: Option<f64>,
    children: Vec<Node>,
}

struct Parser<'a> {
    bytes: &'a [u8],
    pos: usize,
    nodes: usize,
}

impl Parser<'_> {
    fn skip_space(&mut self) {
        while self
            .bytes
            .get(self.pos)
            .is_some_and(u8::is_ascii_whitespace)
        {
            self.pos += 1;
        }
    }

    /// `KEYWORD[args]` (or parentheses); `None` on malformed input.
    fn node(&mut self, depth: usize) -> Option<Node> {
        self.nodes += 1;
        if depth > MAX_WKT_DEPTH || self.nodes > MAX_WKT_NODES {
            return None;
        }
        self.skip_space();
        let start = self.pos;
        while self
            .bytes
            .get(self.pos)
            .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
        {
            self.pos += 1;
        }
        let keyword = std::str::from_utf8(&self.bytes[start..self.pos])
            .ok()?
            .to_ascii_uppercase();
        self.skip_space();
        let close = match self.bytes.get(self.pos)? {
            b'[' => b']',
            b'(' => b')',
            _ => return None,
        };
        self.pos += 1;
        let mut node = Node {
            keyword,
            ..Node::default()
        };
        loop {
            self.skip_space();
            match *self.bytes.get(self.pos)? {
                byte if byte == close => {
                    self.pos += 1;
                    return Some(node);
                }
                b',' => self.pos += 1,
                b'"' => {
                    let start = self.pos + 1;
                    let length = self.bytes[start..].iter().position(|byte| *byte == b'"')?;
                    self.pos = start + length + 1;
                    if node.name.is_none() {
                        node.name = Some(
                            String::from_utf8_lossy(&self.bytes[start..start + length]).into(),
                        );
                    }
                }
                _ => {
                    let start = self.pos;
                    while self.bytes.get(self.pos).is_some_and(|byte| {
                        byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-' | b'+')
                    }) {
                        self.pos += 1;
                    }
                    if self.pos == start {
                        return None;
                    }
                    let token = std::str::from_utf8(&self.bytes[start..self.pos]).ok()?;
                    self.skip_space();
                    if matches!(self.bytes.get(self.pos), Some(b'[' | b'(')) {
                        self.pos = start;
                        node.children.push(self.node(depth + 1)?);
                    } else if node.number.is_none() {
                        node.number = token.parse::<f64>().ok();
                    }
                }
            }
        }
    }
}

fn is_unit_keyword(keyword: &str) -> bool {
    matches!(keyword, "UNIT" | "LENGTHUNIT")
}

fn unit_of(node: &Node) -> Option<CrsUnit> {
    let unit = node.children.iter().find(|c| is_unit_keyword(&c.keyword))?;
    Some(CrsUnit::linear(unit.name.as_deref()?, unit.number?))
}

fn is_geographic(keyword: &str) -> bool {
    matches!(keyword, "GEOGCS" | "GEOGCRS" | "GEOGRAPHICCRS")
}

fn is_crs(keyword: &str) -> bool {
    is_geographic(keyword)
        || matches!(
            keyword,
            "PROJCS" | "PROJCRS" | "PROJECTEDCRS" | "GEOCCS" | "GEODCRS" | "GEODETICCRS"
        )
}

fn node_unit(node: &Node) -> CrsUnit {
    match node.keyword.as_str() {
        "COMPD_CS" | "COMPOUNDCRS" => node
            .children
            .iter()
            .find(|child| is_crs(&child.keyword))
            .map_or_else(CrsUnit::assumed_metres, node_unit),
        keyword if is_geographic(keyword) => CrsUnit::declared(CrsLinearUnit::Geographic),
        // WKT2 may put the unit in the first axis instead of after the coordinate system.
        _ => unit_of(node)
            .or_else(|| {
                node.children
                    .iter()
                    .filter(|child| child.keyword == "AXIS")
                    .find_map(unit_of)
            })
            .unwrap_or_else(CrsUnit::assumed_metres),
    }
}

fn wkt_unit(text: &str) -> CrsUnit {
    if text.len() > MAX_WKT_BYTES {
        return CrsUnit::assumed_metres();
    }
    let mut parser = Parser {
        bytes: text.as_bytes(),
        pos: 0,
        nodes: 0,
    };
    parser
        .node(0)
        .map_or_else(CrsUnit::assumed_metres, |root| node_unit(&root))
}

const KEY_MODEL_TYPE: u16 = 1024;
const KEY_PROJ_LINEAR_UNITS: u16 = 3076;
const MODEL_TYPE_GEOGRAPHIC: u16 = 2;

/// EPSG linear unit code -> (name, metres per unit).
fn epsg_linear_unit(code: u16) -> Option<(&'static str, f64)> {
    Some(match code {
        9001 => ("metre", 1.0),
        9002 => ("foot", 0.3048),
        9003 => ("US survey foot", 0.304_800_609_601_219_2),
        9030 => ("nautical mile", 1852.0),
        9036 => ("kilometre", 1000.0),
        _ => return None,
    })
}

/// Reads `GeoKey` ids 1024 (model type) and 3076 (projected linear units) of a `GeoTIFF` key directory.
fn geotiff_unit(directory: &[u8]) -> CrsUnit {
    let word = |index: usize| {
        directory
            .get(index * 2..index * 2 + 2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
    };
    let Some(count) = word(3) else {
        return CrsUnit::assumed_metres();
    };
    let mut unit = CrsUnit::assumed_metres();
    for entry in 0..usize::from(count).min(MAX_GEOKEYS) {
        let base = 4 + entry * 4;
        let (Some(key), Some(location), Some(value)) = (word(base), word(base + 1), word(base + 3))
        else {
            break;
        };
        // Only values stored in the directory itself (location 0) are short codes.
        if location != 0 {
            continue;
        }
        if key == KEY_MODEL_TYPE && value == MODEL_TYPE_GEOGRAPHIC {
            return CrsUnit::declared(CrsLinearUnit::Geographic);
        }
        if key == KEY_PROJ_LINEAR_UNITS
            && let Some((name, factor)) = epsg_linear_unit(value)
        {
            unit = CrsUnit::linear(name, factor);
        }
    }
    unit
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wkt(text: &str) -> CrsUnit {
        CrsRecord::new("EPSG".into(), 1, CrsRaw::Wkt(text.into()))
            .unwrap()
            .linear_unit()
    }

    fn foot(unit: &CrsUnit) -> f64 {
        match &unit.unit {
            CrsLinearUnit::Linear {
                metres_per_unit, ..
            } => *metres_per_unit,
            other => panic!("not linear: {other:?}"),
        }
    }

    const GEOGCS: &str = r#"GEOGCS["NAD83",DATUM["D",SPHEROID["GRS 1980",6378137,298.257]],PRIMEM["Greenwich",0],UNIT["degree",0.0174532925199433]]"#;

    #[test]
    fn wkt1_projcs_metre_is_declared() {
        let text = format!(
            r#"PROJCS["UTM 32N",{GEOGCS},PROJECTION["Transverse_Mercator"],PARAMETER["scale_factor",0.9996],UNIT["metre",1],AXIS["E",EAST]]"#
        );
        assert_eq!(wkt(&text), CrsUnit::declared(CrsLinearUnit::Metres));
    }

    #[test]
    fn wkt1_us_survey_foot() {
        let text = format!(
            r#"PROJCS["SPCS",{GEOGCS},PROJECTION["Lambert_Conformal_Conic_2SP"],UNIT["US survey foot",0.3048006096012192]]"#
        );
        let unit = wkt(&text);
        assert!(!unit.assumed);
        assert!((foot(&unit) - 0.304_800_609_601_219_2).abs() < 1e-15);
    }

    #[test]
    fn wkt2_lengthunit_after_cs_and_in_axis() {
        let after_cs = r#"PROJCRS["X",BASEGEOGCRS["N",ANGLEUNIT["degree",0.0174532925199433]],CONVERSION["c",PARAMETER["p",1,LENGTHUNIT["metre",1]]],CS[Cartesian,2],AXIS["(E)",east],LENGTHUNIT["US survey foot",0.3048006096012192]]"#;
        assert!((foot(&wkt(after_cs)) - 0.304_800_609_601_219_2).abs() < 1e-15);
        let in_axis =
            r#"PROJCRS["X",CS[Cartesian,2],AXIS["(E)",east,ORDER[1],LENGTHUNIT["foot",0.3048]]]"#;
        assert!((foot(&wkt(in_axis)) - 0.3048).abs() < 1e-15);
    }

    #[test]
    fn top_level_geographic_is_geographic() {
        assert_eq!(wkt(GEOGCS).unit, CrsLinearUnit::Geographic);
        let wkt2 = r#"GEOGCRS["WGS 84",DATUM["d"],CS[ellipsoidal,2],ANGLEUNIT["degree",0.0174532925199433]]"#;
        assert_eq!(wkt(wkt2).unit, CrsLinearUnit::Geographic);
    }

    #[test]
    fn nested_degree_of_a_projected_crs_is_not_geographic() {
        let text = format!(r#"PROJCS["P",{GEOGCS},PROJECTION["x"]]"#);
        assert_eq!(wkt(&text), CrsUnit::assumed_metres());
    }

    #[test]
    fn compound_uses_the_horizontal_component() {
        let text = format!(
            r#"COMPD_CS["c",PROJCS["h",{GEOGCS},UNIT["foot",0.3048]],VERT_CS["v",VERT_DATUM["d",2005],UNIT["metre",1]]]"#
        );
        assert!((foot(&wkt(&text)) - 0.3048).abs() < 1e-15);
        let wkt2 = r#"COMPOUNDCRS["c",PROJCRS["h",CS[Cartesian,2],LENGTHUNIT["foot",0.3048]],VERTCRS["v",LENGTHUNIT["metre",1]]]"#;
        assert!((foot(&wkt(wkt2)) - 0.3048).abs() < 1e-15);
    }

    #[test]
    fn no_unit_or_authority_only_is_assumed_metres() {
        assert_eq!(wkt(r#"PROJCS["P"]"#), CrsUnit::assumed_metres());
        let authority_only = CrsRecord::new("EPSG".into(), 32632, CrsRaw::None).unwrap();
        assert_eq!(authority_only.linear_unit(), CrsUnit::assumed_metres());
        assert_eq!(CrsRecord::local().linear_unit(), CrsUnit::assumed_metres());
    }

    fn geotiff(entries: &[(u16, u16, u16, u16)]) -> CrsUnit {
        let mut words = vec![1, 1, 0, entries.len() as u16];
        for entry in entries {
            words.extend([entry.0, entry.1, entry.2, entry.3]);
        }
        let key_directory = words.iter().flat_map(|word| word.to_le_bytes()).collect();
        CrsRecord::new(
            String::new(),
            0,
            CrsRaw::GeoTiff {
                key_directory,
                doubles: Vec::new(),
                ascii: Vec::new(),
            },
        )
        .unwrap()
        .linear_unit()
    }

    #[test]
    fn geotiff_linear_units_and_model_type() {
        let unit = geotiff(&[(1024, 0, 1, 1), (3076, 0, 1, 9002)]);
        assert!(!unit.assumed);
        assert!((foot(&unit) - 0.3048).abs() < 1e-15);
        assert_eq!(
            geotiff(&[(3076, 0, 1, 9001)]),
            CrsUnit::declared(CrsLinearUnit::Metres)
        );
        assert_eq!(
            geotiff(&[(1024, 0, 1, 2)]),
            CrsUnit::declared(CrsLinearUnit::Geographic)
        );
        assert_eq!(geotiff(&[(3076, 0, 1, 1)]), CrsUnit::assumed_metres());
    }

    #[test]
    fn malformed_input_falls_back_to_assumed_metres() {
        for text in [
            "PROJCS[",
            "PROJCS[\"unterminated",
            "]]]]",
            "UNIT[\"x\",abc]",
            r#"PROJCS["P",UNIT["x",0]]"#,
            r#"PROJCS["P",UNIT["x",-3]]"#,
            r#"PROJCS["P",UNIT["x",NaN]]"#,
        ] {
            assert_eq!(wkt(text), CrsUnit::assumed_metres(), "{text}");
        }
        let deep = format!("{}{}", "A[".repeat(10_000), "]".repeat(10_000));
        assert_eq!(wkt(&deep), CrsUnit::assumed_metres());
        let truncated = CrsRaw::GeoTiff {
            key_directory: vec![1, 0, 1, 0, 0, 0, 9, 0, 1],
            doubles: Vec::new(),
            ascii: Vec::new(),
        };
        let record = CrsRecord::new(String::new(), 0, truncated).unwrap();
        assert_eq!(record.linear_unit(), CrsUnit::assumed_metres());
    }
}
