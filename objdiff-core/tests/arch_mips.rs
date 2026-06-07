use objdiff_core::{diff, obj};

mod common;

#[test]
#[cfg(feature = "mips")]
fn read_mips() {
    let diff_config = diff::DiffObjConfig { mips_register_prefix: true, ..Default::default() };
    let obj =
        obj::read::parse(include_object!("data/mips/main.c.o"), &diff_config, diff::DiffSide::Base)
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
    let obj_be = obj::read::parse(
        include_object!("data/mips/code_be.o"),
        &diff_config,
        diff::DiffSide::Base,
    )
    .unwrap();
    assert_eq!(obj_be.endianness, object::Endianness::Big);
    let obj_le = obj::read::parse(
        include_object!("data/mips/code_le.o"),
        &diff_config,
        diff::DiffSide::Base,
    )
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
        diff::DiffSide::Base,
    )
    .unwrap();
    insta::assert_debug_snapshot!(obj.symbols);
}

#[test]
#[cfg(feature = "mips")]
fn ido_mdebug_line_numbers() {
    let diff_config = diff::DiffObjConfig::default();
    let obj = obj::read::parse(
        include_object!("data/mips/ido_lines_example.o"),
        &diff_config,
        diff::DiffSide::Base,
    )
    .unwrap();

    let text_section = obj.sections.iter().find(|s| s.name == ".text").unwrap();
    assert_eq!(text_section.line_info.get(&0), Some(&6));
    assert_eq!(text_section.line_info.get(&12), Some(&7));
    assert_eq!(text_section.line_info.get(&56), Some(&9));
    assert_eq!(text_section.line_info.len(), 66);
}

#[test]
#[cfg(feature = "mips")]
fn mwcc_dwarf1_line_numbers_multiple_functions() {
    let diff_config = diff::DiffObjConfig::default();
    let obj = obj::read::parse(
        include_object!("data/mips/mwcc_lines_example.o"),
        &diff_config,
        diff::DiffSide::Base,
    )
    .unwrap();

    for function_name in ["foo", "bar"] {
        let symbol = obj.symbols.iter().find(|s| s.name == function_name).unwrap();
        let section_idx = symbol.section.unwrap();
        let section = &obj.sections[section_idx];
        assert!(
            section.line_info.values().any(|line| *line > 0),
            "{function_name} should have valid line numbers"
        );
    }
}

#[test]
#[cfg(feature = "mips")]
fn ee_gcc_mdebug_line_numbers() {
    let diff_config = diff::DiffObjConfig::default();
    let obj = obj::read::parse(
        include_object!("data/mips/ee_gcc_lines_example.o"),
        &diff_config,
        diff::DiffSide::Base,
    )
    .unwrap();

    let text_section = obj.sections.iter().find(|s| s.name == ".text").unwrap();
    assert_eq!(text_section.line_info.get(&0), Some(&1));
    assert_eq!(text_section.line_info.get(&12), Some(&2));
    assert_eq!(text_section.line_info.get(&20), Some(&4));
    assert_eq!(text_section.line_info.get(&40), Some(&5));
    assert_eq!(text_section.line_info.get(&64), Some(&7));
    assert_eq!(text_section.line_info.get(&80), Some(&8));
    assert_eq!(text_section.line_info.get(&88), Some(&10));
    assert_eq!(text_section.line_info.get(&96), Some(&11));
    assert_eq!(text_section.line_info.get(&104), Some(&12));
    assert_eq!(text_section.line_info.get(&144), Some(&13));
    assert_eq!(text_section.line_info.len(), 10);
}
