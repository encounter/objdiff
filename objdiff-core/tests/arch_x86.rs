use objdiff_core::{diff, obj};

mod common;

#[test]
#[cfg(feature = "x86")]
fn read_x86() {
    let diff_config = diff::DiffObjConfig::default();
    let obj = obj::read::parse(include_object!("data/x86/staticdebug.obj"), &diff_config).unwrap();
    insta::assert_debug_snapshot!(obj);
    let symbol_idx = obj.symbols.iter().position(|s| s.name == "?PrintThing@@YAXXZ").unwrap();
    let diff = diff::code::no_diff_code(&obj, symbol_idx, &diff_config).unwrap();
    insta::assert_debug_snapshot!(diff.instruction_rows);
    let output = common::display_diff(&obj, &diff, symbol_idx, &diff_config);
    insta::assert_snapshot!(output);
}

#[test]
#[cfg(feature = "x86")]
fn read_x86_combine_sections() {
    let diff_config = diff::DiffObjConfig {
        combine_data_sections: true,
        combine_text_sections: true,
        ..Default::default()
    };
    let obj = obj::read::parse(include_object!("data/x86/rtest.obj"), &diff_config).unwrap();
    insta::assert_debug_snapshot!(obj.sections);
}
