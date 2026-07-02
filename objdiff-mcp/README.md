# objdiff-mcp

A [Model Context Protocol](https://modelcontextprotocol.io) server that exposes
[objdiff](https://github.com/encounter/objdiff)'s diffing engine so a model can
drive decompilation matching without any UI.

It's designed to close the loop with an IDA bridge: read the reference in IDA →
write/adjust C/C++ → compile to an object → **`diff_function`** against the
baseline → read the per-instruction diff → repeat until 100%.

```
  IDA (reference)         decomp project          objdiff-mcp (this)
  ida-bridge MCP   →   edit C++ → build     →   diff_function → match% + instr diff
        ▲                                              │
        └──────────────  agent iterates  ◀────────────┘
```

## Build

```bash
cargo build --release -p objdiff-mcp
# binary: target/release/objdiff-mcp
```

Built on `objdiff-core` (all architectures: ARM, ARM64, MIPS, PPC, SuperH,
x86/x86_64) with COFF + ELF support.

## Run

The server is persistent — it runs until closed and holds project/config state
across calls.

**Persistent HTTP instance** (recommended; e.g. on the Windows build VM next to
the compiled objects, reached from the agent over the network):

```bash
objdiff-mcp --transport http --bind 0.0.0.0:3001 [--project C:\path\to\project]
# MCP endpoint: http://<host>:3001/mcp
```

**stdio** (spawned by the client via `.mcp.json`):

```bash
objdiff-mcp                      # --transport stdio is the default
```

Logs go to stderr; stdout is reserved for the protocol on stdio.

## Tools

| Tool | Purpose |
|---|---|
| `open_project` | Load an `objdiff.json` so later calls refer to **units** by name instead of file paths. |
| `list_units` | List the project's units with their resolved target/base object paths (optional name filter). |
| `build` | Run the project's build command for a unit's base (or target) object; returns command line, exit status, and compiler output. |
| `diff_function` | Diff one function between the target (expected/baseline) and base (current/your build). Returns the match percent and a **side-by-side, per-instruction diff** with mismatch markers. The primary matching tool. |
| `diff_overview` | List every function in the object pair with its match percent, worst first. Use to pick what to work on. |
| `set_config` | Set a persistent objdiff config option (e.g. `x86.formatter`, `spaceBetweenArgs`, `demangler`) applied to subsequent diffs. |
| `version` | Report the server version. |

`diff_function` / `diff_overview` take **either** a project `unit` **or** explicit
`target`+`base` object-file paths, plus an optional per-call `config` map of
objdiff config overrides. Mismatch marker legend:
`~` replace · `o` opcode-mismatch · `a` arg-mismatch · `+` insert · `-` delete.

## The matching loop

1. `open_project` once (or `--project` at startup).
2. Understand the target function in IDA (via the IDA bridge).
3. Edit the C/C++ for the unit.
4. `build` the unit.
5. `diff_function(unit, symbol)` — read the match % and the side-by-side diff.
6. Adjust based on the mismatching instructions; cross-check offsets/targets in
   IDA. Go to 3. Repeat until 100%.

Use `diff_overview(unit, only_mismatches=true)` to triage which functions to
attack first.

## Connecting the agent

**HTTP (shared instance):** point your MCP client at `http://<host>:3001/mcp`.

**stdio (`.mcp.json`):**

```json
{
  "mcpServers": {
    "objdiff": { "command": "/path/to/objdiff-mcp" }
  }
}
```

## Note on the baseline

objdiff compares two objects: the **target** (the original/expected function's
machine code, the "baseline") and the **base** (your current build). Producing
the baseline object — extracting the original function's bytes into a COFF/ELF
object with name/xref-derived relocations — is a build/extraction step outside
objdiff (best done IDA-side, where the names and xrefs live). Point `target_path`
in `objdiff.json` at that file.
