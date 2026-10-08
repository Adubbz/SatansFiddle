# Verified compiler profiles

Profiles identify compiler executables by SHA-256 and check every breakpoint's
instruction bytes after wibo maps the guest image. A version string cannot
substitute for a matching hash. Unknown images and short debugger memory
operations fail compilation.

| Build | SHA-256 |
| --- | --- |
| MWCC 2.3.3 / 000921 | `6375fd27814a1cb7eddbc229ecbd15b57d83562f95be23804f0a06ad4814e2df` |
| MWCC PS2 3.0 / 011126 | `0e16a5d6205101f840f85c02664f21cd63b39a0dec2dff417b3a61b4477f0e00` |

## Floating expression annotation

Both images place the expression kind at node offset zero, the evaluation-order
byte at `+5`, and the type pointer at `+0x0a`. Type size is the unaligned word at
`type+2`. Floating constants have kind `0x33`. Their payload is an eight-byte
binary64 value, including when the source type is binary32.

| Anchor | 2.3.3 | 3.0-011126 |
| --- | --- | --- |
| Annotation driver | `0x004b28c0` | `0x004b9500` |
| Floating case hook | `0x004b28f7`, node in EBP | `0x004b9532`, node in EBX |
| Constant payload offset | `+0x14` | `+0x18` |
| Simple-constant test | `0x004b21e0` | `0x004b8d00` |
| Fast-return hook | `0x004b290d` | `0x004b9548` |
| Ordinary-return hook | `0x004b330d` | `0x004ba11c` |
| Current function object | `0x00555ec0` | `0x0057b6bc` |
| Object link-name pointer offset | `+0x2c` | `+0x30` |

The fast-return path leaves `node+5` unwritten. Initialization at the floating
case repairs that read before its parent can derive an evaluation-order byte
from it. The hook reaches floating nodes only, rather than stopping for every
expression. Ordinary annotation retains the compiler's own writes, including
its decision to materialize complex literals through other node forms.

Explicit identity overrides are reapplied at the annotation's return, after
these ordinary writes. The original source type and bits are captured before
rewriting the node, so a pooled or converted literal retains its configured
identity. Return breakpoints are enabled only while a selected constant is
being annotated. Identical constants in a named function receive the same
policy. No ordinal, address, or arena residue identifies a configured constant.

## Constant arguments after optimization

Later optimizer passes can propagate a local's value into a fresh kind-`0x33`
argument node. Its address and evaluation-order byte differ from the initializer
that passed through annotation. The call-argument consumer is therefore another
required initialization point: `0x0049e925` in 2.3.3 and `0x004a4ae3` in 3.0,
with the argument node in ECX in both images. This hook initializes direct
floating constants and applies stable identity overrides at the actual read;
other expression kinds retain the compiler's established ordering decisions
unless an explicit constant selector applies.

Optimization can also introduce a kind-`0x1e` assignment to a temporary at the
first of two calls sharing a literal. Its RHS is at `node_value+4`. Constant
identity follows that RHS without treating the temporary's symbol as a selector.
A pooled constant is a kind-4 dereference of a kind-`0x38` reference to an object
observed at an actual pool-builder return. Only those tracked objects qualify;
normal variable loads do not. Explicit selectors apply to the wrapper's flag at
the call consumer, while its native dependency flag is preserved when no row
matches. Traversal has a finite depth limit and rejects cycles.

An integral-valued source float such as `257.0f` can be lowered to a kind-`0x30`
conversion of a kind-`0x32` private wide-integer literal. Selected source IEEE
identity is captured before annotation rewrites that node. At the verified
annotation return, the runtime records the rewritten child and its 16-byte
payload. The consumer accepts this identity only in the same function context
with the same root type, conversion mode, child type/mode and payload fingerprint.
The private integer
encoding is never interpreted as a host integer or floating value. Callee rows
still apply only at argument consumption; capturing identity does not mark a
selector consumed. Untracked conversions and arbitrary expressions do not
acquire a guessed constant identity.

The other flag reads in the 3.0 call-lowering routine inspect the saved callee
expression (`0x004a4b0d`, `0x004a525a`) or a special hidden argument
(`0x004a4b3b`, `0x004a51eb`). They are not additional reads of the real floating
arguments and do not require constant-argument breakpoints.

The callee expression is saved at `ESP+0x0c` in 2.3.3 and `ESP+8` in 3.0. A
direct call's callee expression has kind `0x38`; its value field points to the
callee object, whose link-name field identifies the mangled function. Optional
`callee` selectors distinguish identical float values passed to different named
functions. Scoped decisions take precedence over general identity decisions
independently of row order. A scoped selector is consumed only at a matching
call; it does not alter the same value elsewhere during annotation. Indirect
call targets without this direct symbol path do not match named-callee selectors.

Live 3.0 probes show propagated `6.0f` and `21.0f` arguments in
`CScene::GetNowVillagerTime` as direct floating-constant nodes. The raw `6.0f`
consumer byte held arena residue `0x4d`. Selecting `6.0f` at the consumer restores
retail's instruction order. `CrossFadeIn`, `CrossFadeOut`, and `CrossFade` each
receive direct `1.0f` nodes; the Out node's raw byte held residue `0x6d`. A
callee-scoped Out-only selector reproduces the required distinct decisions.

In 3.0, the simple-constant predicate immediately succeeds when the compiler's
optimization setting at `0x0057dbe6` is zero. Thus the unwritten-byte path affects
nontrivial bit patterns too under the game's `-O3,p` options. `-O2` reaches the
constant pool naturally; the profile does not change optimization settings.

## Pooled-literal alias queries

MWCC 2.3.3's alias predicate at `0x00432f50` accesses
`[object+0x24]+0x22` for objects of storage kind zero or one. The literal pool
builder at `0x004b2280` also creates storage-kind-zero objects, but `+0x24` points
to an eight-byte value buffer. The query consequently reads beyond that buffer.
Its result changes whether a literal load can be reused across an unknown store.

Pool-hit return `0x004b22b3` carries the literal object in EAX; new-object return
`0x004b2365` carries it in EBP. These hooks identify actual pool objects. The
alias hook applies the configured decision only to objects observed at those
returns; it does not infer literal identity from invented symbol names or
accidentally matching value bytes. Tracking resets at translation-unit boundaries,
and a subsequent pool-hit return registers a persistent literal again.

The 3.0 pool builder is `0x004b8da0`, with object returns at `0x004b8dd3` and
`0x004b8e8b`, both in EBP. Its value pointer is at object offset `+0x28`.
A corresponding unsafe alias predicate has not been established for this image.
Satan's Fiddle therefore preserves its normal alias behavior and rejects
explicit pooled-literal policy requests for 3.0. It does not apply the 2.3.3
predicate's addresses or assumptions to 3.0.

## Translation-unit state

| Anchor | 2.3.3 | 3.0-011126 |
| --- | --- | --- |
| Translation-unit entry | `0x00432040` | `0x004324f0` |
| Pascal basename length | `0x00557b28` | `0x0057daac` |
| GPR helper mask | `0x0051ce00` | `0x0053c990` |
| FPR helper mask | `0x0051ce04` | `0x0053c994` |

The configured masks are written at translation-unit entry. The compiler then
accumulates additional arguments naturally within the unit. The 3.0 masks are
ORed by the helper argument builder at `0x004a550d`, `0x004a5567`, and
`0x004a55e6`. Function naming uses the link-name pointer, falling back to the
plain-name pointer at object offset `+8`; the interned name text starts at `+10`.

`--translation-unit` supplies the logical basename when a build compiles a
temporary replacement source. The compiler still parses the actual input path.

## Validation

The real-compiler regression suite exercises both profiles and compares function
instructions independently of section and symbol metadata. Repeated compiles
check byte reproducibility. Mixed call arguments demonstrate evaluation-order
policy changes, and repeated equal constants demonstrate that one stable selector
covers every occurrence. Call-target probes on both compiler versions demonstrate
the direct-symbol path, and reversed general/scoped row orders produce identical
objects. Both `-O2` profiles also consume callee-scoped `0.01f` selectors
through verified pool references, and the 3.0 two-call fixture validates the
first-call temporary-assignment wrapper. Callee-scoped `257.0f` probes on both
`-O2` profiles validate the preserved-identity integer-lowering path. A normal
3.0 `monster.cpp` object has identical SHA-256 before and after that extension.
Canonical ChronicleTwo object comparisons validate the consumer hook
against all five identified world-function regressions. A store between two `0.01f` uses under 2.3.3 `-O2`
produces two `lwc1` instructions with conservative reload and one with reuse.
Wrong hashes, stale identities, unsupported literal policies, and compiler errors
must fail without publishing an object.
