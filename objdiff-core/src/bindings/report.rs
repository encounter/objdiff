#![allow(clippy::needless_lifetimes)] // Generated serde code
use std::ops::AddAssign;

use anyhow::{bail, Result};
use prost::Message;
use serde_json::error::Category;

// Protobuf report types
include!(concat!(env!("OUT_DIR"), "/objdiff.report.rs"));
include!(concat!(env!("OUT_DIR"), "/objdiff.report.serde.rs"));

pub const REPORT_VERSION: u32 = 2;

impl Report {
    /// Attempts to parse the report as binary protobuf or JSON.
    pub fn parse(data: &[u8]) -> Result<Self> {
        if data.is_empty() {
            bail!(std::io::Error::from(std::io::ErrorKind::UnexpectedEof));
        }
        let report = if data[0] == b'{' {
            // Load as JSON
            Self::from_json(data)?
        } else {
            // Load as binary protobuf
            Self::decode(data)?
        };
        Ok(report)
    }

    /// Attempts to parse the report as JSON, migrating from the legacy report format if necessary.
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

    /// Migrates the report to the latest version.
    /// Fails if the report version is newer than supported.
    pub fn migrate(&mut self) -> Result<()> {
        if self.version == 0 {
            self.migrate_v0()?;
        }
        if self.version == 1 {
            self.migrate_v1()?;
        }
        if self.version != REPORT_VERSION {
            bail!("Unsupported report version: {}", self.version);
        }
        Ok(())
    }

    /// Adds `complete_code`, `complete_data`, `complete_code_percent`, and `complete_data_percent`
    /// to measures, and sets `progress_categories` in unit metadata.
    fn migrate_v0(&mut self) -> Result<()> {
        let Some(measures) = &mut self.measures else {
            bail!("Missing measures in report");
        };
        for unit in &mut self.units {
            let Some(unit_measures) = &mut unit.measures else {
                bail!("Missing measures in report unit");
            };
            let mut complete = false;
            if let Some(metadata) = &mut unit.metadata {
                if metadata.module_name.is_some() || metadata.module_id.is_some() {
                    metadata.progress_categories = vec!["modules".to_string()];
                } else {
                    metadata.progress_categories = vec!["dol".to_string()];
                }
                complete = metadata.complete.unwrap_or(false);
            };
            if complete {
                unit_measures.complete_code = unit_measures.total_code;
                unit_measures.complete_data = unit_measures.total_data;
                unit_measures.complete_code_percent = 100.0;
                unit_measures.complete_data_percent = 100.0;
            } else {
                unit_measures.complete_code = 0;
                unit_measures.complete_data = 0;
                unit_measures.complete_code_percent = 0.0;
                unit_measures.complete_data_percent = 0.0;
            }
            measures.complete_code += unit_measures.complete_code;
            measures.complete_data += unit_measures.complete_data;
        }
        measures.calc_matched_percent();
        self.calculate_progress_categories();
        self.version = 1;
        Ok(())
    }

    /// Adds `total_units` and `complete_units` to measures.
    fn migrate_v1(&mut self) -> Result<()> {
        let Some(total_measures) = &mut self.measures else {
            bail!("Missing measures in report");
        };
        for unit in &mut self.units {
            let Some(measures) = &mut unit.measures else {
                bail!("Missing measures in report unit");
            };
            let complete = unit.metadata.as_ref().and_then(|m| m.complete).unwrap_or(false) as u32;
            let progress_categories =
                unit.metadata.as_ref().map(|m| m.progress_categories.as_slice()).unwrap_or(&[]);
            measures.total_units = 1;
            measures.complete_units = complete;
            total_measures.total_units += 1;
            total_measures.complete_units += complete;
            for id in progress_categories {
                if let Some(category) = self.categories.iter_mut().find(|c| &c.id == id) {
                    let Some(measures) = &mut category.measures else {
                        bail!("Missing measures in category");
                    };
                    measures.total_units += 1;
                    measures.complete_units += complete;
                }
            }
        }
        self.version = 2;
        Ok(())
    }

    /// Calculate progress categories based on unit metadata.
    pub fn calculate_progress_categories(&mut self) {
        for unit in &self.units {
            let Some(metadata) = unit.metadata.as_ref() else {
                continue;
            };
            let Some(measures) = unit.measures.as_ref() else {
                continue;
            };
            for category_id in &metadata.progress_categories {
                let category = match self.categories.iter_mut().find(|c| &c.id == category_id) {
                    Some(category) => category,
                    None => {
                        self.categories.push(ReportCategory {
                            id: category_id.clone(),
                            name: String::new(),
                            measures: Some(Default::default()),
                        });
                        self.categories.last_mut().unwrap()
                    }
                };
                *category.measures.get_or_insert_with(Default::default) += *measures;
            }
        }
        for category in &mut self.categories {
            let measures = category.measures.get_or_insert_with(Default::default);
            measures.calc_fuzzy_match_percent();
            measures.calc_matched_percent();
        }
    }

    /// Split the report into multiple reports based on progress categories.
    /// Assumes progress categories are in the format `version`, `version.category`.
    /// This is a hack for projects that generate all versions in a single report.
    pub fn split(self) -> Vec<(String, Report)> {
        let mut reports = Vec::new();
        // Map units to Option to allow taking ownership
        let mut units = self.units.into_iter().map(Some).collect::<Vec<_>>();
        for category in &self.categories {
            if category.id.contains(".") {
                // Skip subcategories
                continue;
            }
            fn is_sub_category(id: &str, parent: &str, sep: char) -> bool {
                id.starts_with(parent) && id.get(parent.len()..).is_some_and(|s| s.starts_with(sep))
            }
            let mut sub_categories = self
                .categories
                .iter()
                .filter(|c| is_sub_category(&c.id, &category.id, '.'))
                .cloned()
                .collect::<Vec<_>>();
            // Remove category prefix
            for sub_category in &mut sub_categories {
                sub_category.id = sub_category.id[category.id.len() + 1..].to_string();
            }
            let mut sub_units = units
                .iter_mut()
                .filter_map(|opt| {
                    let unit = opt.as_mut()?;
                    let metadata = unit.metadata.as_ref()?;
                    if metadata.progress_categories.contains(&category.id) {
                        opt.take()
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>();
            for sub_unit in &mut sub_units {
                // Remove leading version/ from unit name
                if let Some(name) =
                    sub_unit.name.strip_prefix(&category.id).and_then(|s| s.strip_prefix('/'))
                {
                    sub_unit.name = name.to_string();
                }
                // Filter progress categories
                let Some(metadata) = sub_unit.metadata.as_mut() else {
                    continue;
                };
                metadata.progress_categories = metadata
                    .progress_categories
                    .iter()
                    .filter(|c| is_sub_category(c, &category.id, '.'))
                    .map(|c| c[category.id.len() + 1..].to_string())
                    .collect();
            }
            reports.push((category.id.clone(), Report {
                measures: category.measures,
                units: sub_units,
                version: self.version,
                categories: sub_categories,
            }));
        }
        reports
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
        self.complete_code_percent = if self.total_code == 0 {
            100.0
        } else {
            self.complete_code as f32 / self.total_code as f32 * 100.0
        };
        self.complete_data_percent = if self.total_data == 0 {
            100.0
        } else {
            self.complete_data as f32 / self.total_data as f32 * 100.0
        };
    }
}

impl From<&ReportItem> for ChangeItemInfo {
    fn from(value: &ReportItem) -> Self {
        Self { fuzzy_match_percent: value.fuzzy_match_percent, size: value.size }
    }
}

impl AddAssign for Measures {
    fn add_assign(&mut self, other: Self) {
        self.fuzzy_match_percent += other.fuzzy_match_percent * other.total_code as f32;
        self.total_code += other.total_code;
        self.matched_code += other.matched_code;
        self.total_data += other.total_data;
        self.matched_data += other.matched_data;
        self.total_functions += other.total_functions;
        self.matched_functions += other.matched_functions;
        self.complete_code += other.complete_code;
        self.complete_data += other.complete_data;
        self.total_units += other.total_units;
        self.complete_units += other.complete_units;
    }
}

/// Allows [collect](Iterator::collect) to be used on an iterator of [Measures].
impl FromIterator<Measures> for Measures {
    fn from_iter<T>(iter: T) -> Self
    where T: IntoIterator<Item = Measures> {
        let mut measures = Measures::default();
        for other in iter {
            measures += other;
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
                ..Default::default()
            }),
            units: value.units.into_iter().map(ReportUnit::from).collect::<Vec<_>>(),
            ..Default::default()
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
