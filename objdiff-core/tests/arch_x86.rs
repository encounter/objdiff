use objdiff_core::{diff, diff::display::SymbolFilter, obj};

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

#[test]
#[cfg(feature = "x86")]
fn read_x86_64() {
    let diff_config = diff::DiffObjConfig::default();
    let obj = obj::read::parse(include_object!("data/x86_64/vs2022.o"), &diff_config).unwrap();
    insta::assert_debug_snapshot!(obj);
    let symbol_idx =
        obj.symbols.iter().position(|s| s.name == "?Dot@Vector@@QEAAMPEAU1@@Z").unwrap();
    let diff = diff::code::no_diff_code(&obj, symbol_idx, &diff_config).unwrap();
    insta::assert_debug_snapshot!(diff.instruction_rows);
    let output = common::display_diff(&obj, &diff, symbol_idx, &diff_config);
    insta::assert_snapshot!(output);
}

#[test]
#[cfg(feature = "x86")]
fn display_section_ordering() {
    let diff_config = diff::DiffObjConfig::default();
    let obj = obj::read::parse(include_object!("data/x86/basenode.obj"), &diff_config).unwrap();
    let obj_diff =
        diff::diff_objs(Some(&obj), None, None, &diff_config, &diff::MappingConfig::default())
            .unwrap()
            .left
            .unwrap();
    let section_display =
        diff::display::display_sections(&obj, &obj_diff, SymbolFilter::None, false, false, false);
    insta::assert_debug_snapshot!(section_display);
}

#[test]
#[cfg(feature = "x86")]
fn read_x86_jumptable() {
    let diff_config = diff::DiffObjConfig::default();
    let obj = obj::read::parse(include_object!("data/x86/jumptable.o"), &diff_config).unwrap();
    insta::assert_debug_snapshot!(obj);
    let symbol_idx = obj.symbols.iter().position(|s| s.name == "?test@@YAHH@Z").unwrap();
    let diff = diff::code::no_diff_code(&obj, symbol_idx, &diff_config).unwrap();
    insta::assert_debug_snapshot!(diff.instruction_rows);
    let output = common::display_diff(&obj, &diff, symbol_idx, &diff_config);
    insta::assert_snapshot!(output);
}

// Inferred size of functions should ignore symbols with specific prefixes
#[test]
#[cfg(feature = "x86")]
fn read_x86_local_labels() {
    let diff_config = diff::DiffObjConfig::default();
    let obj = obj::read::parse(include_object!("data/x86/local_labels.obj"), &diff_config).unwrap();
    insta::assert_debug_snapshot!(obj);
}
