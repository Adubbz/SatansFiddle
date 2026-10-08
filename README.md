# Satan's Fiddle

Satan's Fiddle runs a supported PlayStation 2 Metrowerks compiler under `wibo`
and LLDB. It controls compiler state that otherwise depends on earlier files or
uninitialized memory, so ordinary C++ can produce repeatable object code.

The compiler executable is selected by its SHA-256 digest. Address profiles are
specific to **2.3.3** and **3.0-011126**; `compiler_version` is an optional
consistency check, not permission to use an unknown executable. An unknown hash
or mismatched profile stops before compiler hooks are installed.

## Build and run

Use Linux x86-64, Rust with edition 2024 support, LLDB development headers and
`liblldb`, a C++ toolchain, and an unstripped `wibo` binary with a supported PE
loader symbol (`wibo::Executable::resolveImports` or the older
`loadPEFromSource`). Release binaries with stripped loader symbols cannot
establish the required compiler-loaded breakpoint. Keep the compiler's
support files beside its executable, including its normal include directories.

```sh
cargo build
cp config.example.json config.json
# Set compiler_path and wibo_path in config.json.
cargo run -- --config config.json --output sample.o tests/compile_sample/sample.cpp
```

If LLDB is outside the system include/library paths, set `LLDB_INCLUDE` and
`LLDB_LIB_DIR` for the build. At runtime the configured library directory is
included in the executable's rpath; LLDB's server and any additional library
dependencies must also be available. `LLDB_DEBUGSERVER_PATH` can select
`lldb-server` when it is not in LLDB's normal installation location.

Every CLI invocation performs one compilation. `--translation-unit NAME` can
provide the original source basename when a build adapter compiles generated
temporary input; it controls helper-mask and floating-point selector lookup.
The in-process configuration and compiler profile are initialized once; callers of `compile()` should use a
fresh process for each job. Compiler errors and hook validation errors return a
nonzero exit status. A completed ELF object is published only after the compiler
exits successfully and all configured selectors have been validated; a failed
job preserves any existing requested output.

## Configuration

JSON is the preferred configuration format. The existing compiler and helper-mask
keys retain their meanings; `floating_point` adds deterministic defaults and
optional fixes in the same file. See [config.example.json](config.example.json).
Existing TOML files remain supported; [config.example.toml](config.example.toml)
shows the equivalent settings. Format detection uses the leading JSON object
brace, so a renamed file does not silently select a different schema.

Unknown fields, malformed options, invalid selectors, and duplicate selectors
are errors.

| Field | Default | Purpose |
| --- | --- | --- |
| `compiler_path` | Required | Supported compiler executable. |
| `compiler_version` | Hash detection | Optional profile consistency check: `2.3.3` or `3.0-011126`. |
| `compiler_options` | Empty | Shell-style arguments before generated `-o OUTPUT INPUT`; quoted values remain one argument. |
| `wibo_path` | `WIBO_PATH`, then `PATH` | Debug-symbol `wibo` executable. |
| `default_gpr_helper_mask` | `0` | Initial GPR helper history for each translation unit. |
| `default_fpr_helper_mask` | `0` | Initial FPR helper history for each translation unit. |
| `translation_units` | Empty | Per-file `name`, `gpr_helper_mask`, `fpr_helper_mask` overrides. |
| `floating_point.default_evaluate_first` | `false` | Deterministic float-constant argument annotation. |
| `floating_point.default_literal_reload` | `true` | Conservatively reload pooled literals across unknown-address stores. |
| `floating_point.expression_overrides` | Empty | Exact constant identities with an `evaluate_first` Boolean. |
| `floating_point.literal_overrides` | Empty | Per-literal `reload` Boolean. |

Translation-unit selectors use source basenames; both slash styles in reported
paths are normalized. Files with the same basename therefore share a selector.
Function selectors use the compiler's mangled name, not a demangled display
name. Floating-point values use IEEE bits, preserving negative zero and NaN
payloads instead of matching decimal spellings or compiler arena addresses.

An expression selector supplies `translation_unit`, `function`, `value_type`
(`binary32` or `binary64`), `value_bits` (a hexadecimal string beginning `0x`),
and `evaluate_first`. An optional `callee` names the compiler's mangled callee
for a call-specific adjustment. All occurrences of the resulting stable identity
receive the same adjustment.
A binary32 value must fit 32 bits; a binary64 value must fit 64 bits. Unscoped
rows apply at annotation and constant-argument consumption. Callee-scoped rows
apply at argument consumption only; a matching scoped row takes precedence over
an unscoped row, independently of configuration ordering.

A literal selector supplies `translation_unit`, `value_type`, `value_bits`, and
`reload`. Its identity does not include the compiler's temporary literal symbol
name. Duplicate identities are rejected even when they specify the same value.
Occurrence, ordinal, read-position, and raw-address selector fields are unsupported
and rejected.

## Why these controls are necessary

**Helper-call history.** MWCC accumulates masks describing argument registers
read by compiler helpers such as double arithmetic and floating conversions.
The masks survive translation-unit boundaries in a whole-program invocation.
Compiling a file alone can therefore allocate registers differently. The tool
seeds the masks at translation-unit entry; subsequent helper calls accumulate
history normally. These are helper argument masks, not general register
reservation masks or ordinary function-call masks.

**Float constant evaluation order.** The float-constant annotation path copies
an evaluate-first byte from uninitialized front-end storage. The leaked byte
changes constant argument materialization and scheduling. The tool initializes
the byte before the compiler's float annotation path can read it. Normal
annotation can still assign its own value; an explicit constant identity override
is reapplied at the validated annotation return paths. Verification diagnostics
read back that byte after the write. Later compiler transformations can still
produce identical code for two policies; a selector does not guarantee a change
to the final instruction sequence. The affected byte and internal node
layout differ between compiler versions, so each profile uses validated offsets.
Optimization also creates fresh constant nodes after annotation. The argument
consumer initializes those direct constants before reading their flags. Explicit
selectors additionally recognize same-type assignment wrappers and references
to registered literal-pool objects; native wrapper flags stay intact unless a
selector matches. Type-changing conversions and arbitrary variable references
are not inferred as literal identities. The verified integral-float lowering
retains its original annotated IEEE identity and validates the converted child
before applying a selector. An unsupported or stale selector fails
the compilation instead of silently having no effect.

**Literal reuse across stores (2.3.3).** MWCC's alias query sometimes treats a pooled
float literal's value buffer as variable metadata. The resulting unrelated byte
can determine whether a literal load survives a store through an unknown
address. Literal hooks track compiler-created pool objects and apply the
configured reload policy only to those objects. Ordinary variables retain the
compiler's normal alias analysis. The 3.0-011126 image has no validated
affected alias path; its default leaves normal literal handling unchanged.
Explicit `default_literal_reload` settings or nonempty literal overrides are
rejected for that profile. Omit those keys when using 3.0-011126.

These defaults provide a repeatable baseline. Reproducing a particular retail
whole-program build can require its helper history and selected override values;
determinism alone does not establish a retail match. The tool does not replace
C++ implementations with inline assembly or rewrite emitted instruction bytes.

## Verification

```sh
cargo test
# Run the genuine compiler tests explicitly; neither compiler is bundled.
export SATANSFIDDLE_TEST_COMPILER_233=/absolute/path/to/mwccmips.exe
export SATANSFIDDLE_TEST_COMPILER_300=/absolute/path/to/mwccps2.exe
export SATANSFIDDLE_TEST_WIBO=/absolute/path/to/debug-symbol/wibo
cargo test --test compiler_cli -- --ignored --nocapture
```

The ordinary test run validates the typed configuration, selector identities,
widths, duplicate rejection, quoted arguments, and rejection of an unsupported
compiler. The genuine compiler suite is explicitly ignored until requested;
missing prerequisites fail that explicit run instead of producing a passing
placeholder. It compiles a public standalone C++ sample on both profiles,
compares repeat outputs, checks scheduling and literal-load differences,
exercises exact selectors across repeated and propagated constants, distinct
named callees, pooled references, integral-float lowering, selector precedence
and stale-selector rejection,
checks environment-provided header search and canonical translation-unit names,
and verifies compiler failures preserve existing outputs.

The tests require no game source or ROM. The 2.3.3 fixture uses `-O2` to exercise
pooled literals; 3.0-011126 uses `-O3,p` plus an `-O2` pooled-constant regression,
and checks rejection of unsupported literal controls. Set
`SATANSFIDDLE_TEST_KEEP_ARTIFACTS=1` to retain test objects for inspection.
`SATANSFIDDLE_VERIFY=1` prints hook counts and selected annotation-byte readbacks;
it does not print the process environment.
