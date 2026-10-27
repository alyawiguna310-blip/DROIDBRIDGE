//! A small, project-owned DEX integer execution engine.
//!
//! This is an early runtime component, not Android Runtime (ART) and not a
//! complete Android application environment. It executes only a bounded subset
//! of DEX instructions over 32-bit integer registers. Same-DEX static calls
//! between integer/void methods are supported with bounded depth and a shared
//! instruction budget. Objects, fields, arrays, exceptions, Android framework
//! APIs, native libraries, and graphics are still unsupported.
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

/// List methods whose signature is supported by the current execution API.
///
/// This checks metadata only; it does not guarantee that every instruction
/// inside the method is supported. Results follow the class/method order in
/// the DEX file and can be filtered to one class descriptor.
pub fn list_static_integer_methods(
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
            if signature.return_type == "I" && signature.parameter_types.iter().all(|ty| ty == "I")
            {
                found.push(signature);
            }
        }
    }
    Ok(found)
}

/// Resolve and execute a static integer method from a parsed DEX file.
///
/// Only methods returning a 32-bit integer and taking zero or more 32-bit
/// integer arguments are accepted. Instance methods, object/reference
/// parameters, wide values, method calls, and other Android runtime behavior
/// are outside this engine's scope. The selected code item is passed to the
/// bounded interpreter, which rejects unsupported instructions and try/catch
/// tables.
pub fn execute_static_integer_method(
    dex: &DexFile<'_>,
    class_descriptor: &str,
    method_name: &str,
    parameters: &[i32],
) -> Result<MethodExecutionReport, MethodExecutionError> {
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
            same_name.push((*method, signature));
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
        .filter(|(method, _)| method.access_flags & 0x8 != 0)
        .collect();
    if static_methods.is_empty() {
        return Err(MethodExecutionError::MethodIsNotStatic {
            class_descriptor: class_descriptor.to_owned(),
            method_name: method_name.to_owned(),
        });
    }

    let mut expected: Vec<usize> = static_methods
        .iter()
        .map(|(_, signature)| signature.parameter_types.len())
        .collect();
    expected.sort_unstable();
    expected.dedup();

    let matching_arity: Vec<_> = static_methods
        .into_iter()
        .filter(|(_, signature)| signature.parameter_types.len() == parameters.len())
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
        .filter(|(_, signature)| {
            signature.return_type == "I" && signature.parameter_types.iter().all(|ty| ty == "I")
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

    let (method, signature) = supported.into_iter().next().expect("one supported match");
    if method.code_off == 0 {
        return Err(MethodExecutionError::MethodHasNoCode {
            signature: method_signature_label(&signature),
        });
    }
    let code = dex.code_item(method.code_off)?;
    let engine = IntegerDexEngine::default();
    let mut budget = ExecutionBudget {
        max_steps: engine.limits.max_instruction_steps,
        steps: 0,
    };
    let mut resolver = |target_method_idx: u32, arguments: &[i32], budget: &mut ExecutionBudget| {
        execute_dex_method_index(dex, target_method_idx, arguments, &engine, budget, 1)
    };
    let execution =
        engine.execute_with_budget(&code, parameters, &mut budget, Some(&mut resolver))?;
    Ok(MethodExecutionReport {
        signature,
        execution,
    })
}

fn execute_dex_method_index(
    dex: &DexFile<'_>,
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

    let (method, signature) = find_encoded_method(dex, method_idx)?;
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
    if !signature.parameter_types.iter().all(|ty| ty == "I") {
        return Err(static_call_failed(
            method_idx,
            "only 32-bit integer parameters are supported",
        ));
    }
    if signature.return_type != "I" && signature.return_type != "V" {
        return Err(static_call_failed(
            method_idx,
            "only int and void return types are supported",
        ));
    }
    if parameters.len() != signature.parameter_types.len() {
        return Err(static_call_failed(
            method_idx,
            "argument count does not match the DEX method signature",
        ));
    }

    let code = dex
        .code_item(method.code_off)
        .map_err(|error| static_call_failed(method_idx, error))?;
    let mut resolver = |child_method_idx: u32, arguments: &[i32], budget: &mut ExecutionBudget| {
        execute_dex_method_index(dex, child_method_idx, arguments, engine, budget, depth + 1)
    };
    let report = engine.execute_with_budget(&code, parameters, budget, Some(&mut resolver))?;

    match (signature.return_type.as_str(), report.return_value) {
        ("I", EngineReturn::Int(value)) => Ok(EngineReturn::Int(value)),
        ("V", EngineReturn::Void) => Ok(EngineReturn::Void),
        ("I", EngineReturn::Void) => Err(static_call_failed(
            method_idx,
            "method is declared to return int but executed return-void",
        )),
        ("V", EngineReturn::Int(_)) => Err(static_call_failed(
            method_idx,
            "method is declared void but executed an integer return",
        )),
        _ => unreachable!("only int and void signatures passed validation"),
    }
}

fn find_encoded_method(
    dex: &DexFile<'_>,
    method_idx: u32,
) -> Result<(EncodedMethod, MethodSignature), EngineError> {
    let signature = dex
        .method_signature(method_idx)
        .map_err(|error| static_call_failed(method_idx, error))?;
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
        if let Some(method) = data
            .direct_methods
            .iter()
            .chain(data.virtual_methods.iter())
            .find(|method| method.method_idx == method_idx)
        {
            return Ok((*method, signature));
        }
    }
    Err(static_call_failed(
        method_idx,
        "method definition is not present in this DEX file",
    ))
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
                    pending_result = Some(call_resolver(method_idx, &arguments, budget)?);
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
    use crate::dex::testutil::{TEST_CLASS, TestMethod, build_dex};

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
                code(
                    2,
                    1,
                    &[
                        u16::from(opcode),
                        literal_and_register,
                        0x000f,
                    ],
                )
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
        let caller = code(3, 2, &[0x0271, 0x0000, 0x0021, 0x000a, 0x000f]);
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
