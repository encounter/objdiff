use objdiff_core::{diff, obj};

mod common;

fn parse(data: &[u8]) -> (obj::Object, diff::DiffObjConfig) {
    let diff_config = diff::DiffObjConfig::default();
    let obj = obj::read::parse(data, &diff_config, diff::DiffSide::Base).unwrap();
    (obj, diff_config)
}

/// Snapshots the parsed object, then the disassembly of `_main`, which holds every relocation
/// kind that the compiler emits for the test source.
fn read_macho(arch: &str, data: &[u8]) {
    let (obj, diff_config) = parse(data);
    insta::assert_debug_snapshot!(format!("{arch}_object"), obj);
    let symbol_idx = obj.symbols.iter().position(|s| s.name == "_main").unwrap();
    let diff = diff::code::no_diff_code(&obj, symbol_idx, &diff_config).unwrap();
    insta::assert_debug_snapshot!(format!("{arch}_instructions"), diff.instruction_rows);
    let output = common::display_diff(&obj, &diff, symbol_idx, &diff_config);
    insta::assert_snapshot!(format!("{arch}_display"), output);
}

#[test]
#[cfg(feature = "x86")]
fn read_macho_x86_64() { read_macho("x86_64", include_object!("data/macho/fibonacci_x86_64.o")); }

#[test]
#[cfg(feature = "x86")]
fn read_macho_i386() { read_macho("i386", include_object!("data/macho/fibonacci_i386.o")); }

#[test]
#[cfg(feature = "arm64")]
fn read_macho_arm64() { read_macho("arm64", include_object!("data/macho/fibonacci_arm64.o")); }

#[test]
#[cfg(feature = "arm64")]
fn read_macho_arm64_32() {
    read_macho("arm64_32", include_object!("data/macho/fibonacci_arm64_32.o"));
}

#[test]
#[cfg(feature = "arm")]
fn read_macho_arm() { read_macho("armv6", include_object!("data/macho/fibonacci_armv6.o")); }

/// Thumb functions are marked in the Mach-O symbol table rather than by mapping symbols.
#[test]
#[cfg(feature = "arm")]
fn read_macho_arm_thumb() {
    read_macho("armv6_thumb", include_object!("data/macho/fibonacci_armv6_thumb.o"));
}

/// unarm only disassembles up to ARMv6K, so only the parsed object is snapshotted for ARMv7.
#[test]
#[cfg(feature = "arm")]
fn read_macho_armv7() {
    let (obj, _) = parse(include_object!("data/macho/fibonacci_armv7.o"));
    insta::assert_debug_snapshot!("armv7_object", obj);
}
