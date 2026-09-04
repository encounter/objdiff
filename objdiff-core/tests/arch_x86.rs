use objdiff_core::{diff, diff::display::SymbolFilter, obj};

mod common;

#[test]
#[cfg(feature = "x86")]
fn read_x86() {
    let diff_config = diff::DiffObjConfig::default();
    let obj = obj::read::parse(
        include_object!("data/x86/staticdebug.obj"),
        &diff_config,
        diff::DiffSide::Base,
    )
    .unwrap();
    insta::assert_debug_snapshot!(obj);
    let symbol_idx = obj.symbols.iter().position(|s| s.name == "?PrintThing@@YAXXZ").unwrap();
    let diff = diff::code::no_diff_code(&obj, symbol_idx, &diff_config).unwrap();
    insta::assert_debug_snapshot!(diff.instruction_rows);
    let output = common::display_diff(&obj, &diff, symbol_idx, &diff_config);
    insta::assert_snapshot!(output);
}

#[test]
#[cfg(feature = "x86")]
fn diff_single_x86_symbol() {
    let diff_config = diff::DiffObjConfig::default();
    let obj = obj::read::parse(
        include_object!("data/x86/staticdebug.obj"),
        &diff_config,
        diff::DiffSide::Target,
    )
    .unwrap();
    let symbol_name = "?PrintThing@@YAXXZ";
    let symbol_idx = obj.symbol_by_name(symbol_name).unwrap();

    let result = diff::diff_objs_for_symbol(
        Some(&obj),
        Some(&obj),
        symbol_name,
        &diff_config,
        &diff::MappingConfig::default(),
    )
    .unwrap();
    let left = result.left.unwrap();
    let right = result.right.unwrap();

    assert_eq!(left.symbols.len(), obj.symbols.len());
    assert_eq!(right.symbols.len(), obj.symbols.len());
    assert_eq!(left.symbols[symbol_idx].target_symbol, Some(symbol_idx));
    assert_eq!(right.symbols[symbol_idx].target_symbol, Some(symbol_idx));
    assert!(!left.symbols[symbol_idx].instruction_rows.is_empty());
    assert!(!right.symbols[symbol_idx].instruction_rows.is_empty());
    assert!(left.sections.iter().all(|section| section.data_diff.is_empty()));
    assert!(right.sections.iter().all(|section| section.data_diff.is_empty()));
    assert!(
        left.symbols
            .iter()
            .enumerate()
            .filter(|(idx, _)| *idx != symbol_idx)
            .all(|(_, symbol)| symbol.instruction_rows.is_empty())
    );
    assert!(
        right
            .symbols
            .iter()
            .enumerate()
            .filter(|(idx, _)| *idx != symbol_idx)
            .all(|(_, symbol)| symbol.instruction_rows.is_empty())
    );
}

#[test]
#[cfg(feature = "x86")]
fn diff_single_symbol_falls_back_from_missing_mapping() {
    let diff_config = diff::DiffObjConfig::default();
    let obj = obj::read::parse(
        include_object!("data/x86/staticdebug.obj"),
        &diff_config,
        diff::DiffSide::Target,
    )
    .unwrap();
    let symbol_name = "?PrintThing@@YAXXZ";
    let symbol_idx = obj.symbol_by_name(symbol_name).unwrap();
    let mut mapping_config = diff::MappingConfig::default();
    mapping_config.mappings.insert(symbol_name.into(), "missing".into());

    let result = diff::diff_objs_for_symbol(
        Some(&obj),
        Some(&obj),
        symbol_name,
        &diff_config,
        &mapping_config,
    )
    .unwrap();

    assert_eq!(result.left.unwrap().symbols[symbol_idx].target_symbol, Some(symbol_idx));
    assert_eq!(result.right.unwrap().symbols[symbol_idx].target_symbol, Some(symbol_idx));
}

#[test]
fn diff_single_common_symbol() {
    let common_symbol = obj::Symbol {
        name: "common".into(),
        size: 4,
        flags: obj::SymbolFlag::Common.into(),
        ..Default::default()
    };
    let left = obj::Object { symbols: vec![common_symbol.clone()], ..Default::default() };
    let right = obj::Object { symbols: vec![common_symbol], ..Default::default() };

    let result = diff::diff_objs_for_symbol(
        Some(&left),
        Some(&right),
        "common",
        &diff::DiffObjConfig::default(),
        &diff::MappingConfig::default(),
    )
    .unwrap();

    assert_eq!(result.left.unwrap().symbols[0].target_symbol, Some(0));
    assert_eq!(result.right.unwrap().symbols[0].target_symbol, Some(0));
}

#[test]
#[cfg(feature = "x86")]
fn diff_x86_summary_omits_details() {
    let diff_config = diff::DiffObjConfig::default();
    let left = obj::read::parse(
        include_object!("data/x86/staticdebug.obj"),
        &diff_config,
        diff::DiffSide::Target,
    )
    .unwrap();
    let right = obj::read::parse(
        include_object!("data/x86/staticdebug.obj"),
        &diff_config,
        diff::DiffSide::Base,
    )
    .unwrap();

    let result = diff::diff_objs_summary(
        Some(&left),
        Some(&right),
        None,
        &diff_config,
        &diff::MappingConfig::default(),
    )
    .unwrap();
    let left_diff = result.left.unwrap();
    let right_diff = result.right.unwrap();

    assert!(
        left_diff
            .symbols
            .iter()
            .filter(|symbol| symbol.target_symbol.is_some())
            .all(|symbol| symbol.match_percent == Some(100.0))
    );
    assert!(left_diff.symbols.iter().all(|symbol| symbol.instruction_rows.is_empty()));
    assert!(right_diff.symbols.iter().all(|symbol| symbol.instruction_rows.is_empty()));
    assert!(left_diff.sections.iter().all(|section| section.data_diff.is_empty()));
    assert!(right_diff.sections.iter().all(|section| section.data_diff.is_empty()));
}

#[test]
#[cfg(feature = "x86")]
fn read_x86_combine_sections() {
    let diff_config = diff::DiffObjConfig {
        combine_data_sections: true,
        combine_text_sections: true,
        ..Default::default()
    };
    let obj =
        obj::read::parse(include_object!("data/x86/rtest.obj"), &diff_config, diff::DiffSide::Base)
            .unwrap();
    insta::assert_debug_snapshot!(obj.sections);
}

#[test]
#[cfg(feature = "x86")]
fn read_x86_64() {
    let diff_config = diff::DiffObjConfig::default();
    let obj = obj::read::parse(
        include_object!("data/x86_64/vs2022.o"),
        &diff_config,
        diff::DiffSide::Base,
    )
    .unwrap();
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
    let obj = obj::read::parse(
        include_object!("data/x86/basenode.obj"),
        &diff_config,
        diff::DiffSide::Base,
    )
    .unwrap();
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
    let obj = obj::read::parse(
        include_object!("data/x86/jumptable.o"),
        &diff_config,
        diff::DiffSide::Base,
    )
    .unwrap();
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
    let obj = obj::read::parse(
        include_object!("data/x86/local_labels.obj"),
        &diff_config,
        diff::DiffSide::Base,
    )
    .unwrap();
    insta::assert_debug_snapshot!(obj);
}

#[test]
#[cfg(feature = "x86")]
fn read_x86_indirect_table() {
    let diff_config = diff::DiffObjConfig::default();
    let obj = obj::read::parse(
        include_object!("data/x86/indirect_table.obj"),
        &diff_config,
        diff::DiffSide::Base,
    )
    .unwrap();
    insta::assert_debug_snapshot!(obj);
    let symbol_idx = obj.symbols.iter().position(|s| s.name == "?process@@YAHHHH@Z").unwrap();
    let diff = diff::code::no_diff_code(&obj, symbol_idx, &diff_config).unwrap();
    insta::assert_debug_snapshot!(diff.instruction_rows);
    let output = common::display_diff(&obj, &diff, symbol_idx, &diff_config);
    insta::assert_snapshot!(output);
}
