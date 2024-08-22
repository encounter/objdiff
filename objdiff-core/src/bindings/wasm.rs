use prost::Message;
use wasm_bindgen::prelude::*;

use crate::{bindings::diff::DiffResult, diff, obj};

fn parse_object(
    data: Option<Box<[u8]>>,
    config: &diff::DiffObjConfig,
) -> Result<Option<obj::ObjInfo>, JsError> {
    data.as_ref().map(|data| obj::read::parse(data, config)).transpose().to_js()
}

fn parse_and_run_diff(
    left: Option<Box<[u8]>>,
    right: Option<Box<[u8]>>,
    config: diff::DiffObjConfig,
) -> Result<DiffResult, JsError> {
    let target = parse_object(left, &config)?;
    let base = parse_object(right, &config)?;
    run_diff(target.as_ref(), base.as_ref(), config)
}

fn run_diff(
    left: Option<&obj::ObjInfo>,
    right: Option<&obj::ObjInfo>,
    config: diff::DiffObjConfig,
) -> Result<DiffResult, JsError> {
    log::debug!("Running diff with config: {:?}", config);
    let result = diff::diff_objs(&config, left, right, None).to_js()?;
    let left = left.and_then(|o| result.left.as_ref().map(|d| (o, d)));
    let right = right.and_then(|o| result.right.as_ref().map(|d| (o, d)));
    Ok(DiffResult::new(left, right))
}

// #[wasm_bindgen]
// pub fn run_diff_json(
//     left: Option<Box<[u8]>>,
//     right: Option<Box<[u8]>>,
//     config: diff::DiffObjConfig,
// ) -> Result<String, JsError> {
//     let out = run_diff_opt_box(left, right, config)?;
//     serde_json::to_string(&out).map_err(|e| JsError::new(&e.to_string()))
// }

#[wasm_bindgen]
pub fn run_diff_proto(
    left: Option<Box<[u8]>>,
    right: Option<Box<[u8]>>,
    config: diff::DiffObjConfig,
) -> Result<Box<[u8]>, JsError> {
    let out = parse_and_run_diff(left, right, config)?;
    Ok(out.encode_to_vec().into_boxed_slice())
}

#[wasm_bindgen(start)]
fn start() -> Result<(), JsError> {
    console_error_panic_hook::set_once();
    #[cfg(debug_assertions)]
    console_log::init_with_level(log::Level::Debug).to_js()?;
    #[cfg(not(debug_assertions))]
    console_log::init_with_level(log::Level::Info).to_js()?;
    Ok(())
}

#[inline]
fn to_js_error(e: impl std::fmt::Display) -> JsError { JsError::new(&e.to_string()) }

trait ToJsResult {
    type Output;

    fn to_js(self) -> Result<Self::Output, JsError>;
}

impl<T, E: std::fmt::Display> ToJsResult for Result<T, E> {
    type Output = T;

    fn to_js(self) -> Result<T, JsError> { self.map_err(to_js_error) }
}
