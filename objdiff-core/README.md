# objdiff-core

objdiff-core contains the core functionality of [objdiff](https://github.com/encounter/objdiff), a tool for comparing object files in decompilation projects. See the main repository for more information.

## Crate feature flags

- **`all`**: Enables all main features.
- **`config`**: Enables objdiff configuration file support.
- **`dwarf`**: Enables extraction of line number information from DWARF debug sections.
- **`mips`**: Enables the MIPS backend powered by [rabbitizer](https://github.com/Decompollaborate/rabbitizer). (Note: C library with Rust bindings)
- **`ppc`**: Enables the PowerPC backend powered by [ppc750cl](https://github.com/encounter/ppc750cl).
- **`x86`**: Enables the x86 backend powered by [iced-x86](https://crates.io/crates/iced-x86).
- **`arm`**: Enables the ARM backend powered by [unarm](https://github.com/AetiasHax/unarm).
- **`arm64`**: Enables the ARM64 backend powered by [yaxpeax-arm](https://github.com/iximeow/yaxpeax-arm).
- **`bindings`**: Enables serialization and deserialization of objdiff data structures.
