use alloc::string::String;

use crate::diff::Demangler;

#[cfg(feature = "demangler")]
impl Demangler {
    pub fn demangle(&self, name: &str) -> Option<String> {
        match self {
            Demangler::Codewarrior => Self::demangle_codewarrior(name),
            Demangler::Msvc => Self::demangle_msvc(name),
            Demangler::Itanium => Self::demangle_itanium(name),
            Demangler::GnuLegacy => Self::demangle_gnu_legacy(name),
            Demangler::Auto => {
                // Try to guess
                if name.starts_with('?') {
                    Self::demangle_msvc(name)
                } else {
                    Self::demangle_codewarrior(name)
                        .or_else(|| Self::demangle_gnu_legacy(name))
                        .or_else(|| Self::demangle_itanium(name))
                }
            }
        }
    }

    fn demangle_codewarrior(name: &str) -> Option<String> {
        cwdemangle::demangle(name, &cwdemangle::DemangleOptions::default())
    }

    fn demangle_msvc(name: &str) -> Option<String> {
        msvc_demangler::demangle(name, msvc_demangler::DemangleFlags::llvm()).ok()
    }

    fn demangle_itanium(name: &str) -> Option<String> {
        let name = name.trim_start_matches('.');
        cpp_demangle::Symbol::new(name)
            .ok()
            .and_then(|s| s.demangle(&cpp_demangle::DemangleOptions::default()).ok())
    }

    fn demangle_gnu_legacy(name: &str) -> Option<String> {
        let name = name.trim_start_matches('.');
        gnuv2_demangle::demangle(name, &gnuv2_demangle::DemangleConfig::new_no_cfilt_mimics()).ok()
    }
}

#[cfg(not(feature = "demangler"))]
impl Demangler {
    pub fn demangle(&self, _name: &str) -> Option<String> { None }
}
