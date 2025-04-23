pub mod diff;
pub mod report;

use std::str::FromStr;

use anyhow::{Context, Result, anyhow};
use objdiff_core::diff::{ConfigEnum, ConfigPropertyId, ConfigPropertyKind, DiffObjConfig};

pub fn apply_config_args(diff_config: &mut DiffObjConfig, args: &[String]) -> Result<()> {
    for config in args {
        let (key, value) = config.split_once('=').context("--config expects \"key=value\"")?;
        let property_id = ConfigPropertyId::from_str(key)
            .map_err(|()| anyhow!("Invalid configuration property: {}", key))?;
        diff_config.set_property_value_str(property_id, value).map_err(|()| {
            let mut options = String::new();
            match property_id.kind() {
                ConfigPropertyKind::Boolean => {
                    options = "true, false".to_string();
                }
                ConfigPropertyKind::Choice(variants) => {
                    for (i, variant) in variants.iter().enumerate() {
                        if i > 0 {
                            options.push_str(", ");
                        }
                        options.push_str(variant.value);
                    }
                }
            }
            anyhow!("Invalid value for {}. Expected one of: {}", property_id.name(), options)
        })?;
    }
    Ok(())
}
