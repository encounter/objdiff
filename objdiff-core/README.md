# objdiff-core

objdiff-core contains the core functionality of [objdiff](https://github.com/encounter/objdiff), a tool for comparing object files in decompilation projects. See the main repository for more information.

## Crate feature flags

- **`all`**: Enables all main features.
- **`bindings`**: Enables serialization and deserialization of objdiff data structures.
- **`config`**: Enables objdiff configuration file support.
- **`dwarf`**: Enables extraction of line number information from DWARF debug sections.
- **`arm64`**: Enables the ARM64 backend powered by [yaxpeax-arm](https://github.com/iximeow/yaxpeax-arm).
- **`arm`**: Enables the ARM backend powered by [unarm](https://github.com/AetiasHax/unarm).
- **`mips`**: Enables the MIPS backend powered by [rabbitizer](https://github.com/Decompollaborate/rabbitizer).
- **`ppc`**: Enables the PowerPC backend powered by [ppc750cl](https://github.com/encounter/ppc750cl).
- **`superh`**: Enables the SuperH backend powered by an included disassembler.
- **`x86`**: Enables the x86 backend powered by [iced-x86](https://crates.io/crates/iced-x86).
