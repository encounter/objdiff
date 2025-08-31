# objdiff [![Build Status]][actions]

[Build Status]: https://github.com/encounter/objdiff/actions/workflows/build.yaml/badge.svg
[actions]: https://github.com/encounter/objdiff/actions

A local diffing tool for decompilation projects. Inspired by [decomp.me](https://decomp.me) and [asm-differ](https://github.com/simonlindholm/asm-differ).

Features:

- Compare entire object files: functions and data
- Built-in C++ symbol demangling (GCC, MSVC, CodeWarrior, Itanium)
- Automatic rebuild on source file changes
- Project integration via [configuration file](#configuration)
- Search and filter objects with quick switching
- Click-to-highlight values and registers
- Detailed progress reporting (powers [decomp.dev](https://decomp.dev))
- WebAssembly API, [web interface](https://github.com/encounter/objdiff-web) and [Visual Studio Code extension](https://marketplace.visualstudio.com/items?itemName=decomp-dev.objdiff) (WIP)

Supports:

- ARM (GBA, DS, 3DS)
- ARM64 (Switch)
- MIPS (N64, PS1, PS2, PSP)
- PowerPC (GameCube, Wii, PS3, Xbox 360)
- SuperH (Saturn, Dreamcast)
- x86, x86_64 (PC)

See [Usage](#usage) for more information.

## Downloads

To build from source, see [Building](#building).

### GUI

- [Windows (x86_64)](https://github.com/encounter/objdiff/releases/latest/download/objdiff-windows-x86_64.exe)
- [Linux (x86_64)](https://github.com/encounter/objdiff/releases/latest/download/objdiff-linux-x86_64)
- [macOS (arm64)](https://github.com/encounter/objdiff/releases/latest/download/objdiff-macos-arm64)
- [macOS (x86_64)](https://github.com/encounter/objdiff/releases/latest/download/objdiff-macos-x86_64)

For Linux and macOS, run `chmod +x objdiff-*` to make the binary executable.

### CLI

CLI binaries are available on the [releases page](https://github.com/encounter/objdiff/releases).

## Screenshots

![Symbol Screenshot](assets/screen-symbols.png)
![Diff Screenshot](assets/screen-diff.png)

## Usage

objdiff compares two relocatable object files (`.o`). Here's how it works:

1. **Create an `objdiff.json` configuration file** in your project root (or generate it with your build script).  
  This file lists **all objects in the project** with their target ("expected") and base ("current") paths.

2. **Load the project** in objdiff.

3. **Select an object** from the sidebar to begin diffing.

4. **objdiff automatically:**
   - Executes the build system to compile the base object (from current source code)
   - Compares the two objects and displays the differences
   - Watches for source file changes and rebuilds when detected

The configuration file allows complete flexibility in project structure - your build directories can have any layout as long as the paths are specified correctly.

See [Configuration](#configuration) for setup details.

## Configuration

Projects can add an `objdiff.json` file to configure the tool automatically. The configuration file must be located in
the root project directory.

If your project has a generator script (e.g. `configure.py`), it's highly recommended to generate the objdiff configuration
file as well. You can then add `objdiff.json` to your `.gitignore` to prevent it from being committed.

```json
{
  "$schema": "https://raw.githubusercontent.com/encounter/objdiff/main/config.schema.json",
  "custom_make": "ninja",
  "custom_args": [
    "-d",
    "keeprsp"
  ],
  "build_target": false,
  "build_base": true,
  "watch_patterns": [
    "*.c",
    "*.cc",
    "*.cp",
    "*.cpp",
    "*.cxx",
    "*.c++",
    "*.h",
    "*.hh",
    "*.hp",
    "*.hpp",
    "*.hxx",
    "*.h++",
    "*.pch",
    "*.pch++",
    "*.inc",
    "*.s",
    "*.S",
    "*.asm",
    "*.py",
    "*.yml",
    "*.txt",
    "*.json"
  ],
  "ignore_patterns": [
    "build/**/*"
  ],
  "units": [
    {
      "name": "main/MetroTRK/mslsupp",
      "target_path": "build/asm/MetroTRK/mslsupp.o",
      "base_path": "build/src/MetroTRK/mslsupp.o",
      "metadata": {}
    }
  ]
}
```

### Schema

> [!NOTE]  
> View [config.schema.json](config.schema.json) for all available options. Below is a summary of the most important options.

#### Build Configuration

**`custom_make`** _(optional, default: `"make"`)_  
If the project uses a different build system (e.g. `ninja`), specify it here. The build command will be `[custom_make] [custom_args] path/to/object.o`.

**`custom_args`** _(optional)_  
Additional arguments to pass to the build command prior to the object path.

**`build_target`**  _(default: `false`)_  
If true, objdiff will build the target objects before diffing (e.g. `make path/to/target.o`). Useful if target objects are not built by default or can change based on project configuration. Requires proper build system configuration.

**`build_base`**  _(default: `true`)_  
If true, objdiff will build the base objects before diffing (e.g. `make path/to/base.o`). It's unlikely you'll want to disable this unless using an external tool to rebuild the base object.

#### File Watching

**`watch_patterns`** _(optional, default: listed above)_  
A list of glob patterns to watch for changes ([supported syntax](https://docs.rs/globset/latest/globset/#syntax)). When these files change, objdiff automatically rebuilds and re-compares objects.

**`ignore_patterns`** _(optional, default: listed above)_  
A list of glob patterns to explicitly ignore when watching for changes ([supported syntax](https://docs.rs/globset/latest/globset/#syntax)).

#### Units (Objects)

**`units`** _(optional)_  
If specified, objdiff displays a list of objects in the sidebar for easy navigation. Each unit contains:

- **`name`** _(optional)_ - The display name in the UI. Defaults to the object's `path`.
- **`target_path`** _(optional)_ - Path to the "target" or "expected" object (the **intended result**).
- **`base_path`** _(optional)_ - Path to the "base" or "current" object (built from **current source code**). Omit if there is no source object yet.
- **`metadata.auto_generated`** _(optional)_ - Hides the object from the sidebar but includes it in progress reports.
- **`metadata.complete`** _(optional)_ - Marks the object as "complete" (linked) when `true` or "incomplete" when `false`.

## Building

Install Rust via [rustup](https://rustup.rs).

```shell
git clone https://github.com/encounter/objdiff.git
cd objdiff
cargo run --release
```

Or install directly with cargo:

```shell
cargo install --locked --git https://github.com/encounter/objdiff.git objdiff-gui objdiff-cli
```

Binaries will be installed to `~/.cargo/bin` as `objdiff` and `objdiff-cli`.

## Contributing

Install `pre-commit` to run linting and formatting automatically:

```shell
rustup toolchain install nightly  # Required for cargo fmt/clippy
cargo install --locked cargo-deny # https://github.com/EmbarkStudios/cargo-deny
uv tool install pre-commit        # https://docs.astral.sh/uv, or use pipx or pip
pre-commit install
```

## License

Licensed under either of

- Apache License, Version 2.0, ([LICENSE-APACHE](LICENSE-APACHE) or <http://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or <http://opensource.org/licenses/MIT>)

at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in the work by you, as
defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
