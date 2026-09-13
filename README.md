# Satan's Fiddle

Satan's Fiddle is a tool designed to commandeer MWCC to make it play nice. The general aim is to allow non-deterministic behaviours to be configurable at compile time and generally to co-opt the compiler's internal state. At this stage the project is only targeting Linux and runs the compiler via `wibo`, though this could potentially change down the line.

## Configuration

| Field | Required | Description |
| --- | --- | --- |
| `compiler_path` | Yes | Path to the MWCC executable that `wibo` will run. |
| `compiler_version` | No | Compiler version fallback used when the executable's SHA-256 hash is not recognized. Version `2.3.3` is currently supported. |
| `compiler_options` | No | Shell-style argument string placed before the generated `-o <output> <input>` arguments. Quotes preserve values containing spaces. It defaults to an empty string. |
| `wibo_path` | No | Explicit path to `wibo`. This takes priority over the `WIBO_PATH` environment variable and searching `PATH`. |
| `default_gpr_helper_mask` | Yes | GPR helper-call mask used for translation units without an override. |
| `default_fpr_helper_mask` | Yes | FPR helper-call mask used for translation units without an override. |
| `translation_units` | Yes | List of per-translation-unit mask overrides. |
| `translation_units[].name` | Yes | Translation-unit name. |
| `translation_units[].gpr_helper_mask` | Yes | GPR mask to apply for the translation unit. |
| `translation_units[].fpr_helper_mask` | Yes | FPR mask to apply for the translation unit. |

Some of MWCC's terrible behaviours are outlined below, alongside the options included to workaround them.

## MWCC Behaviours

### Persistence of register usage masks

Under MWCC the `$a0–$a3` and `$f12–$f19` registers are caller-saved and any values live across calls must not be in one of these registers. Conversely, `$s0–$s7`/`$f20+` are callee-saved and survive. Register allocation is based on Chaitin graph colouring. To put it simply, the lowest number free register is always selected for a value.

Stored at `0x0051CE00` and `0x0051CE04` are respectively the compiler's helper function GPR and FPR masks. These masks specify which argument registers compiler-emitted helper calls read to ensure they are not re-used. These are builtin helper functions produced by MWCC including `fptosi` `litodp` `__moddi3` and `memcpy`. Standard function calls do not utilise these masks.

Bits in these masks are set based on an argument's index rather than the register itself. As such, 64-bit arguments occupy two registers and still set only one bit. The way it does this is as follows:
- Integer arguments: ``gpr |= 1 << (index + 3)``
- Float arguments: ``fpr |= 1 << (12 + 2 * (index - 1))``

**The bug**
MWCC never clears these masks again after first setting them, even across translation units. This means that registers are marked as "used by helper calls" if ANY previous function marked them as used. This ultimately means that certain values receive higher registers than they otherwise would in the absence of this bug.

This issue is particularly pertinent in games such as Dark Cloud, which compile the entire program in one invocation of MWCC. This causes these flags to accumulate across the entire game, altering register allocation behaviours when compared to compiling each translation unit individually.
