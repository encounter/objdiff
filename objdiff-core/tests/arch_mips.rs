use objdiff_core::{diff, obj};

mod common;

#[test]
#[cfg(feature = "mips")]
fn read_mips() {
    let diff_config = diff::DiffObjConfig { mips_register_prefix: true, ..Default::default() };
    let obj = obj::read::parse(include_object!("data/mips/main.c.o"), &diff_config).unwrap();
    insta::assert_debug_snapshot!(obj);
    let symbol_idx = obj.symbols.iter().position(|s| s.name == "ControlEntry").unwrap();
    let diff = diff::code::no_diff_code(&obj, symbol_idx, &diff_config).unwrap();
    insta::assert_debug_snapshot!(diff.instruction_rows);
    let output = common::display_diff(&obj, &diff, symbol_idx, &diff_config);
    insta::assert_snapshot!(output);
}
