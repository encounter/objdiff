use objdiff_core::{
    diff::{self, display},
    obj,
    obj::SectionKind,
};

mod common;

#[test]
#[cfg(feature = "ppc")]
fn read_ppc() {
    let diff_config = diff::DiffObjConfig::default();
    let obj = obj::read::parse(include_object!("data/ppc/IObj.o"), &diff_config).unwrap();
    insta::assert_debug_snapshot!(obj);
    let symbol_idx =
        obj.symbols.iter().position(|s| s.name == "Type2Text__10SObjectTagFUi").unwrap();
    let diff = diff::code::no_diff_code(&obj, symbol_idx, &diff_config).unwrap();
    insta::assert_debug_snapshot!(diff.instruction_rows);
    let output = common::display_diff(&obj, &diff, symbol_idx, &diff_config);
    insta::assert_snapshot!(output);
}

#[test]
#[cfg(feature = "ppc")]
fn read_dwarf1_line_info() {
    let diff_config = diff::DiffObjConfig::default();
    let obj = obj::read::parse(include_object!("data/ppc/m_Do_hostIO.o"), &diff_config).unwrap();
    let line_infos = obj
        .sections
        .iter()
        .filter(|s| s.kind == SectionKind::Code)
        .map(|s| s.line_info.clone())
        .collect::<Vec<_>>();
    insta::assert_debug_snapshot!(line_infos);
}

#[test]
#[cfg(feature = "ppc")]
fn read_extab() {
    let diff_config = diff::DiffObjConfig::default();
    let obj = obj::read::parse(include_object!("data/ppc/NMWException.o"), &diff_config).unwrap();
    insta::assert_debug_snapshot!(obj);
}

#[test]
#[cfg(feature = "ppc")]
fn diff_ppc() {
    let diff_config = diff::DiffObjConfig::default();
    let mapping_config = diff::MappingConfig::default();
    let target_obj =
        obj::read::parse(include_object!("data/ppc/CDamageVulnerability_target.o"), &diff_config)
            .unwrap();
    let base_obj =
        obj::read::parse(include_object!("data/ppc/CDamageVulnerability_base.o"), &diff_config)
            .unwrap();
    let diff =
        diff::diff_objs(Some(&target_obj), Some(&base_obj), None, &diff_config, &mapping_config)
            .unwrap();

    let target_diff = diff.left.as_ref().unwrap();
    let base_diff = diff.right.as_ref().unwrap();
    let sections_display = display::display_sections(
        &target_obj,
        &target_diff,
        display::SymbolFilter::None,
        false,
        false,
        true,
    );
    insta::assert_debug_snapshot!(sections_display);

    let target_symbol_idx = target_obj
        .symbols
        .iter()
        .position(|s| s.name == "WeaponHurts__20CDamageVulnerabilityCFRC11CWeaponModei")
        .unwrap();
    let target_symbol_diff = &target_diff.symbols[target_symbol_idx];
    let base_symbol_idx = base_obj
        .symbols
        .iter()
        .position(|s| s.name == "WeaponHurts__20CDamageVulnerabilityCFRC11CWeaponModei")
        .unwrap();
    let base_symbol_diff = &base_diff.symbols[base_symbol_idx];
    assert_eq!(target_symbol_diff.target_symbol, Some(base_symbol_idx));
    assert_eq!(base_symbol_diff.target_symbol, Some(target_symbol_idx));
    insta::assert_debug_snapshot!((target_symbol_diff, base_symbol_diff));
}
