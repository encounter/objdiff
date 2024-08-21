use anyhow::{bail, Result};
use prost::Message;
use serde_json::error::Category;

// Protobuf report types
include!(concat!(env!("OUT_DIR"), "/objdiff.report.rs"));
include!(concat!(env!("OUT_DIR"), "/objdiff.report.serde.rs"));

impl Report {
    pub fn parse(data: &[u8]) -> Result<Self> {
        if data.is_empty() {
            bail!(std::io::Error::from(std::io::ErrorKind::UnexpectedEof));
        }
        if data[0] == b'{' {
            // Load as JSON
            Self::from_json(data).map_err(anyhow::Error::new)
        } else {
            // Load as binary protobuf
            Self::decode(data).map_err(anyhow::Error::new)
        }
    }

    fn from_json(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        match serde_json::from_slice::<Self>(bytes) {
            Ok(report) => Ok(report),
            Err(e) => {
                match e.classify() {
                    Category::Io | Category::Eof | Category::Syntax => Err(e),
                    Category::Data => {
                        // Try to load as legacy report
                        match serde_json::from_slice::<LegacyReport>(bytes) {
                            Ok(legacy_report) => Ok(Report::from(legacy_report)),
                            Err(_) => Err(e),
                        }
                    }
                }
            }
        }
    }
}

impl Measures {
    /// Average the fuzzy match percentage over total code bytes.
    pub fn calc_fuzzy_match_percent(&mut self) {
        if self.total_code == 0 {
            self.fuzzy_match_percent = 100.0;
        } else {
            self.fuzzy_match_percent /= self.total_code as f32;
        }
    }

    /// Calculate the percentage of matched code, data, and functions.
    pub fn calc_matched_percent(&mut self) {
        self.matched_code_percent = if self.total_code == 0 {
            100.0
        } else {
            self.matched_code as f32 / self.total_code as f32 * 100.0
        };
        self.matched_data_percent = if self.total_data == 0 {
            100.0
        } else {
            self.matched_data as f32 / self.total_data as f32 * 100.0
        };
        self.matched_functions_percent = if self.total_functions == 0 {
            100.0
        } else {
            self.matched_functions as f32 / self.total_functions as f32 * 100.0
        };
    }
}

impl From<&ReportItem> for ChangeItemInfo {
    fn from(value: &ReportItem) -> Self {
        Self { fuzzy_match_percent: value.fuzzy_match_percent, size: value.size }
    }
}

/// Allows [collect](Iterator::collect) to be used on an iterator of [Measures].
impl FromIterator<Measures> for Measures {
    fn from_iter<T>(iter: T) -> Self
    where T: IntoIterator<Item = Measures> {
        let mut measures = Measures::default();
        for other in iter {
            measures.fuzzy_match_percent += other.fuzzy_match_percent * other.total_code as f32;
            measures.total_code += other.total_code;
            measures.matched_code += other.matched_code;
            measures.total_data += other.total_data;
            measures.matched_data += other.matched_data;
            measures.total_functions += other.total_functions;
            measures.matched_functions += other.matched_functions;
        }
        measures.calc_fuzzy_match_percent();
        measures.calc_matched_percent();
        measures
    }
}

// Older JSON report types
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
struct LegacyReport {
    fuzzy_match_percent: f32,
    total_code: u64,
    matched_code: u64,
    matched_code_percent: f32,
    total_data: u64,
    matched_data: u64,
    matched_data_percent: f32,
    total_functions: u32,
    matched_functions: u32,
    matched_functions_percent: f32,
    units: Vec<LegacyReportUnit>,
}

impl From<LegacyReport> for Report {
    fn from(value: LegacyReport) -> Self {
        Self {
            measures: Some(Measures {
                fuzzy_match_percent: value.fuzzy_match_percent,
                total_code: value.total_code,
                matched_code: value.matched_code,
                matched_code_percent: value.matched_code_percent,
                total_data: value.total_data,
                matched_data: value.matched_data,
                matched_data_percent: value.matched_data_percent,
                total_functions: value.total_functions,
                matched_functions: value.matched_functions,
                matched_functions_percent: value.matched_functions_percent,
            }),
            units: value.units.into_iter().map(ReportUnit::from).collect(),
        }
    }
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
struct LegacyReportUnit {
    name: String,
    fuzzy_match_percent: f32,
    total_code: u64,
    matched_code: u64,
    total_data: u64,
    matched_data: u64,
    total_functions: u32,
    matched_functions: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    complete: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    module_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    module_id: Option<u32>,
    sections: Vec<LegacyReportItem>,
    functions: Vec<LegacyReportItem>,
}

impl From<LegacyReportUnit> for ReportUnit {
    fn from(value: LegacyReportUnit) -> Self {
        let mut measures = Measures {
            fuzzy_match_percent: value.fuzzy_match_percent,
            total_code: value.total_code,
            matched_code: value.matched_code,
            total_data: value.total_data,
            matched_data: value.matched_data,
            total_functions: value.total_functions,
            matched_functions: value.matched_functions,
            ..Default::default()
        };
        measures.calc_matched_percent();
        Self {
            name: value.name.clone(),
            measures: Some(measures),
            sections: value.sections.into_iter().map(ReportItem::from).collect(),
            functions: value.functions.into_iter().map(ReportItem::from).collect(),
            metadata: Some(ReportUnitMetadata {
                complete: value.complete,
                module_name: value.module_name.clone(),
                module_id: value.module_id,
                ..Default::default()
            }),
        }
    }
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
struct LegacyReportItem {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    demangled_name: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "serialize_hex",
        deserialize_with = "deserialize_hex"
    )]
    address: Option<u64>,
    size: u64,
    fuzzy_match_percent: f32,
}

impl From<LegacyReportItem> for ReportItem {
    fn from(value: LegacyReportItem) -> Self {
        Self {
            name: value.name,
            size: value.size,
            fuzzy_match_percent: value.fuzzy_match_percent,
            metadata: Some(ReportItemMetadata {
                demangled_name: value.demangled_name,
                virtual_address: value.address,
            }),
        }
    }
}

fn serialize_hex<S>(x: &Option<u64>, s: S) -> Result<S::Ok, S::Error>
where S: serde::Serializer {
    if let Some(x) = x {
        s.serialize_str(&format!("{:#x}", x))
    } else {
        s.serialize_none()
    }
}

fn deserialize_hex<'de, D>(d: D) -> Result<Option<u64>, D::Error>
where D: serde::Deserializer<'de> {
    use serde::Deserialize;
    let s = String::deserialize(d)?;
    if s.is_empty() {
        Ok(None)
    } else if !s.starts_with("0x") {
        Err(serde::de::Error::custom("expected hex string"))
    } else {
        u64::from_str_radix(&s[2..], 16).map(Some).map_err(serde::de::Error::custom)
    }
}
