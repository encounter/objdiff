use objdiff_core::{diff, obj};

mod common;

#[test]
#[cfg(feature = "mips")]
fn read_mips() {
    let diff_config = diff::DiffObjConfig { mips_register_prefix: true, ..Default::default() };
    let obj =
        obj::read::parse(include_object!("data/mips/main.c.o"), &diff_config, obj::DiffSide::Base)
            .unwrap();
    insta::assert_debug_snapshot!(obj);
    let symbol_idx = obj.symbols.iter().position(|s| s.name == "ControlEntry").unwrap();
    let diff = diff::code::no_diff_code(&obj, symbol_idx, &diff_config).unwrap();
    insta::assert_debug_snapshot!(diff.instruction_rows);
    let output = common::display_diff(&obj, &diff, symbol_idx, &diff_config);
    insta::assert_snapshot!(output);
}

#[test]
#[cfg(feature = "mips")]
fn cross_endian_diff() {
    let diff_config = diff::DiffObjConfig::default();
    let obj_be =
        obj::read::parse(include_object!("data/mips/code_be.o"), &diff_config, obj::DiffSide::Base)
            .unwrap();
    assert_eq!(obj_be.endianness, object::Endianness::Big);
    let obj_le =
        obj::read::parse(include_object!("data/mips/code_le.o"), &diff_config, obj::DiffSide::Base)
            .unwrap();
    assert_eq!(obj_le.endianness, object::Endianness::Little);
    let left_symbol_idx = obj_be.symbols.iter().position(|s| s.name == "func_00000000").unwrap();
    let right_symbol_idx =
        obj_le.symbols.iter().position(|s| s.name == "func_00000000__FPcPc").unwrap();
    let (left_diff, right_diff) =
        diff::code::diff_code(&obj_be, &obj_le, left_symbol_idx, right_symbol_idx, &diff_config)
            .unwrap();
    // Although the objects differ in endianness, the instructions should match.
    assert_eq!(left_diff.instruction_rows[0].kind, diff::InstructionDiffKind::None);
    assert_eq!(right_diff.instruction_rows[0].kind, diff::InstructionDiffKind::None);
    assert_eq!(left_diff.instruction_rows[1].kind, diff::InstructionDiffKind::None);
    assert_eq!(right_diff.instruction_rows[1].kind, diff::InstructionDiffKind::None);
    assert_eq!(left_diff.instruction_rows[2].kind, diff::InstructionDiffKind::None);
    assert_eq!(right_diff.instruction_rows[2].kind, diff::InstructionDiffKind::None);
}

#[test]
#[cfg(feature = "mips")]
fn filter_non_matching() {
    let diff_config = diff::DiffObjConfig::default();
    let obj = obj::read::parse(
        include_object!("data/mips/vw_main.c.o"),
        &diff_config,
        obj::DiffSide::Base,
    )
    .unwrap();
    insta::assert_debug_snapshot!(obj.symbols);
}
