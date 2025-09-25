use objdiff_core::{diff, obj};

mod common;

#[test]
#[cfg(feature = "arm")]
fn read_arm() {
    let diff_config = diff::DiffObjConfig { ..Default::default() };
    let obj = obj::read::parse(
        include_object!("data/arm/LinkStateItem.o"),
        &diff_config,
        diff::DiffSide::Base,
    )
    .unwrap();
    insta::assert_debug_snapshot!(obj);
    let symbol_idx =
        obj.symbols.iter().position(|s| s.name == "_ZN13LinkStateItem12OnStateLeaveEi").unwrap();
    let diff = diff::code::no_diff_code(&obj, symbol_idx, &diff_config).unwrap();
    insta::assert_debug_snapshot!(diff.instruction_rows);
    let output = common::display_diff(&obj, &diff, symbol_idx, &diff_config);
    insta::assert_snapshot!(output);
}

#[test]
#[cfg(feature = "arm")]
fn read_thumb() {
    let diff_config = diff::DiffObjConfig { ..Default::default() };
    let obj =
        obj::read::parse(include_object!("data/arm/thumb.o"), &diff_config, diff::DiffSide::Base)
            .unwrap();
    insta::assert_debug_snapshot!(obj);
    let symbol_idx = obj
        .symbols
        .iter()
        .position(|s| s.name == "THUMB_BRANCH_ServerDisplay_UncategorizedMove")
        .unwrap();
    let diff = diff::code::no_diff_code(&obj, symbol_idx, &diff_config).unwrap();
    insta::assert_debug_snapshot!(diff.instruction_rows);
    let output = common::display_diff(&obj, &diff, symbol_idx, &diff_config);
    insta::assert_snapshot!(output);
}

#[test]
#[cfg(feature = "arm")]
fn combine_text_sections() {
    let diff_config = diff::DiffObjConfig { combine_text_sections: true, ..Default::default() };
    let obj = obj::read::parse(
        include_object!("data/arm/enemy300.o"),
        &diff_config,
        diff::DiffSide::Base,
    )
    .unwrap();
    let symbol_idx = obj.symbols.iter().position(|s| s.name == "Enemy300Draw").unwrap();
    let diff = diff::code::no_diff_code(&obj, symbol_idx, &diff_config).unwrap();
    insta::assert_debug_snapshot!(diff.instruction_rows);
    let output = common::display_diff(&obj, &diff, symbol_idx, &diff_config);
    insta::assert_snapshot!(output);
}

#[test]
#[cfg(feature = "arm")]
fn trim_trailing_hword() {
    let diff_config = diff::DiffObjConfig::default();
    let obj = obj::read::parse(
        include_object!("data/arm/issue_253.o"),
        &diff_config,
        diff::DiffSide::Base,
    )
    .unwrap();
    let symbol_idx = obj.symbols.iter().position(|s| s.name == "sub_8030F20").unwrap();
    let diff = diff::code::no_diff_code(&obj, symbol_idx, &diff_config).unwrap();
    insta::assert_debug_snapshot!(diff.instruction_rows);
    let output = common::display_diff(&obj, &diff, symbol_idx, &diff_config);
    insta::assert_snapshot!(output);
}
