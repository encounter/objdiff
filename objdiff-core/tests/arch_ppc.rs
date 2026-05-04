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
    let obj =
        obj::read::parse(include_object!("data/ppc/IObj.o"), &diff_config, diff::DiffSide::Base)
            .unwrap();
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
    let obj = obj::read::parse(
        include_object!("data/ppc/m_Do_hostIO.o"),
        &diff_config,
        diff::DiffSide::Base,
    )
    .unwrap();
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
    let obj = obj::read::parse(
        include_object!("data/ppc/NMWException.o"),
        &diff_config,
        diff::DiffSide::Base,
    )
    .unwrap();
    insta::assert_debug_snapshot!(obj);
}

#[test]
#[cfg(feature = "ppc")]
fn diff_ppc() {
    let diff_config = diff::DiffObjConfig::default();
    let mapping_config = diff::MappingConfig::default();
    let target_obj = obj::read::parse(
        include_object!("data/ppc/CDamageVulnerability_target.o"),
        &diff_config,
        diff::DiffSide::Target,
    )
    .unwrap();
    let base_obj = obj::read::parse(
        include_object!("data/ppc/CDamageVulnerability_base.o"),
        &diff_config,
        diff::DiffSide::Base,
    )
    .unwrap();
    let diff =
        diff::diff_objs(Some(&target_obj), Some(&base_obj), None, &diff_config, &mapping_config)
            .unwrap();

    let target_diff = diff.left.as_ref().unwrap();
    let base_diff = diff.right.as_ref().unwrap();
    let sections_display = display::display_sections(
        &target_obj,
        target_diff,
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

#[test]
#[cfg(feature = "ppc")]
fn read_vmx128_coff() {
    let diff_config = diff::DiffObjConfig { combine_data_sections: true, ..Default::default() };
    let obj = obj::read::parse(
        include_object!("data/ppc/vmx128.obj"),
        &diff_config,
        diff::DiffSide::Base,
    )
    .unwrap();
    insta::assert_debug_snapshot!(obj);
    let symbol_idx =
        obj.symbols.iter().position(|s| s.name == "?FloatingPointExample@@YAXXZ").unwrap();
    let diff = diff::code::no_diff_code(&obj, symbol_idx, &diff_config).unwrap();
    insta::assert_debug_snapshot!(diff.instruction_rows);
    let output = common::display_diff(&obj, &diff, symbol_idx, &diff_config);
    insta::assert_snapshot!(output);
}

#[test]
#[cfg(feature = "ppc")]
fn decode_sjis_pooled_strings() {
    // Test multiple pooled Shift JIS strings separated by a single null byte between entries. (MWCC)
    let diff_config = diff::DiffObjConfig { combine_data_sections: true, ..Default::default() };
    let obj = obj::read::parse(
        include_object!("data/ppc/m_Do_hostIO.o"),
        &diff_config,
        diff::DiffSide::Base,
    )
    .unwrap();
    common::assert_literal_value(
        &obj,
        &diff_config,
        "createChild__16mDoHIO_subRoot_cFPCcP13JORReflexible",
        15,
        "Shift JIS",
        "危険：既に登録されているホストIOをふたたび登録しようとしています<%s>\n",
    );
    common::assert_literal_value(
        &obj,
        &diff_config,
        "createChild__16mDoHIO_subRoot_cFPCcP13JORReflexible",
        42,
        "Shift JIS",
        "ホストIOの空きエントリがありません。登録できませんでした。\n",
    );
}

#[test]
#[cfg(feature = "ppc")]
fn decode_utf16_unpooled_strings() {
    // Test unpooled UTF-16BE wide strings with null bytes at the start, end, and in the middle of the string. (MSVC)
    let diff_config = diff::DiffObjConfig { combine_data_sections: true, ..Default::default() };
    let obj = obj::read::parse(
        include_object!("data/ppc/KinectSharePanel.obj"),
        &diff_config,
        diff::DiffSide::Base,
    )
    .unwrap();
    common::assert_literal_value(
        &obj,
        &diff_config,
        "?OnPostLink@KinectSharePanel@@AAA?AVDataNode@@PAVDataArray@@@Z",
        84,
        "UTF-16BE",
        "Title Text",
    );
    common::assert_literal_value(
        &obj,
        &diff_config,
        "?OnPostLink@KinectSharePanel@@AAA?AVDataNode@@PAVDataArray@@@Z",
        120,
        "UTF-16BE",
        "http://www.dancecentral.com/content-assets/2012/06/2012E3LogoBox_tn.jpg",
    );
}

#[test]
#[cfg(feature = "ppc")]
fn decode_ascii_strings_with_null_padding() {
    // Test unpooled ASCII strings with more than one null byte at the end.
    let diff_config = diff::DiffObjConfig { combine_data_sections: true, ..Default::default() };
    let obj = obj::read::parse(
        include_object!("data/ppc/vmx128.obj"),
        &diff_config,
        diff::DiffSide::Base,
    )
    .unwrap();
    common::assert_literal_value(
        &obj,
        &diff_config,
        "?PrintVector@@YAXPBDU__vector4@@@Z",
        24,
        "ASCII",
        "%s: [%.2f, %.2f, %.2f, %.2f]\n",
    );
    common::assert_literal_value(
        &obj,
        &diff_config,
        "?ReservedRegisterExample@@YAXXZ",
        59,
        "ASCII",
        "Result from vr66",
    );
}
