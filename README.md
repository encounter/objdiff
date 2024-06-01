# objdiff [![Build Status]][actions]

[Build Status]: https://github.com/encounter/objdiff/actions/workflows/build.yaml/badge.svg
[actions]: https://github.com/encounter/objdiff/actions

A local diffing tool for decompilation projects. Inspired by [decomp.me](https://decomp.me) and [asm-differ](https://github.com/simonlindholm/asm-differ).

Features:

- Compare entire object files: functions and data.
- Built-in symbol demangling for C++. (CodeWarrior, Itanium & MSVC)
- Automatic rebuild on source file changes.
- Project integration via [configuration file](#configuration).
- Search and filter all of a project's objects and quickly switch.
- Click to highlight all instances of values and registers.

Supports:

- PowerPC 750CL (GameCube, Wii)
- MIPS (N64, PS1, PS2, PSP)
- x86 (COFF only at the moment)
- ARM (DS)

See [Usage](#usage) for more information.

![Symbol Screenshot](assets/screen-symbols.png)
![Diff Screenshot](assets/screen-diff.png)

## Usage

objdiff works by comparing two relocatable object files (`.o`). The objects are expected to have the same relative path
from the "target" and "base" directories.

For example, if the target ("expected") object is located at `build/asm/MetroTRK/mslsupp.o` and the base ("actual")
object is located at `build/src/MetroTRK/mslsupp.o`, the following configuration would be used:

- Target build directory: `build/asm`
- Base build directory: `build/src`
- Object: `MetroTRK/mslsupp.o`

objdiff will then execute the build system from the project directory to build both objects:

```sh
$ make build/asm/MetroTRK/mslsupp.o # Only if "Build target object" is enabled
$ make build/src/MetroTRK/mslsupp.o
```

The objects will then be compared and the results will be displayed in the UI.

See [Configuration](#configuration) for more information.

## Configuration

While **not required** (most settings can be specified in the UI), projects can add an `objdiff.json` (or
`objdiff.yaml`, `objdiff.yml`) file to configure the tool automatically. The configuration file must be located in
the root project directory.

If your project has a generator script (e.g. `configure.py`), it's recommended to generate the objdiff configuration
file as well. You can then add `objdiff.json` to your `.gitignore` to prevent it from being committed.

```json5
// objdiff.json
{
  custom_make: "ninja",
  custom_args: ["-d", "keeprsp"],

  // Only required if objects use "path" instead of "target_path" and "base_path".
  target_dir: "build/asm",
  base_dir: "build/src",

  build_target: true,
  watch_patterns: ["*.c", "*.cp", "*.cpp", "*.h", "*.hpp", "*.py"],
  objects: [
    {
      name: "main/MetroTRK/mslsupp",

      // Option 1: Relative to target_dir and base_dir
      path: "MetroTRK/mslsupp.o",
      // Option 2: Explicit paths from project root
      // Useful for more complex directory layouts
      target_path: "build/asm/MetroTRK/mslsupp.o",
      base_path: "build/src/MetroTRK/mslsupp.o",

      reverse_fn_order: false,
    },
    // ...
  ],
}
```

`custom_make` _(optional)_: By default, objdiff will use `make` to build the project.  
If the project uses a different build system (e.g. `ninja`), specify it here.  
The build command will be `[custom_make] [custom_args] path/to/object.o`.

`custom_args` _(optional)_: Additional arguments to pass to the build command prior to the object path.

`target_dir` _(optional)_: Relative from the root of the project, this where the "target" or "expected" objects are located.  
These are the **intended result** of the match.

`base_dir` _(optional)_: Relative from the root of the project, this is where the "base" or "actual" objects are located.  
These are objects built from the **current source code**.

`build_target`: If true, objdiff will tell the build system to build the target objects before diffing (e.g.
`make path/to/target.o`).  
This is useful if the target objects are not built by default or can change based on project configuration or edits
to assembly files.  
Requires the build system to be configured properly.

`watch_patterns` _(optional)_: A list of glob patterns to watch for changes.
([Supported syntax](https://docs.rs/globset/latest/globset/#syntax))  
If any of these files change, objdiff will automatically rebuild the objects and re-compare them.  
If not specified, objdiff will use the default patterns listed above.

`objects` _(optional)_: If specified, objdiff will display a list of objects in the sidebar for easy navigation.

> `name` _(optional)_: The name of the object in the UI. If not specified, the object's `path` will be used.
>
> `path`: Relative path to the object from the `target_dir` and `base_dir`.  
> Requires `target_dir` and `base_dir` to be specified.
>
> `target_path`: Path to the target object from the project root.  
> Required if `path` is not specified.
>
> `base_path`: Path to the base object from the project root.  
> Required if `path` is not specified.
>
> `reverse_fn_order` _(optional)_: Displays function symbols in reversed order.  
> Used to support MWCC's `-inline deferred` option, which reverses the order of functions in the object file.

## Building

Install Rust via [rustup](https://rustup.rs).

```shell
$ git clone https://github.com/encounter/objdiff.git
$ cd objdiff
$ cargo run --release
# or, for wgpu backend (recommended on macOS)
$ cargo run --release --features wgpu
```

## License

Licensed under either of

- Apache License, Version 2.0, ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in the work by you, as
defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
