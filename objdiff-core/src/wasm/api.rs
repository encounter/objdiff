use alloc::{
    format,
    str::FromStr,
    string::{String, ToString},
    vec::Vec,
};
use core::cell::RefCell;

use prost::Message;

use crate::{bindings::diff::DiffResult, diff, obj};

wit_bindgen::generate!({
    world: "api",
});

use exports::objdiff::core::diff::{
    DiffConfigBorrow, Guest as GuestTypes, GuestDiffConfig, GuestObject, Object, ObjectBorrow,
};

struct Component;

impl Guest for Component {
    fn init() -> Result<(), String> {
        // console_error_panic_hook::set_once();
        // #[cfg(debug_assertions)]
        // console_log::init_with_level(log::Level::Debug).map_err(|e| e.to_string())?;
        // #[cfg(not(debug_assertions))]
        // console_log::init_with_level(log::Level::Info).map_err(|e| e.to_string())?;
        Ok(())
    }

    fn version() -> String { env!("CARGO_PKG_VERSION").to_string() }
}

#[repr(transparent)]
struct ResourceDiffConfig(RefCell<diff::DiffObjConfig>);

impl GuestTypes for Component {
    type DiffConfig = ResourceDiffConfig;
    type Object = obj::ObjInfo;

    fn run_diff(
        left: Option<ObjectBorrow>,
        right: Option<ObjectBorrow>,
        diff_config: DiffConfigBorrow,
    ) -> Result<Vec<u8>, String> {
        let diff_config = diff_config.get::<ResourceDiffConfig>().0.borrow();
        let result = run_diff_internal(
            left.as_ref().map(|o| o.get()),
            right.as_ref().map(|o| o.get()),
            &diff_config,
            &diff::MappingConfig::default(),
        )
        .map_err(|e| e.to_string())?;
        Ok(result.encode_to_vec())
    }
}

impl GuestDiffConfig for ResourceDiffConfig {
    fn new() -> Self { Self(RefCell::new(diff::DiffObjConfig::default())) }

    fn set_property(&self, key: String, value: String) -> Result<(), String> {
        let id = diff::ConfigPropertyId::from_str(&key)
            .map_err(|_| format!("Invalid property key {:?}", key))?;
        self.0
            .borrow_mut()
            .set_property_value_str(id, &value)
            .map_err(|_| format!("Invalid property value {:?}", value))
    }

    fn get_property(&self, key: String) -> Result<String, String> {
        let id = diff::ConfigPropertyId::from_str(&key)
            .map_err(|_| format!("Invalid property key {:?}", key))?;
        Ok(self.0.borrow().get_property_value(id).to_string())
    }
}

impl GuestObject for obj::ObjInfo {
    fn parse(data: Vec<u8>, diff_config: DiffConfigBorrow) -> Result<Object, String> {
        let diff_config = diff_config.get::<ResourceDiffConfig>().0.borrow();
        obj::read::parse(&data, &diff_config).map(|o| Object::new(o)).map_err(|e| e.to_string())
    }
}

fn run_diff_internal(
    left: Option<&obj::ObjInfo>,
    right: Option<&obj::ObjInfo>,
    diff_config: &diff::DiffObjConfig,
    mapping_config: &diff::MappingConfig,
) -> anyhow::Result<DiffResult> {
    log::debug!("Running diff with config: {:?}", diff_config);
    let result = diff::diff_objs(diff_config, mapping_config, left, right, None)?;
    let left = left.and_then(|o| result.left.as_ref().map(|d| (o, d)));
    let right = right.and_then(|o| result.right.as_ref().map(|d| (o, d)));
    Ok(DiffResult::new(left, right))
}

export!(Component);
