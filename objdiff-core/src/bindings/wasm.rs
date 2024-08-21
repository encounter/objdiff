use anyhow::Context;
use prost::Message;
use wasm_bindgen::prelude::*;

use crate::{bindings::diff::DiffResult, diff, obj};

#[wasm_bindgen]
pub fn run_diff(
    left: Option<Box<[u8]>>,
    right: Option<Box<[u8]>>,
    config: diff::DiffObjConfig,
) -> Result<String, JsError> {
    let target = left
        .as_ref()
        .map(|data| obj::read::parse(data, &config).context("Loading target"))
        .transpose()
        .map_err(|e| JsError::new(&e.to_string()))?;
    let base = right
        .as_ref()
        .map(|data| obj::read::parse(data, &config).context("Loading base"))
        .transpose()
        .map_err(|e| JsError::new(&e.to_string()))?;
    let result = diff::diff_objs(&config, target.as_ref(), base.as_ref(), None)
        .map_err(|e| JsError::new(&e.to_string()))?;
    let left = target.as_ref().and_then(|o| result.left.as_ref().map(|d| (o, d)));
    let right = base.as_ref().and_then(|o| result.right.as_ref().map(|d| (o, d)));
    let out = DiffResult::new(left, right);
    serde_json::to_string(&out).map_err(|e| JsError::new(&e.to_string()))
}

#[wasm_bindgen]
pub fn run_diff_proto(
    left: Option<Box<[u8]>>,
    right: Option<Box<[u8]>>,
    config: diff::DiffObjConfig,
) -> Result<Box<[u8]>, JsError> {
    let target = left
        .as_ref()
        .map(|data| obj::read::parse(data, &config).context("Loading target"))
        .transpose()
        .map_err(|e| JsError::new(&e.to_string()))?;
    let base = right
        .as_ref()
        .map(|data| obj::read::parse(data, &config).context("Loading base"))
        .transpose()
        .map_err(|e| JsError::new(&e.to_string()))?;
    let result = diff::diff_objs(&config, target.as_ref(), base.as_ref(), None)
        .map_err(|e| JsError::new(&e.to_string()))?;
    let left = target.as_ref().and_then(|o| result.left.as_ref().map(|d| (o, d)));
    let right = base.as_ref().and_then(|o| result.right.as_ref().map(|d| (o, d)));
    let out = DiffResult::new(left, right);
    Ok(out.encode_to_vec().into_boxed_slice())
}
