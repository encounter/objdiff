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
fn thumb_short_data_mapping() {
    // When a .2byte directive is used in Thumb code, the assembler emits
    // $d/$t mapping symbols for a 2-byte data region. The disassembler must
    // not read 4 bytes as a .word when the next mapping symbol limits the
    // data region to 2 bytes.
    let diff_config = diff::DiffObjConfig::default();
    let obj = obj::read::parse(
        include_object!("data/arm/code_1_vblank.o"),
        &diff_config,
        diff::DiffSide::Base,
    )
    .unwrap();
    let symbol_idx = obj.symbols.iter().position(|s| s.name == "VBlankDMA_Level1").unwrap();
    let diff = diff::code::no_diff_code(&obj, symbol_idx, &diff_config).unwrap();
    let output = common::display_diff(&obj, &diff, symbol_idx, &diff_config);
    // .2byte data followed by Thumb code must not be merged into a 4-byte .word
    assert!(
        !output.contains(".word"),
        "2-byte data regions should not be decoded as 4-byte .word values.\n\
         The disassembler must respect mapping symbol boundaries."
    );
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
