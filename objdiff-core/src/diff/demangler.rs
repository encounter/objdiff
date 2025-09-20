use crate::diff::Demangler;

#[cfg(feature = "demangler")]
impl Demangler {
    pub fn demangle(&self, name: &str) -> Option<String> {
        match self {
            Demangler::Codewarrior => Self::demangle_codewarrior(name),
            Demangler::Msvc => Self::demangle_msvc(name),
            Demangler::GnuModern => Self::demangle_gnu_modern(name),
            Demangler::GnuV2 => Self::demangle_gnu_v2(name),
            Demangler::Auto => {
                // Try to guess
                if name.starts_with('?') {
                    Self::demangle_msvc(name)
                } else {
                    Self::demangle_codewarrior(name)
                        .or_else(|| Self::demangle_gnu_v2(name))
                        .or_else(|| Self::demangle_gnu_modern(name))
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

    fn demangle_gnu_modern(name: &str) -> Option<String> {
        cpp_demangle::Symbol::new(name)
            .ok()
            .and_then(|s| s.demangle(&cpp_demangle::DemangleOptions::default()).ok())
    }

    fn demangle_gnu_v2(name: &str) -> Option<String> {
        gnuv2_demangle::demangle(name, &gnuv2_demangle::DemangleConfig::new_no_cfilt_mimics()).ok()
    }
}

#[cfg(not(feature = "demangler"))]
impl Demangler {
    pub fn demangle(&self, _name: &str) -> Option<String> { None }
}
