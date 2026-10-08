use crate::{
    compiler::{COMPILERS, Compiler},
    config::{self, ValueType},
};
use anyhow::{Context, Result, anyhow, bail, ensure};
use lldb::*;
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::Path,
    sync::{Mutex, OnceLock},
};

static COMPILER: OnceLock<&'static Compiler> = OnceLock::new();
static STATE: Mutex<Option<HookState>> = Mutex::new(None);

struct SourceConstant {
    function_object: u32,
    value_type: ValueType,
    bits: u64,
    type_record: u32,
}

struct ConvertedConstant {
    source: SourceConstant,
    child: u32,
    conversion_mode: u8,
    integer_mode: u8,
    integer_type: u32,
    integer_payload: Vec<u8>,
}

#[derive(Default)]
struct HookState {
    error: Option<String>,
    unit: String,
    units: HashSet<String>,
    function_names: HashMap<u32, String>,
    literal_objects: HashSet<u32>,
    annotation_return_ids: Vec<BreakpointID>,
    pending_expressions: HashMap<u32, (usize, bool)>,
    pending_conversions: HashMap<u32, SourceConstant>,
    converted_constants: HashMap<(u32, u32), ConvertedConstant>,
    expression_seen: HashSet<usize>,
    literal_seen: HashSet<usize>,
    constants: usize,
    argument_constants: usize,
    reloads: usize,
}

pub fn load_compiler(path: &Path) -> Result<()> {
    let bytes = fs::read(path).with_context(|| format!("read compiler {}", path.display()))?;
    let hash: [u8; 32] = Sha256::digest(&bytes).into();
    let compiler = COMPILERS.iter().find(|c| c.sha256 == hash).ok_or_else(|| {
        anyhow!(
            "unsupported compiler SHA-256 {}; version fallback cannot authorize memory hooks",
            hex::encode(hash)
        )
    })?;
    if let Some(version) = config::compiler_version() {
        ensure!(
            version == compiler.version,
            "compiler_version {version} disagrees with verified image {}",
            compiler.version
        );
    }
    COMPILER
        .set(compiler)
        .map_err(|_| anyhow!("compiler already initialized"))?;
    Ok(())
}

pub fn configure_hooks(target: &SBTarget, process: &SBProcess) -> Result<()> {
    let c = compiler()?;
    ensure!(
        c.alias_test.is_some()
            || (config::floating_point().literal_overrides.is_empty()
                && !config::floating_point().default_literal_reload_explicit),
        "compiler {} does not have a verified pooled-literal alias hook; literal_overrides are unsupported",
        c.version
    );
    for &(address, signature) in c.signatures {
        let bytes = read(process, address, signature.len())?;
        ensure!(
            bytes == signature,
            "compiler {} opcode signature differs at {address:#x}",
            c.version
        );
    }
    *STATE.lock().map_err(|_| anyhow!("hook state poisoned"))? = Some(HookState::default());
    for address in std::iter::once(c.translation_unit)
        .chain(std::iter::once(c.annotation))
        .chain(std::iter::once(c.argument_read))
        .chain(c.annotation_returns.iter().map(|x| x.0))
        .chain(c.literal_returns.iter().map(|x| x.0))
        .chain(c.alias_test)
    {
        let breakpoint = target.breakpoint_create_by_load_address(address);
        ensure!(
            breakpoint.num_resolved_locations() == 1,
            "unresolved hook at {address:#x}"
        );
        breakpoint.set_callback(callback);
        if c.annotation_returns.iter().any(|x| x.0 == address) {
            breakpoint.location_at_index(0).set_enabled(false);
            STATE
                .lock()
                .map_err(|_| anyhow!("hook state poisoned"))?
                .as_mut()
                .ok_or_else(|| anyhow!("hook state missing"))?
                .annotation_return_ids
                .push(breakpoint.id());
        }
    }
    Ok(())
}

fn compiler() -> Result<&'static Compiler> {
    COMPILER
        .get()
        .copied()
        .ok_or_else(|| anyhow!("compiler not initialized"))
}

// LLDB invokes this through C++. A Rust panic must never unwind through it.
fn callback(process: &SBProcess, thread: &SBThread, _: &SBBreakpointLocation) -> bool {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| apply(process, thread)));
    let error = match result {
        Ok(Ok(())) => return false,
        Ok(Err(error)) => format!("{error:#}"),
        Err(_) => "panic inside compiler hook".to_owned(),
    };
    if let Ok(mut state) = STATE.lock() {
        if let Some(state) = state.as_mut() {
            state.error = Some(error.clone());
        }
    }
    eprintln!("satansfiddle: {error}");
    true
}

fn read(process: &SBProcess, address: u64, size: usize) -> Result<Vec<u8>> {
    let mut bytes = vec![0; size];
    let count = process
        .read_memory(address, &mut bytes)
        .map_err(|e| anyhow!("read {address:#x}: {e:?}"))?;
    ensure!(
        count == size,
        "short memory read at {address:#x}: {count}/{size}"
    );
    Ok(bytes)
}

fn write(process: &SBProcess, address: u64, bytes: &[u8]) -> Result<()> {
    let count = process
        .write_memory(address, bytes)
        .map_err(|e| anyhow!("write {address:#x}: {e:?}"))?;
    ensure!(
        count == bytes.len(),
        "short memory write at {address:#x}: {count}/{}",
        bytes.len()
    );
    Ok(())
}

fn u32_at(process: &SBProcess, address: u64) -> Result<u32> {
    let bytes = read(process, address, 4)?;
    Ok(u32::from_le_bytes(
        bytes.try_into().map_err(|_| anyhow!("invalid word size"))?,
    ))
}

fn register(frame: &SBFrame, name: &str) -> Result<SBValue> {
    frame
        .find_value(name, lldb::ValueType::Register)
        .ok_or_else(|| anyhow!("missing guest register {name}"))
}

fn reg(frame: &SBFrame, name: &str) -> Result<u32> {
    Ok(register(frame, name)?
        .try_value_as_unsigned()
        .map_err(|e| anyhow!("read register {name}: {e:?}"))? as u32)
}

fn cstring(process: &SBProcess, address: u64) -> Result<String> {
    let mut bytes = Vec::new();
    for offset in 0..4096 {
        let byte = read(process, address + offset, 1)?[0];
        if byte == 0 {
            return String::from_utf8(bytes).context("non-UTF8 compiler identifier");
        }
        bytes.push(byte);
    }
    bail!("unterminated compiler identifier at {address:#x}")
}

fn function(process: &SBProcess, c: &Compiler) -> Result<String> {
    let object = u32_at(process, c.current_function)? as u64;
    if object == 0 {
        return Ok("?".to_owned());
    }
    let mut name = u32_at(process, object + c.object_link_name)? as u64;
    if name == 0 {
        name = u32_at(process, object + 8)? as u64;
    }
    if name == 0 {
        return Ok("?".to_owned());
    }
    cstring(process, name + 10)
}

fn call_target(
    process: &SBProcess,
    compiler: &Compiler,
    expression: u64,
) -> Result<Option<String>> {
    if expression == 0 || read(process, expression, 1)?[0] != 0x38 {
        return Ok(None);
    }
    let object = u32_at(process, expression + compiler.node_value)? as u64;
    if object == 0 {
        return Ok(None);
    }
    let mut name = u32_at(process, object + compiler.object_link_name)? as u64;
    if name == 0 {
        name = u32_at(process, object + 8)? as u64;
    }
    if name == 0 {
        return Ok(None);
    }
    Ok(Some(cstring(process, name + 10)?))
}

// Integer IEEE narrowing avoids host rounding mode and preserves NaN payload
// bits instead of invoking a floating conversion that can quiet a signaling NaN.
fn binary32_bits(bits: u64) -> u32 {
    let sign = ((bits >> 32) as u32) & 0x8000_0000;
    let exponent = ((bits >> 52) & 0x7ff) as i32;
    let fraction = bits & 0x000f_ffff_ffff_ffff;
    if exponent == 0x7ff {
        if fraction == 0 {
            return sign | 0x7f80_0000;
        }
        let payload = (fraction >> 29) as u32;
        return sign | 0x7f80_0000 | payload.max(1);
    }
    if exponent == 0 {
        return sign;
    }
    let mantissa = fraction | (1 << 52);
    let mut target_exponent = exponent - 1023 + 127;
    if target_exponent >= 255 {
        return sign | 0x7f80_0000;
    }
    if target_exponent <= 0 {
        return sign | round_even(mantissa, (30 - target_exponent) as u32) as u32;
    }
    let mut narrowed = round_even(mantissa, 29);
    if narrowed == 1 << 24 {
        narrowed >>= 1;
        target_exponent += 1;
    }
    if target_exponent >= 255 {
        return sign | 0x7f80_0000;
    }
    sign | ((target_exponent as u32) << 23) | (narrowed as u32 & 0x007f_ffff)
}

fn round_even(value: u64, shift: u32) -> u64 {
    if shift >= 64 {
        return 0;
    }
    let high = value >> shift;
    let low = value & ((1u64 << shift) - 1);
    let half = 1u64 << (shift - 1);
    high + u64::from(low > half || (low == half && high & 1 != 0))
}

fn constant(process: &SBProcess, type_record: u64, value: u64) -> Result<(ValueType, u64)> {
    let size = u32_at(process, type_record + 2)?;
    let bytes = read(process, value, 8)?;
    let bits = u64::from_le_bytes(
        bytes
            .try_into()
            .map_err(|_| anyhow!("invalid constant size"))?,
    );
    match size {
        4 => Ok((ValueType::Binary32, binary32_bits(bits) as u64)),
        8 => Ok((ValueType::Binary64, bits)),
        _ => bail!("unsupported floating constant type size {size}"),
    }
}

// These are compiler-internal forms verified at the argument flag consumer:
// 0x1e assigns a constant to an optimizer temporary; kind 4 dereferences a
// kind 0x38 reference. Only registered literal-pool objects identify constants,
// so the latter cannot accidentally classify a normal variable load.
fn argument_constant(
    process: &SBProcess,
    compiler: &Compiler,
    state: &HookState,
    function_object: u32,
    node: u64,
) -> Result<Option<(ValueType, u64)>> {
    let mut expression = node;
    let mut visited = HashSet::new();
    let outer_size = u32_at(process, u32_at(process, node + 10)? as u64 + 2)?;
    for _ in 0..8 {
        ensure!(expression != 0, "null constant-expression child");
        ensure!(
            visited.insert(expression),
            "cyclic constant-expression wrapper"
        );
        match read(process, expression, 1)?[0] {
            0x33 => {
                let type_record = u32_at(process, expression + 10)? as u64;
                // Assignment preserves the value type. Do not invent a policy
                // for conversions or incompatible compiler expression forms.
                if u32_at(process, type_record + 2)? != outer_size {
                    return Ok(None);
                }
                return constant(process, type_record, expression + compiler.node_value).map(Some);
            }
            0x1e => {
                expression = u32_at(process, expression + compiler.node_value + 4)? as u64;
            }
            0x30 => {
                // The compiler can lower an integral-valued float to a wide
                // integer followed by a conversion. Preserve its original IEEE
                // identity instead of guessing the private integer encoding.
                let Some(record) = state
                    .converted_constants
                    .get(&(function_object, expression as u32))
                else {
                    return Ok(None);
                };
                let child = u32_at(process, expression + compiler.node_value)?;
                if child != record.child
                    || u32_at(process, expression + 10)? != record.source.type_record
                    || read(process, expression + 1, 1)?[0] != record.conversion_mode
                    || read(process, child as u64, 1)?[0] != 0x32
                    || read(process, child as u64 + 1, 1)?[0] != record.integer_mode
                    || u32_at(process, child as u64 + 10)? != record.integer_type
                    || read(process, child as u64 + compiler.node_value, 16)?
                        != record.integer_payload
                    || u32_at(process, record.source.type_record as u64 + 2)? != outer_size
                {
                    return Ok(None);
                }
                return Ok(Some((record.source.value_type, record.source.bits)));
            }
            4 => {
                let reference = u32_at(process, expression + compiler.node_value)? as u64;
                if reference == 0 || read(process, reference, 1)?[0] != 0x38 {
                    return Ok(None);
                }
                let object = u32_at(process, reference + compiler.node_value)?;
                if !state.literal_objects.contains(&object) {
                    return Ok(None);
                }
                let type_record = u32_at(process, object as u64 + 12)? as u64;
                if u32_at(process, type_record + 2)? != outer_size {
                    return Ok(None);
                }
                return constant(
                    process,
                    type_record,
                    u32_at(process, object as u64 + compiler.object_info)? as u64,
                )
                .map(Some);
            }
            _ => return Ok(None),
        }
    }
    bail!("constant-expression wrapper depth exceeds verified limit")
}

fn set_annotation_returns(process: &SBProcess, state: &HookState, enabled: bool) -> Result<()> {
    for &id in &state.annotation_return_ids {
        let breakpoint = process
            .target()
            .find_breakpoint_by_id(id)
            .ok_or_else(|| anyhow!("missing annotation-return breakpoint {id}"))?;
        breakpoint.location_at_index(0).set_enabled(enabled);
    }
    Ok(())
}

fn apply(process: &SBProcess, thread: &SBThread) -> Result<()> {
    let c = compiler()?;
    let frame = thread.frame_at_index(0);
    let pc = frame.pc();
    let mut state = STATE.lock().map_err(|_| anyhow!("hook state poisoned"))?;
    let s = state
        .as_mut()
        .ok_or_else(|| anyhow!("hook state not initialized"))?;
    if pc == c.translation_unit {
        let size = read(process, c.tu_name, 1)?[0] as usize;
        let name = String::from_utf8(read(process, c.tu_name + 1, size)?)?;
        s.unit =
            config::translation_unit_name(config::translation_unit_override().unwrap_or(&name))
                .to_owned();
        s.units.insert(s.unit.clone());
        s.literal_objects.clear();
        s.converted_constants.clear();
        s.function_names.clear();
        write(
            process,
            c.helper_masks[0],
            &config::gpr_helper_mask(&s.unit).to_le_bytes(),
        )?;
        write(
            process,
            c.helper_masks[1],
            &config::fpr_helper_mask(&s.unit).to_le_bytes(),
        )?;
    } else if pc == c.annotation {
        let node = reg(&frame, c.annotation_node_register)? as u64;
        let rows = &config::floating_point().expression_overrides;
        let default_flag = config::floating_point().default_evaluate_first;
        write(process, node + 5, &[u8::from(default_flag)])?;
        s.constants += 1;
        if !rows.iter().any(|row| row.translation_unit == s.unit) {
            return Ok(());
        }
        let object = u32_at(process, c.current_function)?;
        let function = if let Some(name) = s.function_names.get(&object) {
            name.clone()
        } else {
            let name = function(process, c)?;
            s.function_names.insert(object, name.clone());
            name
        };
        if !rows
            .iter()
            .any(|row| row.translation_unit == s.unit && row.function == function)
        {
            return Ok(());
        }
        let (value_type, bits) = constant(
            process,
            u32_at(process, node + 10)? as u64,
            node + c.node_value,
        )?;
        if rows.iter().any(|row| {
            row.translation_unit == s.unit
                && row.function == function
                && row.value_type == value_type
                && row.value_bits == bits
        }) {
            s.converted_constants.remove(&(object, node as u32));
            s.pending_conversions.insert(
                node as u32,
                SourceConstant {
                    function_object: object,
                    value_type,
                    bits,
                    type_record: u32_at(process, node + 10)?,
                },
            );
            set_annotation_returns(process, s, true)?;
        }
        let mut flag = config::floating_point().default_evaluate_first;
        for (i, row) in config::floating_point()
            .expression_overrides
            .iter()
            .enumerate()
        {
            if row.translation_unit == s.unit
                && row.function == function
                && row.callee.is_none()
                && row.value_type == value_type
                && row.value_bits == bits
            {
                flag = row.evaluate_first;
                s.pending_expressions.insert(node as u32, (i, flag));
                set_annotation_returns(process, s, true)?;
            }
        }
        write(process, node + 5, &[u8::from(flag)])?;
    } else if pc == c.argument_read {
        let node = reg(&frame, c.argument_node_register)? as u64;
        // Fresh direct constants can carry arena residue. Wrappers carry
        // legitimate dependency flags, so retain their native default unless
        // an explicit stable constant selector matches.
        let direct = read(process, node, 1)?[0] == 0x33;
        let default = config::floating_point().default_evaluate_first;
        if direct {
            write(process, node + 5, &[u8::from(default)])?;
            s.argument_constants += 1;
        }
        let rows = &config::floating_point().expression_overrides;
        if !rows.iter().any(|row| row.translation_unit == s.unit) {
            return Ok(());
        }
        let object = u32_at(process, c.current_function)?;
        let function = if let Some(name) = s.function_names.get(&object) {
            name.clone()
        } else {
            let name = function(process, c)?;
            s.function_names.insert(object, name.clone());
            name
        };
        if !rows
            .iter()
            .any(|row| row.translation_unit == s.unit && row.function == function)
        {
            return Ok(());
        }
        let Some((value_type, bits)) = argument_constant(process, c, s, object, node)? else {
            return Ok(());
        };
        let wants_callee = rows.iter().any(|row| {
            row.translation_unit == s.unit
                && row.function == function
                && row.value_type == value_type
                && row.value_bits == bits
                && row.callee.is_some()
        });
        let callee = if wants_callee {
            call_target(
                process,
                c,
                u32_at(process, frame.sp() + c.call_callee_stack_offset)? as u64,
            )?
        } else {
            None
        };
        let mut flag = if direct {
            default
        } else {
            read(process, node + 5, 1)?[0] != 0
        };
        let mut selected = None;
        // Scoped decisions override the general value policy independently of
        // configuration order. Every matched selector identifies real input.
        for scoped in [false, true] {
            for (index, row) in rows.iter().enumerate() {
                if row.translation_unit == s.unit
                    && row.function == function
                    && row.value_type == value_type
                    && row.value_bits == bits
                    && row.callee.is_some() == scoped
                    && (row.callee.is_none() || row.callee.as_deref() == callee.as_deref())
                {
                    flag = row.evaluate_first;
                    selected = Some(index);
                    s.expression_seen.insert(index);
                }
            }
        }
        if selected.is_some() || direct {
            write(process, node + 5, &[u8::from(flag)])?;
        }
        if selected.is_some() && std::env::var_os("SATANSFIDDLE_VERIFY").is_some() {
            let actual = read(process, node + 5, 1)?[0];
            ensure!(
                actual == u8::from(flag),
                "argument constant memory verification failed"
            );
            eprintln!(
                "satansfiddle: call-expression {}/{} callee={} {:?} {:#x} evaluate_first={actual}",
                s.unit,
                function,
                callee.as_deref().unwrap_or("*"),
                value_type,
                bits
            );
        }
    } else if let Some(&(_, register_name)) = c.annotation_returns.iter().find(|x| x.0 == pc) {
        let node = reg(&frame, register_name)?;
        if let Some(source) = s.pending_conversions.remove(&node) {
            if read(process, node as u64, 1)?[0] == 0x30 {
                let child = u32_at(process, node as u64 + c.node_value)?;
                if child != 0 && read(process, child as u64, 1)?[0] == 0x32 {
                    let integer_payload = read(process, child as u64 + c.node_value, 16)?;
                    s.converted_constants.insert(
                        (source.function_object, node),
                        ConvertedConstant {
                            source,
                            child,
                            conversion_mode: read(process, node as u64 + 1, 1)?[0],
                            integer_mode: read(process, child as u64 + 1, 1)?[0],
                            integer_type: u32_at(process, child as u64 + 10)?,
                            integer_payload,
                        },
                    );
                }
            }
        }
        if let Some((index, flag)) = s.pending_expressions.remove(&node) {
            write(process, node as u64 + 5, &[u8::from(flag)])?;
            s.expression_seen.insert(index);
            if std::env::var_os("SATANSFIDDLE_VERIFY").is_some() {
                let row = config::floating_point()
                    .expression_overrides
                    .get(index)
                    .ok_or_else(|| anyhow!("missing expression override"))?;
                let actual = read(process, node as u64 + 5, 1)?[0];
                ensure!(
                    actual == u8::from(flag),
                    "expression override memory verification failed"
                );
                eprintln!(
                    "satansfiddle: expression {}/{} {:?} {:#x} evaluate_first={actual}",
                    row.translation_unit, row.function, row.value_type, row.value_bits
                );
            }
        }
        if s.pending_expressions.is_empty() && s.pending_conversions.is_empty() {
            set_annotation_returns(process, s, false)?;
        }
    } else if let Some(&(_, register_name)) = c.literal_returns.iter().find(|x| x.0 == pc) {
        s.literal_objects.insert(reg(&frame, register_name)?);
    } else if Some(pc) == c.alias_test {
        let object = u32_at(process, frame.sp() + 4)?;
        if !s.literal_objects.contains(&object) {
            return Ok(());
        }
        let object = object as u64;
        let (value_type, bits) = constant(
            process,
            u32_at(process, object + 12)? as u64,
            u32_at(process, object + c.object_info)? as u64,
        )?;
        let mut reload = config::floating_point().default_literal_reload;
        for (i, row) in config::floating_point()
            .literal_overrides
            .iter()
            .enumerate()
        {
            if row.translation_unit == s.unit
                && row.value_type == value_type
                && row.value_bits == bits
            {
                reload = row.reload;
                s.literal_seen.insert(i);
            }
        }
        let return_pc = u32_at(process, frame.sp())? as u64;
        register(&frame, "rax")?
            .set_value(&u8::from(reload).to_string())
            .map_err(|e| anyhow!("write return register: {e:?}"))?;
        register(&frame, "rsp")?
            .set_value(&(frame.sp() + 4).to_string())
            .map_err(|e| anyhow!("pop guest return address: {e:?}"))?;
        ensure!(
            frame.set_pc(return_pc),
            "cannot return from literal alias predicate"
        );
        s.reloads += 1;
    }
    Ok(())
}

pub fn check_error() -> Result<()> {
    let state = STATE.lock().map_err(|_| anyhow!("hook state poisoned"))?;
    if let Some(error) = state.as_ref().and_then(|s| s.error.as_ref()) {
        bail!("compiler hook failed: {error}");
    }
    Ok(())
}

pub fn check_complete() -> Result<()> {
    check_error()?;
    let state = STATE.lock().map_err(|_| anyhow!("hook state poisoned"))?;
    let s = state
        .as_ref()
        .ok_or_else(|| anyhow!("no compiler hooks executed"))?;
    ensure!(
        s.pending_expressions.is_empty() && s.pending_conversions.is_empty(),
        "expression annotation hook did not return"
    );
    ensure!(
        !s.units.is_empty(),
        "translation-unit hook was never reached"
    );
    for (i, row) in config::floating_point()
        .expression_overrides
        .iter()
        .enumerate()
    {
        if s.units.contains(&row.translation_unit) {
            ensure!(
                s.expression_seen.contains(&i),
                "unconsumed expression override in {}/{} bits {:#x}",
                row.translation_unit,
                row.function,
                row.value_bits
            );
        }
    }
    for (i, row) in config::floating_point()
        .literal_overrides
        .iter()
        .enumerate()
    {
        if s.units.contains(&row.translation_unit) {
            ensure!(
                s.literal_seen.contains(&i),
                "unconsumed literal override in {} bits {:#x}",
                row.translation_unit,
                row.value_bits
            );
        }
    }
    if std::env::var_os("SATANSFIDDLE_VERIFY").is_some() {
        eprintln!(
            "satansfiddle: {} units, {} float constants, {} argument constants, {} literal alias decisions",
            s.units.len(),
            s.constants,
            s.argument_constants,
            s.reloads
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::binary32_bits;

    #[test]
    fn normalized_binary32_preserves_special_values_and_payloads() {
        assert_eq!(binary32_bits(0), 0);
        assert_eq!(binary32_bits(0x8000_0000_0000_0000), 0x8000_0000);
        assert_eq!(binary32_bits(0x7ff0_0000_0000_0000), 0x7f80_0000);
        assert_eq!(binary32_bits(0xfff0_0000_0000_0000), 0xff80_0000);
        assert_eq!(binary32_bits(0x7ff0_0000_2000_0000), 0x7f80_0001);
        assert_eq!(binary32_bits(0xfff8_0020_0000_0000), 0xffc0_0100);
    }

    #[test]
    fn normalized_binary32_rounds_ties_and_subnormals() {
        // Halfway between 1 and the following float rounds to even 1.
        assert_eq!(binary32_bits(0x3ff0_0000_1000_0000), 0x3f80_0000);
        // Halfway after an odd low bit rounds upwards to the next even float.
        assert_eq!(binary32_bits(0x3ff0_0000_3000_0000), 0x3f80_0002);
        assert_eq!(binary32_bits(0x36a0_0000_0000_0000), 1);
        assert_eq!(binary32_bits(0x3690_0000_0000_0000), 0);
        assert_eq!(binary32_bits(0x3f84_7ae1_4000_0000), 0x3c23_d70a);
    }
}
