//! A small, project-owned DEX integer execution engine.
//!
//! This is an early runtime component, not Android Runtime (ART) and not a
//! complete Android application environment. It executes only a bounded subset
//! of DEX instructions over 32-bit integer registers. Objects, method calls,
//! fields, arrays, exceptions, Android framework APIs, native libraries, and
//! graphics are intentionally unsupported.
//!
//! The input is a parsed CodeItem. It must have no try/catch table and must
//! use only instructions listed in IntegerDexEngine's implementation.
//! Unsupported instructions are rejected before execution.

use crate::dex::CodeItem;
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

/// A refusal or failure from the project-owned integer DEX engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EngineError {
    CodeTooLarge { size: usize, max: usize },
    InstructionStepLimitExceeded { max: u64 },
    TryCatchUnsupported,
    InvalidRegisterLayout { registers_size: u16, ins_size: u16 },
    ParameterCountMismatch { expected: usize, actual: usize },
    TruncatedInstruction { pc: usize, width: usize, remaining: usize },
    UnsupportedOpcode { pc: usize, opcode: u8 },
    InvalidInstructionEncoding { pc: usize, word: u16 },
    InvalidRegister { pc: usize, register: usize, register_count: usize },
    InvalidBranchTarget { pc: usize, target: i64 },
    BranchToNonInstruction { pc: usize, target: usize },
    DivisionByZero { pc: usize },
    MissingReturn { pc: usize },
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
            Self::InvalidRegisterLayout { registers_size, ins_size } => write!(
                f,
                "invalid DEX register layout: {ins_size} input registers exceed {registers_size} total registers"
            ),
            Self::ParameterCountMismatch { expected, actual } => write!(
                f,
                "DEX method expects {expected} 32-bit integer parameters, received {actual}"
            ),
            Self::TruncatedInstruction { pc, width, remaining } => write!(
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
            Self::InvalidRegister { pc, register, register_count } => write!(
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
                write!(f, "DEX method reached code unit {pc} without a return instruction")
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
/// - 32-bit integer arithmetic/bitwise operations and their /2addr forms.
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

        let program = decode_program(&code.insns)?;
        let mut registers = vec![0_i32; usize::from(code.registers_size)];
        let parameter_start = registers.len() - expected_parameters;
        registers[parameter_start..].copy_from_slice(parameters);

        let mut pc = 0usize;
        let mut steps = 0u64;
        while pc < program.len() {
            if steps >= self.limits.max_instruction_steps {
                return Err(EngineError::InstructionStepLimitExceeded {
                    max: self.limits.max_instruction_steps,
                });
            }
            steps += 1;

            let instruction = program[pc].ok_or(EngineError::MissingReturn { pc })?;
            let word = code.insns[pc];
            let mut next_pc = pc + instruction.width;

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
                0x0e => {
                    return Ok(ExecutionReport {
                        return_value: EngineReturn::Void,
                        instruction_steps: steps,
                    });
                }
                0x0f => {
                    let src = usize::from(word >> 8);
                    return Ok(ExecutionReport {
                        return_value: EngineReturn::Int(read_register(&registers, src, pc)?),
                        instruction_steps: steps,
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
                        | (u32::from(code.insns[pc + 2]) << 16)) as i32;
                    write_register(&mut registers, dest, value, pc)?;
                }
                0x15 => {
                    let dest = usize::from(word >> 8);
                    let value = (u32::from(code.insns[pc + 1]) << 16) as i32;
                    write_register(&mut registers, dest, value, pc)?;
                }
                0x28 | 0x29 | 0x2a => {
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
                opcode => return Err(EngineError::UnsupportedOpcode { pc, opcode }),
            }

            pc = next_pc;
        }

        Err(EngineError::MissingReturn { pc })
    }
}

#[derive(Debug, Clone, Copy)]
struct DecodedInstruction {
    opcode: u8,
    width: usize,
    branch_target: Option<usize>,
}

fn decode_program(code: &[u16]) -> Result<Vec<Option<DecodedInstruction>>, EngineError> {
    let mut program = vec![None; code.len()];
    let mut pc = 0usize;

    while pc < code.len() {
        let word = code[pc];
        let opcode = (word & 0x00ff) as u8;
        let width = match opcode {
            0x00 if word == 0 => 1,
            0x00 => return Err(EngineError::UnsupportedOpcode { pc, opcode }),
            0x01 | 0x0e | 0x0f | 0x12 | 0x28 | 0xb0..=0xba => 1,
            0x02 | 0x13 | 0x15 | 0x29 | 0x32..=0x3d | 0x90..=0x9a => 2,
            0x03 | 0x14 | 0x2a => 3,
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

fn read_register(
    registers: &[i32],
    register: usize,
    pc: usize,
) -> Result<i32, EngineError> {
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
        assert_eq!(run(&method, &[]).unwrap().return_value, EngineReturn::Int(-1));
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
        assert_eq!(run(&method, &[]).unwrap().return_value, EngineReturn::Int(3));
    }

    #[test]
    fn handles_two_address_arithmetic() {
        // const/4 v0, 5; const/4 v1, 6; add-int/2addr v0, v1; return v0
        let method = code(2, 0, &[0x5012, 0x6112, 0x10b0, 0x000f]);
        assert_eq!(run(&method, &[]).unwrap().return_value, EngineReturn::Int(11));
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
        let method = code(0, 0, &[0x0071, 0x000e]); // invoke-static is unsupported
        assert_eq!(
            run(&method, &[]),
            Err(EngineError::UnsupportedOpcode { pc: 0, opcode: 0x71 })
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
}
