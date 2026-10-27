//! A small, project-owned DEX integer execution engine.
//!
//! This is an early runtime component, not Android Runtime (ART) and not a
//! complete Android application environment. It executes only a bounded subset
//! of DEX instructions over 32-bit integer registers. Bounded static calls
//! between scalar/void methods are supported across loaded DEX files, with
//! bounded depth and a shared instruction budget. Objects, fields, arrays,
//! exceptions, Android framework APIs, native libraries, and graphics are still unsupported.
//!
//! The input is a parsed CodeItem. It must have no try/catch table and must
//! use only instructions listed in IntegerDexEngine's implementation.
//! Unsupported instructions are rejected before execution.

use crate::dex::{CodeItem, DexError, DexFile, EncodedMethod, MethodSignature};
use std::error::Error;
use std::fmt;

/// Resource limits for one method execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EngineLimits {
    /// Maximum number of 16-bit DEX code units accepted for one method.
    pub max_code_units: usize,
    /// Maximum number of instructions executed, including branches and returns.
    pub max_instruction_steps: u64,
}

impl Default for EngineLimits {
    fn default() -> Self {
        Self {
            max_code_units: 262_144,
            max_instruction_steps: 1_000_000,
        }
    }
}

/// The value returned from a supported DEX method.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineReturn {
    Void,
    Int(i32),
}

/// Outcome from one bounded method execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExecutionReport {
    pub return_value: EngineReturn,
    pub instruction_steps: u64,
}

/// Report for executing a method resolved from a parsed DEX file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MethodExecutionReport {
    pub signature: MethodSignature,
    pub execution: ExecutionReport,
}

/// Failure while finding and executing a method from real DEX metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MethodExecutionError {
    Dex(DexError),
    Engine(EngineError),
    ClassNotFound {
        class_descriptor: String,
    },
    MethodNotFound {
        class_descriptor: String,
        method_name: String,
    },
    MethodIsNotStatic {
        class_descriptor: String,
        method_name: String,
    },
    ArgumentCountMismatch {
        class_descriptor: String,
        method_name: String,
        actual: usize,
        expected: Vec<usize>,
    },
    UnsupportedSignature {
        signature: String,
    },
    MethodHasNoCode {
        signature: String,
    },
    AmbiguousMethod {
        signature: String,
    },
    ReturnTypeMismatch {
        signature: String,
        expected: String,
    },
}

impl fmt::Display for MethodExecutionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Dex(error) => write!(f, "DEX metadata error: {error}"),
            Self::Engine(error) => write!(f, "DEX execution error: {error}"),
            Self::ClassNotFound { class_descriptor } => {
                write!(
                    f,
                    "class {class_descriptor:?} was not found in this DEX file"
                )
            }
            Self::MethodNotFound {
                class_descriptor,
                method_name,
            } => write!(
                f,
                "method {class_descriptor}->{method_name} was not found in this DEX file"
            ),
            Self::MethodIsNotStatic {
                class_descriptor,
                method_name,
            } => write!(
                f,
                "method {class_descriptor}->{method_name} is not static; instance methods are not supported"
            ),
            Self::ArgumentCountMismatch {
                class_descriptor,
                method_name,
                actual,
                expected,
            } => write!(
                f,
                "method {class_descriptor}->{method_name} does not have a supported overload with {actual} arguments (available arities: {expected:?})"
            ),
            Self::UnsupportedSignature { signature } => write!(
                f,
                "method signature {signature} is unsupported; this engine accepts static int(int, ...) methods only"
            ),
            Self::MethodHasNoCode { signature } => {
                write!(f, "method {signature} has no DEX code item")
            }
            Self::AmbiguousMethod { signature } => {
                write!(f, "multiple methods match {signature}")
            }
            Self::ReturnTypeMismatch {
                signature,
                expected,
            } => write!(
                f,
                "method {signature} did not return the declared scalar value ({expected})"
            ),
        }
    }
}

impl Error for MethodExecutionError {}

impl From<DexError> for MethodExecutionError {
    fn from(value: DexError) -> Self {
        Self::Dex(value)
    }
}

impl From<EngineError> for MethodExecutionError {
    fn from(value: EngineError) -> Self {
        Self::Engine(value)
    }
}

/// DEX scalar values represented in a single 32-bit register by this engine.
fn is_int_like_type(ty: &str) -> bool {
    matches!(ty, "I" | "Z" | "B" | "S" | "C")
}

/// Normalize a DEX scalar result or argument to its declared type width.
fn normalize_int_like(ty: &str, value: i32) -> i32 {
    match ty {
        "Z" => i32::from(value != 0),
        "B" => i32::from(value as i8),
        "S" => i32::from(value as i16),
        "C" => i32::from(value as u16),
        _ => value,
    }
}

/// List static scalar methods supported by the current execution API.
///
/// Integer, boolean, byte, short, and char values are represented in one
/// 32-bit register. This checks metadata only; method bodies may still contain
/// unsupported instructions. Results follow DEX class/method order and can be
/// filtered to one class descriptor.
pub fn list_static_scalar_methods(
    dex: &DexFile<'_>,
    class_filter: Option<&str>,
) -> Result<Vec<MethodSignature>, MethodExecutionError> {
    let mut found = Vec::new();
    for class in dex.class_defs() {
        let class_descriptor = dex.type_descriptor(class.class_idx)?;
        if class_filter.is_some_and(|filter| filter != class_descriptor.as_str()) {
            continue;
        }
        let Some(data) = dex.class_data(class)? else {
            continue;
        };
        for method in data
            .direct_methods
            .iter()
            .chain(data.virtual_methods.iter())
        {
            if method.access_flags & 0x8 == 0 || method.code_off == 0 {
                continue;
            }
            let signature = dex.method_signature(method.method_idx)?;
            if is_int_like_type(&signature.return_type)
                && signature
                    .parameter_types
                    .iter()
                    .all(|ty| is_int_like_type(ty))
            {
                found.push(signature);
            }
        }
    }
    Ok(found)
}

/// Backwards-compatible name for listing the current integer-like scalar method set.
///
/// This includes `int`, `boolean`, `byte`, `short`, and `char` signatures.
pub fn list_static_integer_methods(
    dex: &DexFile<'_>,
    class_filter: Option<&str>,
) -> Result<Vec<MethodSignature>, MethodExecutionError> {
    list_static_scalar_methods(dex, class_filter)
}

/// Resolve and execute a static scalar method from a parsed DEX file.
///
/// This single-DEX convenience API rejects calls whose definitions are not in
/// the supplied DEX. Use execute_static_scalar_method_in_dex_set when the APK
/// contains multiple DEX files.
pub fn execute_static_scalar_method(
    dex: &DexFile<'_>,
    class_descriptor: &str,
    method_name: &str,
    parameters: &[i32],
) -> Result<MethodExecutionReport, MethodExecutionError> {
    execute_static_scalar_method_in_dex_set(
        std::slice::from_ref(dex),
        class_descriptor,
        method_name,
        parameters,
    )
}

/// Resolve and execute a scalar method with method definitions available across
/// a set of DEX files from the same APK.
///
/// Method indices are resolved in the caller's DEX first, then the resulting
/// full signature is matched against actual method definitions in the loaded
/// DEX set. This permits calls between classes.dex, classes2.dex, and later
/// files without pretending that method indices are shared between DEX files.
/// Scalar int, boolean, byte, short, and char values occupy one 32-bit register
/// and are normalized at method boundaries.
pub fn execute_static_scalar_method_in_dex_set(
    dexes: &[DexFile<'_>],
    class_descriptor: &str,
    method_name: &str,
    parameters: &[i32],
) -> Result<MethodExecutionReport, MethodExecutionError> {
    let mut root_dex_index = None;
    for (dex_index, dex) in dexes.iter().enumerate() {
        for class in dex.class_defs() {
            if dex.type_descriptor(class.class_idx)? == class_descriptor {
                root_dex_index = Some(dex_index);
                break;
            }
        }
        if root_dex_index.is_some() {
            break;
        }
    }
    let root_dex_index = root_dex_index.ok_or_else(|| MethodExecutionError::ClassNotFound {
        class_descriptor: class_descriptor.to_owned(),
    })?;
    execute_static_scalar_method_in_dex_set_at(
        dexes,
        root_dex_index,
        class_descriptor,
        method_name,
        parameters,
    )
}

/// Execute a scalar method starting from a specific DEX entry in a loaded set.
pub fn execute_static_scalar_method_in_dex_set_at(
    dexes: &[DexFile<'_>],
    root_dex_index: usize,
    class_descriptor: &str,
    method_name: &str,
    parameters: &[i32],
) -> Result<MethodExecutionReport, MethodExecutionError> {
    let dex_index = root_dex_index;
    let dex = dexes
        .get(dex_index)
        .ok_or_else(|| MethodExecutionError::ClassNotFound {
            class_descriptor: class_descriptor.to_owned(),
        })?;
    let mut selected_class = None;
    for class in dex.class_defs() {
        if dex.type_descriptor(class.class_idx)? == class_descriptor {
            selected_class = Some(class);
            break;
        }
    }
    let class = selected_class.ok_or_else(|| MethodExecutionError::ClassNotFound {
        class_descriptor: class_descriptor.to_owned(),
    })?;
    let dex = &dexes[dex_index];
    let Some(data) = dex.class_data(class)? else {
        return Err(MethodExecutionError::MethodNotFound {
            class_descriptor: class_descriptor.to_owned(),
            method_name: method_name.to_owned(),
        });
    };

    let mut same_name = Vec::new();
    for method in data
        .direct_methods
        .iter()
        .chain(data.virtual_methods.iter())
    {
        let signature = dex.method_signature(method.method_idx)?;
        if signature.name == method_name {
            same_name.push((dex_index, *method, signature));
        }
    }
    if same_name.is_empty() {
        return Err(MethodExecutionError::MethodNotFound {
            class_descriptor: class_descriptor.to_owned(),
            method_name: method_name.to_owned(),
        });
    }

    let static_methods: Vec<_> = same_name
        .into_iter()
        .filter(|(_, method, _)| method.access_flags & 0x8 != 0)
        .collect();
    if static_methods.is_empty() {
        return Err(MethodExecutionError::MethodIsNotStatic {
            class_descriptor: class_descriptor.to_owned(),
            method_name: method_name.to_owned(),
        });
    }

    let mut expected: Vec<usize> = static_methods
        .iter()
        .map(|(_, _, signature)| signature.parameter_types.len())
        .collect();
    expected.sort_unstable();
    expected.dedup();

    let matching_arity: Vec<_> = static_methods
        .into_iter()
        .filter(|(_, _, signature)| signature.parameter_types.len() == parameters.len())
        .collect();
    if matching_arity.is_empty() {
        return Err(MethodExecutionError::ArgumentCountMismatch {
            class_descriptor: class_descriptor.to_owned(),
            method_name: method_name.to_owned(),
            actual: parameters.len(),
            expected,
        });
    }

    let supported: Vec<_> = matching_arity
        .into_iter()
        .filter(|(_, _, signature)| {
            is_int_like_type(&signature.return_type)
                && signature
                    .parameter_types
                    .iter()
                    .all(|ty| is_int_like_type(ty))
        })
        .collect();
    if supported.is_empty() {
        let signature = same_name_signature_label(class_descriptor, method_name, parameters.len());
        return Err(MethodExecutionError::UnsupportedSignature { signature });
    }
    if supported.len() > 1 {
        return Err(MethodExecutionError::AmbiguousMethod {
            signature: same_name_signature_label(class_descriptor, method_name, parameters.len()),
        });
    }

    let (dex_index, method, signature) = supported.into_iter().next().expect("one supported match");
    if method.code_off == 0 {
        return Err(MethodExecutionError::MethodHasNoCode {
            signature: method_signature_label(&signature),
        });
    }
    let code = dexes[dex_index].code_item(method.code_off)?;
    let engine = IntegerDexEngine::default();
    let mut budget = ExecutionBudget {
        max_steps: engine.limits.max_instruction_steps,
        steps: 0,
    };
    let normalized_parameters: Vec<i32> = signature
        .parameter_types
        .iter()
        .zip(parameters.iter().copied())
        .map(|(ty, value)| normalize_int_like(ty, value))
        .collect();
    let mut resolver = |target_method_idx: u32, arguments: &[i32], budget: &mut ExecutionBudget| {
        execute_dex_method_index(
            dexes,
            dex_index,
            target_method_idx,
            arguments,
            &engine,
            budget,
            1,
        )
    };
    let mut execution = engine
        .execute_with_budget(
            &code,
            &normalized_parameters,
            &mut budget,
            Some(&mut resolver),
        )
        .map_err(|error| match error {
            EngineError::StaticCallFailed { method_idx, reason } => static_call_failed(
                method_idx,
                format!("while executing {}: {reason}", method_signature_label(&signature)),
            ),
            other => other,
        })?;
    match execution.return_value {
        EngineReturn::Int(value) => {
            execution.return_value =
                EngineReturn::Int(normalize_int_like(&signature.return_type, value));
        }
        EngineReturn::Void => {
            return Err(MethodExecutionError::ReturnTypeMismatch {
                signature: method_signature_label(&signature),
                expected: signature.return_type.clone(),
            });
        }
    }
    Ok(MethodExecutionReport {
        signature,
        execution,
    })
}

/// Backwards-compatible API name; the supported signature set includes scalar
/// boolean, byte, short, and char values in addition to 32-bit integers.
pub fn execute_static_integer_method(
    dex: &DexFile<'_>,
    class_descriptor: &str,
    method_name: &str,
    parameters: &[i32],
) -> Result<MethodExecutionReport, MethodExecutionError> {
    execute_static_scalar_method(dex, class_descriptor, method_name, parameters)
}

fn execute_dex_method_index(
    dexes: &[DexFile<'_>],
    caller_dex_index: usize,
    method_idx: u32,
    parameters: &[i32],
    engine: &IntegerDexEngine,
    budget: &mut ExecutionBudget,
    depth: usize,
) -> Result<EngineReturn, EngineError> {
    if depth > MAX_METHOD_CALL_DEPTH {
        return Err(EngineError::MethodCallDepthLimitExceeded {
            max_depth: MAX_METHOD_CALL_DEPTH,
        });
    }

    let (dex_index, method, signature) =
        match find_encoded_method(dexes, caller_dex_index, method_idx) {
            Ok(resolved) => resolved,
            Err(error) => {
                // A small, explicit set of framework intrinsics can be modeled
                // without pretending that the Android framework itself is loaded.
                let caller_dex = dexes.get(caller_dex_index).ok_or_else(|| {
                    static_call_failed(method_idx, "caller DEX index is outside the loaded DEX set")
                })?;
                let signature = caller_dex
                    .method_signature(method_idx)
                    .map_err(|metadata_error| static_call_failed(method_idx, metadata_error))?;
                if let Some(value) = api30_window_insets_type_intrinsic(&signature, parameters) {
                    return Ok(EngineReturn::Int(value));
                }
                return Err(error);
            }
        };
    if method.access_flags & 0x8 == 0 {
        return Err(static_call_failed(
            method_idx,
            "only static methods are supported",
        ));
    }
    if method.code_off == 0 {
        return Err(static_call_failed(
            method_idx,
            "method has no code item (abstract/native/external methods are unsupported)",
        ));
    }
    if !signature
        .parameter_types
        .iter()
        .all(|ty| is_int_like_type(ty))
    {
        return Err(static_call_failed(
            method_idx,
            "only int, boolean, byte, short, and char parameters are supported",
        ));
    }
    if signature.return_type != "V" && !is_int_like_type(&signature.return_type) {
        return Err(static_call_failed(
            method_idx,
            "only int, boolean, byte, short, char, and void returns are supported",
        ));
    }
    if parameters.len() != signature.parameter_types.len() {
        let method_label = method_signature_label(&signature);
        return Err(static_call_failed(
            method_idx,
            format!(
                "call to {method_label} supplied {} arguments but the signature declares {} parameters",
                parameters.len(),
                signature.parameter_types.len()
            ),
        ));
    }

    let code = dexes[dex_index]
        .code_item(method.code_off)
        .map_err(|error| static_call_failed(method_idx, error))?;
    let normalized_parameters: Vec<i32> = signature
        .parameter_types
        .iter()
        .zip(parameters.iter().copied())
        .map(|(ty, value)| normalize_int_like(ty, value))
        .collect();
    let mut resolver = |child_method_idx: u32, arguments: &[i32], budget: &mut ExecutionBudget| {
        execute_dex_method_index(
            dexes,
            dex_index,
            child_method_idx,
            arguments,
            engine,
            budget,
            depth + 1,
        )
    };
    let report = engine
        .execute_with_budget(&code, &normalized_parameters, budget, Some(&mut resolver))
        .map_err(|error| match error {
            EngineError::StaticCallFailed { method_idx, reason } => static_call_failed(
                method_idx,
                format!("while executing {}: {reason}", method_signature_label(&signature)),
            ),
            other => other,
        })?;

    match (signature.return_type.as_str(), report.return_value) {
        ("I" | "Z" | "B" | "S" | "C", EngineReturn::Int(value)) => Ok(EngineReturn::Int(
            normalize_int_like(&signature.return_type, value),
        )),
        ("V", EngineReturn::Void) => Ok(EngineReturn::Void),
        ("I" | "Z" | "B" | "S" | "C", EngineReturn::Void) => Err(static_call_failed(
            method_idx,
            "method has scalar return type but executed return-void",
        )),
        ("V", EngineReturn::Int(_)) => Err(static_call_failed(
            method_idx,
            "method is declared void but executed an integer return",
        )),
        _ => unreachable!("only scalar and void signatures passed validation"),
    }
}

/// Return deterministic values for a narrow set of Android API 30+
/// WindowInsets.Type intrinsics. These methods return stable bit masks; this
/// is not a general Android framework emulator, and environment-dependent APIs
/// such as SdkExtensions.getExtensionVersion deliberately remain unsupported.
fn api30_window_insets_type_intrinsic(
    signature: &MethodSignature,
    parameters: &[i32],
) -> Option<i32> {
    if signature.class_descriptor != "Landroid/view/WindowInsets$Type;"
        || signature.return_type != "I"
        || !signature.parameter_types.is_empty()
        || !parameters.is_empty()
    {
        return None;
    }

    match signature.name.as_str() {
        "statusBars" => Some(1 << 0),
        "navigationBars" => Some(1 << 1),
        "captionBar" => Some(1 << 2),
        "ime" => Some(1 << 3),
        "systemGestures" => Some(1 << 4),
        "mandatorySystemGestures" => Some(1 << 5),
        "tappableElement" => Some(1 << 6),
        "displayCutout" => Some(1 << 7),
        _ => None,
    }
}

fn find_encoded_method(
    dexes: &[DexFile<'_>],
    caller_dex_index: usize,
    method_idx: u32,
) -> Result<(usize, EncodedMethod, MethodSignature), EngineError> {
    let caller_dex = dexes.get(caller_dex_index).ok_or_else(|| {
        static_call_failed(method_idx, "caller DEX index is outside the loaded DEX set")
    })?;
    let signature = caller_dex
        .method_signature(method_idx)
        .map_err(|error| static_call_failed(method_idx, error))?;

    // Prefer the caller's own DEX, then search the remaining APK DEX files.
    let mut declaration_without_code = false;
    let candidates = std::iter::once(caller_dex_index)
        .chain((0..dexes.len()).filter(|candidate| *candidate != caller_dex_index));
    for dex_index in candidates {
        let dex = &dexes[dex_index];
        for class in dex.class_defs() {
            let descriptor = dex
                .type_descriptor(class.class_idx)
                .map_err(|error| static_call_failed(method_idx, error))?;
            if descriptor != signature.class_descriptor {
                continue;
            }
            let data = dex
                .class_data(class)
                .map_err(|error| static_call_failed(method_idx, error))?;
            let Some(data) = data else {
                continue;
            };
            for method in data
                .direct_methods
                .iter()
                .chain(data.virtual_methods.iter())
            {
                let candidate_signature = dex
                    .method_signature(method.method_idx)
                    .map_err(|error| static_call_failed(method_idx, error))?;
                if candidate_signature == signature {
                    if method.code_off != 0 {
                        return Ok((dex_index, *method, signature));
                    }
                    declaration_without_code = true;
                }
            }
        }
    }
    let method_label = method_signature_label(&signature);
    let reason = if declaration_without_code {
        format!(
            "method {method_label} is declared in loaded DEX metadata but has no code item (native/abstract/external methods are unsupported)"
        )
    } else {
        format!(
            "definition for {method_label} was not found in any loaded DEX file (external/framework code or missing DEX dependency)"
        )
    };
    Err(static_call_failed(method_idx, reason))
}

fn static_call_failed(method_idx: u32, error: impl fmt::Display) -> EngineError {
    EngineError::StaticCallFailed {
        method_idx,
        reason: error.to_string(),
    }
}

fn same_name_signature_label(class_descriptor: &str, method_name: &str, arity: usize) -> String {
    format!("{class_descriptor}->{method_name}/({arity} arguments)")
}

fn method_signature_label(signature: &MethodSignature) -> String {
    format!(
        "{}->{}({}){}",
        signature.class_descriptor,
        signature.name,
        signature.parameter_types.join(","),
        signature.return_type
    )
}

/// A refusal or failure from the project-owned integer DEX engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EngineError {
    CodeTooLarge {
        size: usize,
        max: usize,
    },
    InstructionStepLimitExceeded {
        max: u64,
    },
    TryCatchUnsupported,
    InvalidRegisterLayout {
        registers_size: u16,
        ins_size: u16,
    },
    ParameterCountMismatch {
        expected: usize,
        actual: usize,
    },
    TruncatedInstruction {
        pc: usize,
        width: usize,
        remaining: usize,
    },
    UnsupportedOpcode {
        pc: usize,
        opcode: u8,
    },
    InvalidInstructionEncoding {
        pc: usize,
        word: u16,
    },
    InvalidRegister {
        pc: usize,
        register: usize,
        register_count: usize,
    },
    InvalidBranchTarget {
        pc: usize,
        target: i64,
    },
    BranchToNonInstruction {
        pc: usize,
        target: usize,
    },
    DivisionByZero {
        pc: usize,
    },
    MissingReturn {
        pc: usize,
    },
    MoveResultWithoutIntegerResult {
        pc: usize,
    },
    OutgoingArgumentCountExceeded {
        pc: usize,
        count: usize,
        outs_size: u16,
    },
    StaticCallFailed {
        method_idx: u32,
        reason: String,
    },
    MethodCallDepthLimitExceeded {
        max_depth: usize,
    },
}

impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CodeTooLarge { size, max } => {
                write!(f, "DEX method has {size} code units; the limit is {max}")
            }
            Self::InstructionStepLimitExceeded { max } => {
                write!(f, "DEX method exceeded the {max}-instruction step limit")
            }
            Self::TryCatchUnsupported => {
                f.write_str("DEX methods with try/catch tables are not supported")
            }
            Self::InvalidRegisterLayout {
                registers_size,
                ins_size,
            } => write!(
                f,
                "invalid DEX register layout: {ins_size} input registers exceed {registers_size} total registers"
            ),
            Self::ParameterCountMismatch { expected, actual } => write!(
                f,
                "DEX method expects {expected} 32-bit integer parameters, received {actual}"
            ),
            Self::TruncatedInstruction {
                pc,
                width,
                remaining,
            } => write!(
                f,
                "truncated DEX instruction at code unit {pc}: needs {width} units, {remaining} remain"
            ),
            Self::UnsupportedOpcode { pc, opcode } => {
                write!(f, "unsupported DEX opcode 0x{opcode:02x} at code unit {pc}")
            }
            Self::InvalidInstructionEncoding { pc, word } => write!(
                f,
                "invalid DEX instruction encoding 0x{word:04x} at code unit {pc}"
            ),
            Self::InvalidRegister {
                pc,
                register,
                register_count,
            } => write!(
                f,
                "DEX instruction at code unit {pc} accesses register v{register}, but only {register_count} registers exist"
            ),
            Self::InvalidBranchTarget { pc, target } => write!(
                f,
                "DEX branch at code unit {pc} targets invalid code unit {target}"
            ),
            Self::BranchToNonInstruction { pc, target } => write!(
                f,
                "DEX branch at code unit {pc} targets code unit {target}, which is not an instruction boundary"
            ),
            Self::DivisionByZero { pc } => {
                write!(f, "integer division by zero at code unit {pc}")
            }
            Self::MissingReturn { pc } => {
                write!(
                    f,
                    "DEX method reached code unit {pc} without a return instruction"
                )
            }
            Self::MoveResultWithoutIntegerResult { pc } => {
                write!(
                    f,
                    "move-result at code unit {pc} has no pending integer result"
                )
            }
            Self::OutgoingArgumentCountExceeded {
                pc,
                count,
                outs_size,
            } => write!(
                f,
                "invoke-static at code unit {pc} uses {count} argument words but the method declares outs_size={outs_size}"
            ),
            Self::StaticCallFailed { method_idx, reason } => {
                write!(
                    f,
                    "invoke-static method index {method_idx} failed: {reason}"
                )
            }
            Self::MethodCallDepthLimitExceeded { max_depth } => {
                write!(f, "DEX method call depth exceeded the limit of {max_depth}")
            }
        }
    }
}

impl Error for EngineError {}

/// Project-owned, bounded interpreter for a small integer-only DEX subset.
///
/// Supported instructions:
/// - nop, move, move/from16, move/16;
/// - const/4, const/16, const, const/high16;
/// - return-void, return;
/// - goto, goto/16, goto/32;
/// - if-eq, if-ne, if-lt, if-ge, if-gt, if-le and their *z forms;
/// - 32-bit integer arithmetic/bitwise operations, /2addr and literal forms;
/// - integer negation, bitwise not, and byte/char/short narrowing.
///
/// Method parameters must already be represented as 32-bit integers. They are
/// copied into the final ins_size registers, as required for a DEX method
/// frame. This is useful for developing and testing the engine independently
/// from an external Android runtime, but does not make ordinary APKs runnable.
#[derive(Debug, Clone, Copy, Default)]
pub struct IntegerDexEngine {
    limits: EngineLimits,
}

impl IntegerDexEngine {
    pub fn new(limits: EngineLimits) -> Self {
        Self { limits }
    }

    pub fn limits(&self) -> EngineLimits {
        self.limits
    }

    /// Execute a parsed DEX code item with 32-bit integer parameters.
    ///
    /// Instructions are decoded and branch targets are checked before any
    /// register code runs. The step limit bounds loops. A method that uses an
    /// unsupported instruction is rejected rather than partially interpreted.
    pub fn execute(
        &self,
        code: &CodeItem,
        parameters: &[i32],
    ) -> Result<ExecutionReport, EngineError> {
        let mut budget = ExecutionBudget {
            max_steps: self.limits.max_instruction_steps,
            steps: 0,
        };
        self.execute_with_budget(code, parameters, &mut budget, None)
    }

    fn execute_with_budget(
        &self,
        code: &CodeItem,
        parameters: &[i32],
        budget: &mut ExecutionBudget,
        mut resolver: Option<&mut StaticCallResolver<'_>>,
    ) -> Result<ExecutionReport, EngineError> {
        if code.insns.len() > self.limits.max_code_units {
            return Err(EngineError::CodeTooLarge {
                size: code.insns.len(),
                max: self.limits.max_code_units,
            });
        }
        if code.tries_size != 0 {
            return Err(EngineError::TryCatchUnsupported);
        }
        if code.ins_size > code.registers_size {
            return Err(EngineError::InvalidRegisterLayout {
                registers_size: code.registers_size,
                ins_size: code.ins_size,
            });
        }
        let expected_parameters = usize::from(code.ins_size);
        if parameters.len() != expected_parameters {
            return Err(EngineError::ParameterCountMismatch {
                expected: expected_parameters,
                actual: parameters.len(),
            });
        }

        let program = decode_program(&code.insns, resolver.is_some())?;
        let mut registers = vec![0_i32; usize::from(code.registers_size)];
        let parameter_start = registers.len() - expected_parameters;
        registers[parameter_start..].copy_from_slice(parameters);

        let start_steps = budget.steps;
        let mut pc = 0usize;
        let mut pending_result: Option<EngineReturn> = None;
        while pc < program.len() {
            if budget.steps >= budget.max_steps {
                return Err(EngineError::InstructionStepLimitExceeded {
                    max: budget.max_steps,
                });
            }
            budget.steps += 1;

            let instruction = program[pc].ok_or(EngineError::MissingReturn { pc })?;
            let word = code.insns[pc];
            let mut next_pc = pc + instruction.width;
            let current_result = if instruction.opcode == 0x0a {
                pending_result.take()
            } else {
                pending_result = None;
                None
            };

            match instruction.opcode {
                0x00 => {}
                0x01 => {
                    let dest = usize::from((word >> 8) & 0x0f);
                    let src = usize::from((word >> 12) & 0x0f);
                    let value = read_register(&registers, src, pc)?;
                    write_register(&mut registers, dest, value, pc)?;
                }
                0x02 => {
                    let dest = usize::from(word >> 8);
                    let src = usize::from(code.insns[pc + 1]);
                    let value = read_register(&registers, src, pc)?;
                    write_register(&mut registers, dest, value, pc)?;
                }
                0x03 => {
                    let dest = usize::from(code.insns[pc + 1]);
                    let src = usize::from(code.insns[pc + 2]);
                    let value = read_register(&registers, src, pc)?;
                    write_register(&mut registers, dest, value, pc)?;
                }
                0x0a => {
                    let value = match current_result {
                        Some(EngineReturn::Int(value)) => value,
                        _ => return Err(EngineError::MoveResultWithoutIntegerResult { pc }),
                    };
                    write_register(&mut registers, usize::from(word >> 8), value, pc)?;
                }
                0x0e => {
                    return Ok(ExecutionReport {
                        return_value: EngineReturn::Void,
                        instruction_steps: budget.steps - start_steps,
                    });
                }
                0x0f => {
                    let src = usize::from(word >> 8);
                    return Ok(ExecutionReport {
                        return_value: EngineReturn::Int(read_register(&registers, src, pc)?),
                        instruction_steps: budget.steps - start_steps,
                    });
                }
                0x12 => {
                    let dest = usize::from((word >> 8) & 0x0f);
                    let literal = i32::from((word >> 12) & 0x0f);
                    let value = if literal & 0x08 != 0 {
                        literal - 16
                    } else {
                        literal
                    };
                    write_register(&mut registers, dest, value, pc)?;
                }
                0x13 => {
                    let dest = usize::from(word >> 8);
                    let value = i32::from(code.insns[pc + 1] as i16);
                    write_register(&mut registers, dest, value, pc)?;
                }
                0x14 => {
                    let dest = usize::from(word >> 8);
                    let value = (u32::from(code.insns[pc + 1])
                        | (u32::from(code.insns[pc + 2]) << 16))
                        as i32;
                    write_register(&mut registers, dest, value, pc)?;
                }
                0x15 => {
                    let dest = usize::from(word >> 8);
                    let value = (u32::from(code.insns[pc + 1]) << 16) as i32;
                    write_register(&mut registers, dest, value, pc)?;
                }
                0x7b..=0x7c | 0x8d..=0x8f => {
                    let dest = usize::from((word >> 8) & 0x0f);
                    let src = usize::from(word >> 12);
                    let source = read_register(&registers, src, pc)?;
                    let value = match instruction.opcode {
                        0x7b => source.wrapping_neg(),
                        0x7c => !source,
                        0x8d => i32::from(source as i8),
                        0x8e => i32::from(source as u16),
                        0x8f => i32::from(source as i16),
                        _ => unreachable!("only implemented unary integer opcodes are dispatched"),
                    };
                    write_register(&mut registers, dest, value, pc)?;
                }
                0xd0..=0xd7 => {
                    // 22s: vA/vB are four-bit registers, followed by a signed 16-bit literal.
                    let dest = usize::from((word >> 8) & 0x0f);
                    let src = usize::from(word >> 12);
                    let left = read_register(&registers, src, pc)?;
                    let literal = i32::from(code.insns[pc + 1] as i16);
                    let value = arithmetic_literal(instruction.opcode, left, literal, pc)?;
                    write_register(&mut registers, dest, value, pc)?;
                }
                0xd8..=0xe2 => {
                    // 22b: vA in the first unit; vB and signed 8-bit literal in the second.
                    let dest = usize::from(word >> 8);
                    let operands = code.insns[pc + 1];
                    let src = usize::from(operands & 0x00ff);
                    let literal = i32::from((operands >> 8) as u8 as i8);
                    let left = read_register(&registers, src, pc)?;
                    let value = arithmetic_literal(instruction.opcode, left, literal, pc)?;
                    write_register(&mut registers, dest, value, pc)?;
                }
                0x28..=0x2a => {
                    next_pc = instruction.branch_target.expect("goto target is decoded");
                }
                0x32..=0x37 => {
                    let left_reg = usize::from((word >> 8) & 0x0f);
                    let right_reg = usize::from((word >> 12) & 0x0f);
                    let left = read_register(&registers, left_reg, pc)?;
                    let right = read_register(&registers, right_reg, pc)?;
                    if compare(instruction.opcode, left, right) {
                        next_pc = instruction.branch_target.expect("if target is decoded");
                    }
                }
                0x38..=0x3d => {
                    let register = usize::from(word >> 8);
                    let value = read_register(&registers, register, pc)?;
                    if compare_zero(instruction.opcode, value) {
                        next_pc = instruction.branch_target.expect("if target is decoded");
                    }
                }
                0x90..=0x9a => {
                    let dest = usize::from(word >> 8);
                    let operands = code.insns[pc + 1];
                    let left_reg = usize::from(operands & 0x00ff);
                    let right_reg = usize::from(operands >> 8);
                    let left = read_register(&registers, left_reg, pc)?;
                    let right = read_register(&registers, right_reg, pc)?;
                    let value = arithmetic(instruction.opcode, left, right, pc)?;
                    write_register(&mut registers, dest, value, pc)?;
                }
                0xb0..=0xba => {
                    let dest = usize::from((word >> 8) & 0x0f);
                    let right_reg = usize::from((word >> 12) & 0x0f);
                    let left = read_register(&registers, dest, pc)?;
                    let right = read_register(&registers, right_reg, pc)?;
                    let value = arithmetic(instruction.opcode - 0x20, left, right, pc)?;
                    write_register(&mut registers, dest, value, pc)?;
                }
                0x71 | 0x77 => {
                    let method_idx = u32::from(code.insns[pc + 1]);
                    let (count, first_register, argument_registers) = if instruction.opcode == 0x71
                    {
                        let count = usize::from((word >> 8) & 0x0f);
                        if count > 5 {
                            return Err(EngineError::InvalidInstructionEncoding { pc, word });
                        }
                        let packed = code.insns[pc + 2];
                        let regs = [
                            usize::from(packed & 0x000f),
                            usize::from((packed >> 4) & 0x000f),
                            usize::from((packed >> 8) & 0x000f),
                            usize::from((packed >> 12) & 0x000f),
                            usize::from((word >> 12) & 0x000f),
                        ];
                        (count, 0usize, regs.to_vec())
                    } else {
                        (
                            usize::from(word >> 8),
                            usize::from(code.insns[pc + 2]),
                            Vec::new(),
                        )
                    };
                    if count > usize::from(code.outs_size) {
                        return Err(EngineError::OutgoingArgumentCountExceeded {
                            pc,
                            count,
                            outs_size: code.outs_size,
                        });
                    }
                    let mut arguments = Vec::with_capacity(count);
                    if instruction.opcode == 0x71 {
                        for register in argument_registers.into_iter().take(count) {
                            arguments.push(read_register(&registers, register, pc)?);
                        }
                    } else {
                        for offset in 0..count {
                            arguments.push(read_register(&registers, first_register + offset, pc)?);
                        }
                    }
                    let Some(call_resolver) = resolver.as_deref_mut() else {
                        return Err(EngineError::UnsupportedOpcode {
                            pc,
                            opcode: instruction.opcode,
                        });
                    };
                    let result = call_resolver(method_idx, &arguments, budget).map_err(|error| {
                        static_call_failed(
                            method_idx,
                            format!(
                                "{error}; call site at code unit {pc}: opcode 0x{:02x}, raw code units [{:#06x}, {:#06x}, {:#06x}], encoded argument word count {count}",
                                instruction.opcode,
                                code.insns[pc],
                                code.insns[pc + 1],
                                code.insns[pc + 2],
                            ),
                        )
                    })?;
                    pending_result = Some(result);
                }
                opcode => return Err(EngineError::UnsupportedOpcode { pc, opcode }),
            }

            pc = next_pc;
        }

        Err(EngineError::MissingReturn { pc })
    }
}

const MAX_METHOD_CALL_DEPTH: usize = 32;

#[derive(Debug)]
struct ExecutionBudget {
    max_steps: u64,
    steps: u64,
}

type StaticCallResolver<'a> =
    dyn FnMut(u32, &[i32], &mut ExecutionBudget) -> Result<EngineReturn, EngineError> + 'a;

#[derive(Debug, Clone, Copy)]
struct DecodedInstruction {
    opcode: u8,
    width: usize,
    branch_target: Option<usize>,
}

fn decode_program(
    code: &[u16],
    allow_static_calls: bool,
) -> Result<Vec<Option<DecodedInstruction>>, EngineError> {
    let mut program = vec![None; code.len()];
    let mut pc = 0usize;

    while pc < code.len() {
        let word = code[pc];
        let opcode = (word & 0x00ff) as u8;
        let width = match opcode {
            0x00 if word == 0 => 1,
            0x00 => return Err(EngineError::UnsupportedOpcode { pc, opcode }),
            0x01 | 0x0e | 0x0f | 0x12 | 0x28 | 0x7b..=0x7c | 0x8d..=0x8f | 0xb0..=0xba => 1,
            0x0a if allow_static_calls => 1,
            0x02 | 0x13 | 0x15 | 0x29 | 0x32..=0x3d | 0x90..=0x9a | 0xd0..=0xe2 => 2,
            0x03 | 0x14 | 0x2a => 3,
            0x71 | 0x77 if allow_static_calls => 3,
            _ => return Err(EngineError::UnsupportedOpcode { pc, opcode }),
        };

        let remaining = code.len() - pc;
        if width > remaining {
            return Err(EngineError::TruncatedInstruction {
                pc,
                width,
                remaining,
            });
        }
        if opcode == 0x0e && word != 0x000e {
            return Err(EngineError::InvalidInstructionEncoding { pc, word });
        }

        program[pc] = Some(DecodedInstruction {
            opcode,
            width,
            branch_target: None,
        });
        pc += width;
    }

    // Branch offsets are relative to the current instruction's code-unit offset.
    for pc in 0..code.len() {
        let Some(mut instruction) = program[pc] else {
            continue;
        };
        let offset = match instruction.opcode {
            0x28 => Some(i64::from((code[pc] >> 8) as u8 as i8)),
            0x29 => Some(i64::from(code[pc + 1] as i16)),
            0x2a => {
                let raw = u32::from(code[pc + 1]) | (u32::from(code[pc + 2]) << 16);
                Some(i64::from(raw as i32))
            }
            0x32..=0x3d => Some(i64::from(code[pc + 1] as i16)),
            _ => None,
        };

        if let Some(offset) = offset {
            let target = (pc as i64)
                .checked_add(offset)
                .ok_or(EngineError::InvalidBranchTarget { pc, target: offset })?;
            let target_index = usize::try_from(target)
                .map_err(|_| EngineError::InvalidBranchTarget { pc, target })?;
            if target_index >= program.len() {
                return Err(EngineError::InvalidBranchTarget { pc, target });
            }
            if program[target_index].is_none() {
                return Err(EngineError::BranchToNonInstruction {
                    pc,
                    target: target_index,
                });
            }
            instruction.branch_target = Some(target_index);
            program[pc] = Some(instruction);
        }
    }

    Ok(program)
}

fn read_register(registers: &[i32], register: usize, pc: usize) -> Result<i32, EngineError> {
    registers
        .get(register)
        .copied()
        .ok_or(EngineError::InvalidRegister {
            pc,
            register,
            register_count: registers.len(),
        })
}

fn write_register(
    registers: &mut [i32],
    register: usize,
    value: i32,
    pc: usize,
) -> Result<(), EngineError> {
    let count = registers.len();
    let slot = registers
        .get_mut(register)
        .ok_or(EngineError::InvalidRegister {
            pc,
            register,
            register_count: count,
        })?;
    *slot = value;
    Ok(())
}

fn compare(opcode: u8, left: i32, right: i32) -> bool {
    match opcode {
        0x32 => left == right,
        0x33 => left != right,
        0x34 => left < right,
        0x35 => left >= right,
        0x36 => left > right,
        0x37 => left <= right,
        _ => false,
    }
}

fn compare_zero(opcode: u8, value: i32) -> bool {
    match opcode {
        0x38 => value == 0,
        0x39 => value != 0,
        0x3a => value < 0,
        0x3b => value >= 0,
        0x3c => value > 0,
        0x3d => value <= 0,
        _ => false,
    }
}

fn arithmetic_literal(
    opcode: u8,
    source: i32,
    literal: i32,
    pc: usize,
) -> Result<i32, EngineError> {
    match opcode {
        0xd0 | 0xd8 => arithmetic(0x90, source, literal, pc),
        0xd1 | 0xd9 => arithmetic(0x91, literal, source, pc),
        0xd2 | 0xda => arithmetic(0x92, source, literal, pc),
        0xd3 | 0xdb => arithmetic(0x93, source, literal, pc),
        0xd4 | 0xdc => arithmetic(0x94, source, literal, pc),
        0xd5 | 0xdd => arithmetic(0x95, source, literal, pc),
        0xd6 | 0xde => arithmetic(0x96, source, literal, pc),
        0xd7 | 0xdf => arithmetic(0x97, source, literal, pc),
        0xe0 => arithmetic(0x98, source, literal, pc),
        0xe1 => arithmetic(0x99, source, literal, pc),
        0xe2 => arithmetic(0x9a, source, literal, pc),
        _ => unreachable!("only supported literal arithmetic opcodes are dispatched"),
    }
}

fn arithmetic(opcode: u8, left: i32, right: i32, pc: usize) -> Result<i32, EngineError> {
    Ok(match opcode {
        0x90 => left.wrapping_add(right),
        0x91 => left.wrapping_sub(right),
        0x92 => left.wrapping_mul(right),
        0x93 => {
            if right == 0 {
                return Err(EngineError::DivisionByZero { pc });
            }
            if left == i32::MIN && right == -1 {
                i32::MIN
            } else {
                left / right
            }
        }
        0x94 => {
            if right == 0 {
                return Err(EngineError::DivisionByZero { pc });
            }
            if left == i32::MIN && right == -1 {
                0
            } else {
                left % right
            }
        }
        0x95 => left & right,
        0x96 => left | right,
        0x97 => left ^ right,
        0x98 => left.wrapping_shl((right as u32) & 0x1f),
        0x99 => left >> ((right as u32) & 0x1f),
        0x9a => ((left as u32) >> ((right as u32) & 0x1f)) as i32,
        _ => unreachable!("only supported arithmetic opcodes are dispatched"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dex::testutil::{TEST_CLASS, TestMethod, build_dex, fix_checksum};

    fn code(registers_size: u16, ins_size: u16, insns: &[u16]) -> CodeItem {
        CodeItem {
            registers_size,
            ins_size,
            outs_size: 0,
            tries_size: 0,
            debug_info_off: 0,
            insns: insns.to_vec(),
        }
    }

    fn run(code: &CodeItem, parameters: &[i32]) -> Result<ExecutionReport, EngineError> {
        IntegerDexEngine::default().execute(code, parameters)
    }

    #[test]
    fn executes_constants_addition_and_return() {
        // const/16 v0, 7; const/16 v1, 35; add-int v2, v0, v1; return v2
        let method = code(3, 0, &[0x0013, 7, 0x0113, 35, 0x0290, 0x0100, 0x020f]);
        assert_eq!(
            run(&method, &[]),
            Ok(ExecutionReport {
                return_value: EngineReturn::Int(42),
                instruction_steps: 4,
            })
        );
    }

    #[test]
    fn places_integer_parameters_in_the_final_input_registers() {
        // With two total registers and one input, the input is v1.
        let method = code(2, 1, &[0x010f]);
        assert_eq!(
            run(&method, &[123]),
            Ok(ExecutionReport {
                return_value: EngineReturn::Int(123),
                instruction_steps: 1,
            })
        );
    }

    #[test]
    fn sign_extends_const_four_bit_literals() {
        // const/4 v0, -1; return v0
        let method = code(1, 0, &[0xf012, 0x000f]);
        assert_eq!(
            run(&method, &[]).unwrap().return_value,
            EngineReturn::Int(-1)
        );
    }

    #[test]
    fn executes_all_integer_literal_arithmetic_opcodes() {
        let cases: &[(u8, i32)] = &[
            (0xd0, 15),
            (0xd1, -5),
            (0xd2, 50),
            (0xd3, 2),
            (0xd4, 0),
            (0xd5, 0),
            (0xd6, 15),
            (0xd7, 15),
            (0xd8, 15),
            (0xd9, -5),
            (0xda, 50),
            (0xdb, 2),
            (0xdc, 0),
            (0xdd, 0),
            (0xde, 15),
            (0xdf, 15),
            (0xe0, 320),
            (0xe1, 0),
            (0xe2, 0),
        ];

        for &(opcode, expected) in cases {
            let method = if opcode <= 0xd7 {
                // v0 = operation(v1, #5); return v0
                code(2, 1, &[0x10d0 | u16::from(opcode - 0xd0), 5, 0x000f])
            } else if opcode <= 0xdf {
                // v0 = operation(v1, #5); return v0, with the lit8 in the high byte.
                let literal_and_register = 0x0501;
                code(2, 1, &[u16::from(opcode), literal_and_register, 0x000f])
            } else {
                let literal_and_register = 0x0501;
                code(2, 1, &[u16::from(opcode), literal_and_register, 0x000f])
            };
            let result = run(&method, &[10]);
            assert_eq!(
                result.map(|report| report.return_value),
                Ok(EngineReturn::Int(expected)),
                "opcode 0x{opcode:02x}"
            );
        }
    }

    #[test]
    fn executes_unary_integer_and_narrowing_opcodes() {
        for (opcode, input, expected) in [
            (0x7b_u8, 12_i32, -12_i32),
            (0x7c, 0_i32, -1_i32),
            (0x8d, 255_i32, -1_i32),
            (0x8e, -1_i32, 65_535_i32),
            (0x8f, 65_535_i32, -1_i32),
        ] {
            // const/16 v1, literal; unary-op v0, v1; return v0.
            let value = input as u16 as i16;
            let mut units = vec![0x0113, value as u16];
            units.push(0x1000 | u16::from(opcode));
            units.push(0x000f);
            let method = code(2, 0, &units);
            assert_eq!(
                run(&method, &[]).map(|report| report.return_value),
                Ok(EngineReturn::Int(expected)),
                "opcode 0x{opcode:02x}"
            );
        }
    }

    #[test]
    fn branches_to_valid_instruction_boundaries() {
        // const/4 v0, 1; const/4 v1, 1; if-eq v0, v1, +4;
        // const/4 v2, 2; return v2; const/4 v2, 3; return v2
        let method = code(
            3,
            0,
            &[0x1012, 0x1112, 0x1032, 4, 0x2212, 0x020f, 0x3212, 0x020f],
        );
        assert_eq!(
            run(&method, &[]).unwrap().return_value,
            EngineReturn::Int(3)
        );
    }

    #[test]
    fn handles_two_address_arithmetic() {
        // const/4 v0, 5; const/4 v1, 6; add-int/2addr v0, v1; return v0
        let method = code(2, 0, &[0x5012, 0x6112, 0x10b0, 0x000f]);
        assert_eq!(
            run(&method, &[]).unwrap().return_value,
            EngineReturn::Int(11)
        );
    }

    #[test]
    fn rejects_division_by_zero() {
        // const/4 v0, 1; const/4 v1, 0; div-int v2, v0, v1
        let method = code(3, 0, &[0x1012, 0x0012, 0x0293, 0x0100, 0x020f]);
        assert_eq!(
            run(&method, &[]),
            Err(EngineError::DivisionByZero { pc: 2 })
        );
    }

    #[test]
    fn bounds_infinite_loops_with_a_step_limit() {
        let method = code(0, 0, &[0x0028]); // goto +0
        let engine = IntegerDexEngine::new(EngineLimits {
            max_code_units: 8,
            max_instruction_steps: 3,
        });
        assert_eq!(
            engine.execute(&method, &[]),
            Err(EngineError::InstructionStepLimitExceeded { max: 3 })
        );
    }

    #[test]
    fn rejects_branches_into_instruction_operands() {
        // const/16 occupies code units 0 and 1; goto at 2 targets unit 1.
        let method = code(1, 0, &[0x0013, 7, 0xff28]);
        assert_eq!(
            run(&method, &[]),
            Err(EngineError::BranchToNonInstruction { pc: 2, target: 1 })
        );
    }

    #[test]
    fn rejects_unsupported_opcodes_before_execution() {
        let method = code(0, 0, &[0x0071, 0x000e]); // no resolver is provided
        assert_eq!(
            run(&method, &[]),
            Err(EngineError::UnsupportedOpcode {
                pc: 0,
                opcode: 0x71
            })
        );
    }

    #[test]
    fn rejects_methods_with_exception_handlers() {
        let mut method = code(0, 0, &[0x000e]);
        method.tries_size = 1;
        assert_eq!(run(&method, &[]), Err(EngineError::TryCatchUnsupported));
    }

    #[test]
    fn rejects_wrong_parameter_counts_and_register_accesses() {
        let parameter_method = code(2, 1, &[0x010f]);
        assert_eq!(
            run(&parameter_method, &[]),
            Err(EngineError::ParameterCountMismatch {
                expected: 1,
                actual: 0,
            })
        );

        let bad_register = code(1, 0, &[0x0412, 0x000e]);
        assert_eq!(
            run(&bad_register, &[]),
            Err(EngineError::InvalidRegister {
                pc: 0,
                register: 4,
                register_count: 1,
            })
        );
    }

    #[test]
    fn rejects_truncated_and_fallthrough_methods() {
        let truncated = code(1, 0, &[0x0013]);
        assert_eq!(
            run(&truncated, &[]),
            Err(EngineError::TruncatedInstruction {
                pc: 0,
                width: 2,
                remaining: 1,
            })
        );

        let fallthrough = code(0, 0, &[0x0000]);
        assert_eq!(
            run(&fallthrough, &[]),
            Err(EngineError::MissingReturn { pc: 1 })
        );
    }
    #[test]
    fn resolves_and_executes_an_integer_method_from_real_dex_metadata() {
        let bytes = build_dex(&[TestMethod {
            name: "sum",
            return_type: "I",
            params: vec!["I", "I"],
            registers: 3,
            ins: 2,
            outs: 0,
            // add-int v0, v1, v2; return v0
            insns: vec![0x0090, 0x0201, 0x000f],
        }]);
        let dex =
            DexFile::parse(&bytes, &crate::dex::DexLimits::default()).expect("parse synthetic DEX");

        let result = execute_static_integer_method(&dex, TEST_CLASS, "sum", &[19, 23])
            .expect("execute method from DEX");

        assert_eq!(result.signature.name, "sum");
        assert_eq!(result.execution.return_value, EngineReturn::Int(42));
        assert_eq!(result.execution.instruction_steps, 2);
    }

    #[test]
    fn executes_a_static_method_call_from_the_same_dex_file() {
        let bytes = build_dex(&[
            TestMethod {
                name: "sum",
                return_type: "I",
                params: vec!["I", "I"],
                registers: 3,
                ins: 2,
                outs: 0,
                // add-int v0, v1, v2; return v0
                insns: vec![0x0090, 0x0201, 0x000f],
            },
            TestMethod {
                name: "caller",
                return_type: "I",
                params: vec!["I", "I"],
                registers: 3,
                ins: 2,
                outs: 2,
                // invoke-static {v1, v2}, method@0; move-result v0; return v0
                insns: vec![0x0271, 0x0000, 0x0021, 0x000a, 0x000f],
            },
        ]);
        let dex =
            DexFile::parse(&bytes, &crate::dex::DexLimits::default()).expect("parse synthetic DEX");

        let result = execute_static_integer_method(&dex, TEST_CLASS, "caller", &[19, 23])
            .expect("execute caller and callee");

        assert_eq!(result.execution.return_value, EngineReturn::Int(42));
        assert_eq!(result.execution.instruction_steps, 5);
    }

    #[test]
    fn executes_static_call_range_encoding() {
        let bytes = build_dex(&[
            TestMethod {
                name: "sum",
                return_type: "I",
                params: vec!["I", "I"],
                registers: 3,
                ins: 2,
                outs: 0,
                insns: vec![0x0090, 0x0201, 0x000f],
            },
            TestMethod {
                name: "callerRange",
                return_type: "I",
                params: vec!["I", "I"],
                registers: 3,
                ins: 2,
                outs: 2,
                // invoke-static/range {v1 .. v2}, method@0; move-result v0; return v0
                insns: vec![0x0277, 0x0000, 0x0001, 0x000a, 0x000f],
            },
        ]);
        let dex =
            DexFile::parse(&bytes, &crate::dex::DexLimits::default()).expect("parse synthetic DEX");

        let result = execute_static_integer_method(&dex, TEST_CLASS, "callerRange", &[19, 23])
            .expect("execute invoke-static/range");

        assert_eq!(result.execution.return_value, EngineReturn::Int(42));
        assert_eq!(result.execution.instruction_steps, 5);
    }

    #[test]
    fn supports_void_static_calls_without_move_result() {
        let bytes = build_dex(&[
            TestMethod {
                name: "touch",
                return_type: "V",
                params: vec!["I"],
                registers: 1,
                ins: 1,
                outs: 0,
                insns: vec![0x000e],
            },
            TestMethod {
                name: "caller",
                return_type: "I",
                params: vec!["I"],
                registers: 1,
                ins: 1,
                outs: 1,
                // invoke-static {v0}, method@0; return v0
                insns: vec![0x0171, 0x0000, 0x0000, 0x000f],
            },
        ]);
        let dex =
            DexFile::parse(&bytes, &crate::dex::DexLimits::default()).expect("parse synthetic DEX");

        let result = execute_static_integer_method(&dex, TEST_CLASS, "caller", &[42])
            .expect("execute void static call");

        assert_eq!(result.execution.return_value, EngineReturn::Int(42));
        assert_eq!(result.execution.instruction_steps, 3);
    }

    #[test]
    fn bounds_recursive_static_calls_by_call_depth() {
        let bytes = build_dex(&[TestMethod {
            name: "recurse",
            return_type: "I",
            params: vec!["I"],
            registers: 2,
            ins: 1,
            outs: 1,
            // invoke-static {v1}, method@0; move-result v0; return v0
            insns: vec![0x0171, 0x0000, 0x0001, 0x000a, 0x000f],
        }]);
        let dex =
            DexFile::parse(&bytes, &crate::dex::DexLimits::default()).expect("parse synthetic DEX");

        assert!(matches!(
            execute_static_integer_method(&dex, TEST_CLASS, "recurse", &[1]),
            Err(MethodExecutionError::Engine(
                EngineError::MethodCallDepthLimitExceeded {
                    max_depth: MAX_METHOD_CALL_DEPTH
                }
            ))
        ));
    }

    #[test]
    fn shares_instruction_step_budget_across_static_calls() {
        let mut caller = code(3, 2, &[0x0271, 0x0000, 0x0021, 0x000a, 0x000f]);
        caller.outs_size = 2;
        let callee = code(3, 2, &[0x0090, 0x0201, 0x000f]);
        let engine = IntegerDexEngine::new(EngineLimits {
            max_code_units: 8,
            max_instruction_steps: 4,
        });
        let mut budget = ExecutionBudget {
            max_steps: 4,
            steps: 0,
        };
        let mut resolver = |method_idx, arguments: &[i32], budget: &mut ExecutionBudget| {
            assert_eq!(method_idx, 0);
            engine
                .execute_with_budget(&callee, arguments, budget, None)
                .map(|report| report.return_value)
        };

        assert_eq!(
            engine.execute_with_budget(&caller, &[19, 23], &mut budget, Some(&mut resolver)),
            Err(EngineError::InstructionStepLimitExceeded { max: 4 })
        );
    }

    #[test]
    fn models_api30_window_insets_type_constants_only_for_exact_signatures() {
        let class_descriptor = "Landroid/view/WindowInsets$Type;";
        for (name, expected) in [
            ("statusBars", 1),
            ("navigationBars", 2),
            ("captionBar", 4),
            ("ime", 8),
            ("systemGestures", 16),
            ("mandatorySystemGestures", 32),
            ("tappableElement", 64),
            ("displayCutout", 128),
        ] {
            let signature = MethodSignature {
                class_descriptor: class_descriptor.to_owned(),
                name: name.to_owned(),
                shorty: "I".to_owned(),
                parameter_types: Vec::new(),
                return_type: "I".to_owned(),
            };
            assert_eq!(
                api30_window_insets_type_intrinsic(&signature, &[]),
                Some(expected),
                "{name}"
            );
        }

        let extension_signature = MethodSignature {
            class_descriptor: "Landroid/os/ext/SdkExtensions;".to_owned(),
            name: "getExtensionVersion".to_owned(),
            shorty: "II".to_owned(),
            parameter_types: vec!["I".to_owned()],
            return_type: "I".to_owned(),
        };
        assert_eq!(
            api30_window_insets_type_intrinsic(&extension_signature, &[0]),
            None
        );

        let wrong_arity = MethodSignature {
            class_descriptor: class_descriptor.to_owned(),
            name: "statusBars".to_owned(),
            shorty: "II".to_owned(),
            parameter_types: vec!["I".to_owned()],
            return_type: "I".to_owned(),
        };
        assert_eq!(api30_window_insets_type_intrinsic(&wrong_arity, &[0]), None);
    }

    #[test]
    fn resolves_static_method_definitions_across_loaded_dex_files() {
        fn read_uleb(bytes: &[u8], cursor: &mut usize) -> u32 {
            let mut value = 0_u32;
            let mut shift = 0;
            loop {
                let byte = bytes[*cursor];
                *cursor += 1;
                value |= u32::from(byte & 0x7f) << shift;
                if byte & 0x80 == 0 {
                    return value;
                }
                shift += 7;
            }
        }

        let mut caller_bytes = build_dex(&[
            TestMethod {
                name: "sum",
                return_type: "I",
                params: vec!["I", "I"],
                registers: 3,
                ins: 2,
                outs: 0,
                insns: vec![0x000f], // ID remains as call target; definition removed below
            },
            TestMethod {
                name: "caller",
                return_type: "I",
                params: vec!["I", "I"],
                registers: 3,
                ins: 2,
                outs: 2,
                // invoke-static {v1, v2}, method@0; move-result v0; return v0
                insns: vec![0x0271, 0x0000, 0x0021, 0x000a, 0x000f],
            },
        ]);

        // Keep the method ID for sum in this DEX, but remove its class_data
        // definition. This models a call target defined in classes2.dex.
        let class_defs_off =
            u32::from_le_bytes(caller_bytes[0x64..0x68].try_into().expect("class_defs_off"))
                as usize;
        let class_data_off = u32::from_le_bytes(
            caller_bytes[class_defs_off + 24..class_defs_off + 28]
                .try_into()
                .expect("class_data_off"),
        ) as usize;
        assert_eq!(caller_bytes[class_data_off + 2], 2);
        let first_entry = class_data_off + 4;
        let mut second_entry = first_entry;
        for _ in 0..3 {
            read_uleb(&caller_bytes, &mut second_entry);
        }
        let second_start = second_entry;
        for _ in 0..3 {
            read_uleb(&caller_bytes, &mut second_entry);
        }
        let second_record = caller_bytes[second_start..second_entry].to_vec();
        assert_eq!(second_record.len(), second_start - first_entry);
        caller_bytes[class_data_off + 2] = 1;
        caller_bytes[first_entry..second_start].copy_from_slice(&second_record);
        fix_checksum(&mut caller_bytes);

        let callee_bytes = build_dex(&[TestMethod {
            name: "sum",
            return_type: "I",
            params: vec!["I", "I"],
            registers: 3,
            ins: 2,
            outs: 0,
            insns: vec![0x0090, 0x0201, 0x000f], // add-int v0, v1, v2; return v0
        }]);

        let missing_caller = DexFile::parse(&caller_bytes, &crate::dex::DexLimits::default())
            .expect("parse caller DEX for missing-callee diagnostic");
        let missing_callee_error = execute_static_scalar_method_in_dex_set(
            std::slice::from_ref(&missing_caller),
            TEST_CLASS,
            "caller",
            &[19, 23],
        )
        .expect_err("refuse a callee whose definition is not loaded");
        assert!(
            missing_callee_error.to_string().contains("->sum(I,I)I"),
            "diagnostic should include the missing callee signature: {missing_callee_error}"
        );

        let caller = DexFile::parse(&caller_bytes, &crate::dex::DexLimits::default())
            .expect("parse caller DEX");
        let callee = DexFile::parse(&callee_bytes, &crate::dex::DexLimits::default())
            .expect("parse callee DEX");

        let report = execute_static_scalar_method_in_dex_set(
            &[caller, callee],
            TEST_CLASS,
            "caller",
            &[19, 23],
        )
        .expect("resolve callee from a different DEX file");
        assert_eq!(report.execution.return_value, EngineReturn::Int(42));
        assert_eq!(report.execution.instruction_steps, 5);
    }

    #[test]
    fn normalizes_boolean_byte_char_and_short_scalar_signatures() {
        let bytes = build_dex(&[
            TestMethod {
                name: "truthy",
                return_type: "Z",
                params: vec!["Z"],
                registers: 2,
                ins: 1,
                outs: 0,
                // return the boolean input in v1; it is normalized at method entry.
                insns: vec![0x010f],
            },
            TestMethod {
                name: "asByte",
                return_type: "B",
                params: vec![],
                registers: 1,
                ins: 0,
                outs: 0,
                // const/16 v0, 255; return v0
                insns: vec![0x0013, 255, 0x000f],
            },
            TestMethod {
                name: "asChar",
                return_type: "C",
                params: vec![],
                registers: 1,
                ins: 0,
                outs: 0,
                // const/16 v0, -1; return v0
                insns: vec![0x0013, 0xffff, 0x000f],
            },
            TestMethod {
                name: "asShort",
                return_type: "S",
                params: vec![],
                registers: 1,
                ins: 0,
                outs: 0,
                insns: vec![0x0013, 0xffff, 0x000f],
            },
        ]);
        let dex =
            DexFile::parse(&bytes, &crate::dex::DexLimits::default()).expect("parse synthetic DEX");

        let candidates =
            list_static_scalar_methods(&dex, Some(TEST_CLASS)).expect("list scalar candidates");
        assert_eq!(candidates.len(), 4);

        assert_eq!(
            execute_static_scalar_method(&dex, TEST_CLASS, "truthy", &[42])
                .expect("execute boolean")
                .execution
                .return_value,
            EngineReturn::Int(1)
        );
        assert_eq!(
            execute_static_scalar_method(&dex, TEST_CLASS, "truthy", &[0])
                .expect("execute false boolean")
                .execution
                .return_value,
            EngineReturn::Int(0)
        );
        assert_eq!(
            execute_static_scalar_method(&dex, TEST_CLASS, "asByte", &[])
                .expect("execute byte")
                .execution
                .return_value,
            EngineReturn::Int(-1)
        );
        assert_eq!(
            execute_static_scalar_method(&dex, TEST_CLASS, "asChar", &[])
                .expect("execute char")
                .execution
                .return_value,
            EngineReturn::Int(65_535)
        );
        assert_eq!(
            execute_static_scalar_method(&dex, TEST_CLASS, "asShort", &[])
                .expect("execute short")
                .execution
                .return_value,
            EngineReturn::Int(-1)
        );
    }

    #[test]
    fn lists_only_static_integer_methods_with_code() {
        let bytes = build_dex(&[TestMethod {
            name: "answer",
            return_type: "I",
            params: vec![],
            registers: 1,
            ins: 0,
            outs: 0,
            insns: vec![0x0013, 42, 0x000f],
        }]);
        let dex =
            DexFile::parse(&bytes, &crate::dex::DexLimits::default()).expect("parse synthetic DEX");

        let methods =
            list_static_integer_methods(&dex, Some(TEST_CLASS)).expect("list supported signatures");
        assert_eq!(methods.len(), 1);
        assert_eq!(methods[0].name, "answer");
        assert_eq!(methods[0].return_type, "I");
    }

    #[test]
    fn reports_wrong_argument_count_for_resolved_method() {
        let bytes = build_dex(&[TestMethod {
            name: "sum",
            return_type: "I",
            params: vec!["I", "I"],
            registers: 3,
            ins: 2,
            outs: 0,
            insns: vec![0x0090, 0x0201, 0x000f],
        }]);
        let dex =
            DexFile::parse(&bytes, &crate::dex::DexLimits::default()).expect("parse synthetic DEX");

        assert!(matches!(
            execute_static_integer_method(&dex, TEST_CLASS, "sum", &[42]),
            Err(MethodExecutionError::ArgumentCountMismatch {
                actual: 1,
                expected,
                ..
            }) if expected == vec![2]
        ));
    }

    #[test]
    fn refuses_to_execute_a_missing_method() {
        let bytes = build_dex(&[TestMethod {
            name: "answer",
            return_type: "I",
            params: vec![],
            registers: 1,
            ins: 0,
            outs: 0,
            insns: vec![0x0013, 42, 0x000f],
        }]);
        let dex =
            DexFile::parse(&bytes, &crate::dex::DexLimits::default()).expect("parse synthetic DEX");
        assert!(matches!(
            execute_static_integer_method(&dex, TEST_CLASS, "missing", &[]),
            Err(MethodExecutionError::MethodNotFound { .. })
        ));
    }
}
