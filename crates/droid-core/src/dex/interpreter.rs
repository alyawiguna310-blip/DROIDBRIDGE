//! Bounded decoder and integer-only interpreter for a small DEX opcode subset.
//!
//! This is a core-development aid, not ART and not an Android application
//! runtime. It intentionally rejects unsupported opcodes and methods with
//! try/catch tables instead of trying to execute unfamiliar bytecode.

use super::file::CodeItem;
use std::fmt;

/// Hard upper bound for the number of 16-bit code units in one interpreted method.
pub const MAX_INTERPRETED_CODE_UNITS: usize = 262_144;
/// Hard ceiling on executed instructions, even if a caller requests a larger limit.
pub const MAX_INTERPRETER_STEPS: usize = 1_000_000;

/// Integer arithmetic operations supported by the minimal interpreter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntegerBinaryOp {
    Add,
    Subtract,
    Multiply,
    Divide,
    Remainder,
}

/// Signed comparison conditions used by DEX if-instructions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IfCondition {
    Equal,
    NotEqual,
    LessThan,
    GreaterOrEqual,
    GreaterThan,
    LessOrEqual,
}

impl IfCondition {
    fn evaluate(self, lhs: i32, rhs: i32) -> bool {
        match self {
            Self::Equal => lhs == rhs,
            Self::NotEqual => lhs != rhs,
            Self::LessThan => lhs < rhs,
            Self::GreaterOrEqual => lhs >= rhs,
            Self::GreaterThan => lhs > rhs,
            Self::LessOrEqual => lhs <= rhs,
        }
    }
}

/// One decoded instruction in the deliberately limited supported subset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Instruction {
    Nop,
    Move { dest: u16, src: u16 },
    Const { dest: u16, value: i32 },
    Binary {
        op: IntegerBinaryOp,
        dest: u16,
        lhs: u16,
        rhs: u16,
    },
    ReturnVoid,
    Return { src: u16 },
    Goto { relative_offset: i32 },
    If {
        condition: IfCondition,
        lhs: u16,
        rhs: Option<u16>,
        relative_offset: i32,
    },
}

/// A decoded instruction and its position/length in 16-bit DEX code units.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecodedInstruction {
    pub offset: usize,
    pub width: usize,
    pub instruction: Instruction,
}

/// Decode/interpreter errors. Unsupported operations are rejected, never guessed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DexExecutionError {
    InvalidOffset {
        offset: usize,
        instruction_count: usize,
    },
    TruncatedInstruction {
        offset: usize,
        expected_units: usize,
        available_units: usize,
    },
    UnsupportedOpcode {
        offset: usize,
        opcode: u8,
    },
    InvalidRegister {
        offset: usize,
        register: u16,
        register_count: usize,
    },
    ArgumentCountMismatch {
        expected: usize,
        actual: usize,
    },
    CodeTooLarge {
        code_units: usize,
        max_code_units: usize,
    },
    InvalidCodeItem(&'static str),
    UnsupportedTryCatch,
    InvalidBranchTarget {
        offset: usize,
        target: i64,
        instruction_count: usize,
    },
    DivisionByZero {
        offset: usize,
    },
    StepLimitExceeded {
        max_steps: usize,
    },
    MissingReturn,
}

impl fmt::Display for DexExecutionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidOffset { offset, instruction_count } => {
                write!(f, "instruction offset {offset} is outside {instruction_count} code units")
            }
            Self::TruncatedInstruction { offset, expected_units, available_units } => write!(
                f,
                "instruction at code unit {offset} needs {expected_units} code units but only {available_units} remain"
            ),
            Self::UnsupportedOpcode { offset, opcode } => {
                write!(f, "unsupported DEX opcode/data marker {opcode:#04x} at code unit {offset}")
            }
            Self::InvalidRegister { offset, register, register_count } => write!(
                f,
                "register v{register} at code unit {offset} is outside register file of size {register_count}"
            ),
            Self::ArgumentCountMismatch { expected, actual } => {
                write!(f, "method expects {expected} input registers but received {actual} values")
            }
            Self::CodeTooLarge { code_units, max_code_units } => write!(
                f,
                "method has {code_units} code units, exceeding the interpreter limit of {max_code_units}"
            ),
            Self::InvalidCodeItem(reason) => write!(f, "invalid code item for integer execution: {reason}"),
            Self::UnsupportedTryCatch => {
                write!(f, "integer interpreter refuses code items with try/catch tables")
            }
            Self::InvalidBranchTarget { offset, target, instruction_count } => write!(
                f,
                "branch at code unit {offset} targets invalid code unit {target} (stream length {instruction_count})"
            ),
            Self::DivisionByZero { offset } => {
                write!(f, "integer division by zero at code unit {offset}")
            }
            Self::StepLimitExceeded { max_steps } => {
                write!(f, "DEX execution exceeded the {max_steps}-instruction step limit")
            }
            Self::MissingReturn => write!(f, "DEX instruction stream ended without a return"),
        }
    }
}

impl std::error::Error for DexExecutionError {}

/// Decode one instruction from a DEX instruction stream.
///
/// Offsets and instruction widths are measured in 16-bit code units, as in the
/// DEX format. Supported formats include move, const/4, const/16, const/32,
/// integer arithmetic, return, goto, and integer comparison branches.
pub fn decode_instruction(
    insns: &[u16],
    offset: usize,
) -> Result<DecodedInstruction, DexExecutionError> {
    let word = *insns.get(offset).ok_or(DexExecutionError::InvalidOffset {
        offset,
        instruction_count: insns.len(),
    })?;
    let opcode = (word & 0xff) as u8;
    let high_byte = (word >> 8) as u8;

    let decoded = match opcode {
        0x00 => {
            // Packed/sparse switch and fill-array payloads also have opcode byte
            // zero but a nonzero identifier in the high byte. Do not treat them
            // as no-ops.
            if high_byte != 0 {
                return Err(DexExecutionError::UnsupportedOpcode {
                    offset,
                    opcode: high_byte,
                });
            }
            (1, Instruction::Nop)
        }
        0x01 => {
            let dest = u16::from(high_byte & 0x0f);
            let src = u16::from(high_byte >> 4);
            (1, Instruction::Move { dest, src })
        }
        0x02 => {
            let width = 2;
            require_units(insns, offset, width)?;
            (
                width,
                Instruction::Move {
                    dest: u16::from(high_byte),
                    src: insns[offset + 1],
                },
            )
        }
        0x03 => {
            let width = 3;
            require_units(insns, offset, width)?;
            (
                width,
                Instruction::Move {
                    dest: insns[offset + 1],
                    src: insns[offset + 2],
                },
            )
        }
        0x0e => (1, Instruction::ReturnVoid),
        0x0f => (1, Instruction::Return { src: u16::from(high_byte) }),
        0x12 => {
            let dest = (word >> 8) & 0x0f;
            let nibble = i32::from((word >> 12) & 0x0f);
            let value = if nibble & 0x08 != 0 { nibble - 16 } else { nibble };
            (1, Instruction::Const { dest, value })
        }
        0x13 => {
            let width = 2;
            require_units(insns, offset, width)?;
            (
                width,
                Instruction::Const {
                    dest: u16::from(high_byte),
                    value: i32::from(insns[offset + 1] as i16),
                },
            )
        }
        0x14 => {
            let width = 3;
            require_units(insns, offset, width)?;
            let value = u32::from(insns[offset + 1])
                | (u32::from(insns[offset + 2]) << 16);
            (
                width,
                Instruction::Const {
                    dest: u16::from(high_byte),
                    value: value as i32,
                },
            )
        }
        0x28 => {
            let relative_offset = i32::from(high_byte as i8);
            (1, Instruction::Goto { relative_offset })
        }
        0x29 => {
            let width = 2;
            require_units(insns, offset, width)?;
            (
                width,
                Instruction::Goto {
                    relative_offset: i32::from(insns[offset + 1] as i16),
                },
            )
        }
        0x2a => {
            let width = 3;
            require_units(insns, offset, width)?;
            let relative_offset = u32::from(insns[offset + 1])
                | (u32::from(insns[offset + 2]) << 16);
            (
                width,
                Instruction::Goto {
                    relative_offset: relative_offset as i32,
                },
            )
        }
        0x32..=0x37 => {
            let width = 2;
            require_units(insns, offset, width)?;
            let condition = match opcode {
                0x32 => IfCondition::Equal,
                0x33 => IfCondition::NotEqual,
                0x34 => IfCondition::LessThan,
                0x35 => IfCondition::GreaterOrEqual,
                0x36 => IfCondition::GreaterThan,
                _ => IfCondition::LessOrEqual,
            };
            (
                width,
                Instruction::If {
                    condition,
                    lhs: u16::from(high_byte & 0x0f),
                    rhs: Some(u16::from(high_byte >> 4)),
                    relative_offset: i32::from(insns[offset + 1] as i16),
                },
            )
        }
        0x38..=0x3d => {
            let width = 2;
            require_units(insns, offset, width)?;
            let condition = match opcode {
                0x38 => IfCondition::Equal,
                0x39 => IfCondition::NotEqual,
                0x3a => IfCondition::LessThan,
                0x3b => IfCondition::GreaterOrEqual,
                0x3c => IfCondition::GreaterThan,
                _ => IfCondition::LessOrEqual,
            };
            (
                width,
                Instruction::If {
                    condition,
                    lhs: u16::from(high_byte),
                    rhs: None,
                    relative_offset: i32::from(insns[offset + 1] as i16),
                },
            )
        }
        0x90..=0x94 => {
            let width = 2;
            require_units(insns, offset, width)?;
            let op = binary_op(opcode);
            let rhs_word = insns[offset + 1];
            (
                width,
                Instruction::Binary {
                    op,
                    dest: u16::from(high_byte),
                    lhs: rhs_word & 0xff,
                    rhs: rhs_word >> 8,
                },
            )
        }
        0xb0..=0xb4 => {
            let op = binary_op(opcode - 0x20);
            let dest = (word >> 8) & 0x0f;
            let rhs = word >> 12;
            (
                1,
                Instruction::Binary {
                    op,
                    dest,
                    lhs: dest,
                    rhs,
                },
            )
        }
        _ => return Err(DexExecutionError::UnsupportedOpcode { offset, opcode }),
    };

    Ok(DecodedInstruction {
        offset,
        width: decoded.0,
        instruction: decoded.1,
    })
}

fn require_units(
    insns: &[u16],
    offset: usize,
    expected_units: usize,
) -> Result<(), DexExecutionError> {
    let available_units = insns.len().saturating_sub(offset);
    if available_units < expected_units {
        Err(DexExecutionError::TruncatedInstruction {
            offset,
            expected_units,
            available_units,
        })
    } else {
        Ok(())
    }
}

fn binary_op(opcode: u8) -> IntegerBinaryOp {
    match opcode {
        0x90 | 0xb0 => IntegerBinaryOp::Add,
        0x91 | 0xb1 => IntegerBinaryOp::Subtract,
        0x92 | 0xb2 => IntegerBinaryOp::Multiply,
        0x93 | 0xb3 => IntegerBinaryOp::Divide,
        _ => IntegerBinaryOp::Remainder,
    }
}

fn decode_program(insns: &[u16]) -> Result<Vec<Option<DecodedInstruction>>, DexExecutionError> {
    let mut program = vec![None; insns.len()];
    let mut offset = 0usize;

    while offset < insns.len() {
        let instruction = decode_instruction(insns, offset)?;
        program[offset] = Some(instruction);
        offset = offset
            .checked_add(instruction.width)
            .ok_or(DexExecutionError::InvalidCodeItem("instruction offset overflow"))?;
    }

    // Validate every branch, including branches that may not be taken, and
    // reject targets that point into an instruction's operand code units.
    for instruction in program.iter().flatten() {
        let relative_offset = match instruction.instruction {
            Instruction::Goto { relative_offset }
            | Instruction::If { relative_offset, .. } => relative_offset,
            _ => continue,
        };
        let target = instruction.offset as i64 + i64::from(relative_offset);
        if target < 0
            || target >= insns.len() as i64
            || program[target as usize].is_none()
        {
            return Err(DexExecutionError::InvalidBranchTarget {
                offset: instruction.offset,
                target,
                instruction_count: insns.len(),
            });
        }
    }

    Ok(program)
}

/// Execute a small integer-only DEX method, returning an optional 32-bit result.
///
/// Input values are placed in the trailing input registers according to the
/// CodeItem input-register count. Only 32-bit integer operations and control
/// flow are supported. Object references, wide values, invokes, fields, arrays,
/// native calls, exception handlers, and Android framework services are out of
/// scope. The caller supplies the maximum executed-instruction count to guard
/// against infinite loops. The request is capped at MAX_INTERPRETER_STEPS, and
/// methods above MAX_INTERPRETED_CODE_UNITS are rejected. A zero limit executes
/// no instructions.
pub fn execute_integer_method(
    code: &CodeItem,
    arguments: &[i32],
    max_steps: usize,
) -> Result<Option<i32>, DexExecutionError> {
    if code.tries_size != 0 {
        return Err(DexExecutionError::UnsupportedTryCatch);
    }
    if code.insns.len() > MAX_INTERPRETED_CODE_UNITS {
        return Err(DexExecutionError::CodeTooLarge {
            code_units: code.insns.len(),
            max_code_units: MAX_INTERPRETED_CODE_UNITS,
        });
    }
    if code.ins_size > code.registers_size {
        return Err(DexExecutionError::InvalidCodeItem(
            "ins_size exceeds registers_size",
        ));
    }
    if arguments.len() != usize::from(code.ins_size) {
        return Err(DexExecutionError::ArgumentCountMismatch {
            expected: usize::from(code.ins_size),
            actual: arguments.len(),
        });
    }
    if code.insns.is_empty() {
        return Err(DexExecutionError::MissingReturn);
    }

    let program = decode_program(&code.insns)?;
    let max_steps = max_steps.min(MAX_INTERPRETER_STEPS);
    let mut registers = vec![0i32; usize::from(code.registers_size)];
    let input_start = registers.len() - arguments.len();
    registers[input_start..].copy_from_slice(arguments);
    let mut pc = 0usize;

    for _ in 0..max_steps {
        let Some(decoded) = program.get(pc).copied().flatten() else {
            if pc >= program.len() {
                return Err(DexExecutionError::MissingReturn);
            }
            return Err(DexExecutionError::InvalidCodeItem(
                "program counter is not at an instruction boundary",
            ));
        };
        let next_pc = pc + decoded.width;

        match decoded.instruction {
            Instruction::Nop => pc = next_pc,
            Instruction::Move { dest, src } => {
                let value = read_register(&registers, src, pc)?;
                write_register(&mut registers, dest, value, pc)?;
                pc = next_pc;
            }
            Instruction::Const { dest, value } => {
                write_register(&mut registers, dest, value, pc)?;
                pc = next_pc;
            }
            Instruction::Binary { op, dest, lhs, rhs } => {
                let left = read_register(&registers, lhs, pc)?;
                let right = read_register(&registers, rhs, pc)?;
                let value = apply_binary(op, left, right, pc)?;
                write_register(&mut registers, dest, value, pc)?;
                pc = next_pc;
            }
            Instruction::ReturnVoid => return Ok(None),
            Instruction::Return { src } => {
                return read_register(&registers, src, pc).map(Some);
            }
            Instruction::Goto { relative_offset } => {
                pc = branch_target(pc, relative_offset, &program)?;
            }
            Instruction::If {
                condition,
                lhs,
                rhs,
                relative_offset,
            } => {
                let left = read_register(&registers, lhs, pc)?;
                let right = match rhs {
                    Some(register) => read_register(&registers, register, pc)?,
                    None => 0,
                };
                pc = if condition.evaluate(left, right) {
                    branch_target(pc, relative_offset, &program)?
                } else {
                    next_pc
                };
            }
        }
    }

    Err(DexExecutionError::StepLimitExceeded { max_steps })
}

fn branch_target(
    pc: usize,
    relative_offset: i32,
    program: &[Option<DecodedInstruction>],
) -> Result<usize, DexExecutionError> {
    let target = pc as i64 + i64::from(relative_offset);
    if target < 0
        || target >= program.len() as i64
        || program[target as usize].is_none()
    {
        return Err(DexExecutionError::InvalidBranchTarget {
            offset: pc,
            target,
            instruction_count: program.len(),
        });
    }
    Ok(target as usize)
}

fn read_register(
    registers: &[i32],
    register: u16,
    offset: usize,
) -> Result<i32, DexExecutionError> {
    registers
        .get(usize::from(register))
        .copied()
        .ok_or(DexExecutionError::InvalidRegister {
            offset,
            register,
            register_count: registers.len(),
        })
}

fn write_register(
    registers: &mut [i32],
    register: u16,
    value: i32,
    offset: usize,
) -> Result<(), DexExecutionError> {
    let register_count = registers.len();
    let target = registers
        .get_mut(usize::from(register))
        .ok_or(DexExecutionError::InvalidRegister {
            offset,
            register,
            register_count,
        })?;
    *target = value;
    Ok(())
}

fn apply_binary(
    op: IntegerBinaryOp,
    lhs: i32,
    rhs: i32,
    offset: usize,
) -> Result<i32, DexExecutionError> {
    match op {
        IntegerBinaryOp::Add => Ok(lhs.wrapping_add(rhs)),
        IntegerBinaryOp::Subtract => Ok(lhs.wrapping_sub(rhs)),
        IntegerBinaryOp::Multiply => Ok(lhs.wrapping_mul(rhs)),
        IntegerBinaryOp::Divide => {
            if rhs == 0 {
                Err(DexExecutionError::DivisionByZero { offset })
            } else if lhs == i32::MIN && rhs == -1 {
                Ok(i32::MIN)
            } else {
                Ok(lhs / rhs)
            }
        }
        IntegerBinaryOp::Remainder => {
            if rhs == 0 {
                Err(DexExecutionError::DivisionByZero { offset })
            } else if lhs == i32::MIN && rhs == -1 {
                Ok(0)
            } else {
                Ok(lhs % rhs)
            }
        }
    }
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

    #[test]
    fn decodes_signed_const4_values() {
        assert_eq!(
            decode_instruction(&[0xf012], 0).expect("const/4"),
            DecodedInstruction {
                offset: 0,
                width: 1,
                instruction: Instruction::Const { dest: 0, value: -1 },
            }
        );
    }

    #[test]
    fn reports_truncated_multi_unit_instruction() {
        assert_eq!(
            decode_instruction(&[0x0013], 0),
            Err(DexExecutionError::TruncatedInstruction {
                offset: 0,
                expected_units: 2,
                available_units: 1,
            })
        );
    }

    #[test]
    fn interprets_const16_and_return() {
        // const/16 v0, #42 ; return v0
        let method = code(1, 0, &[0x0013, 0x002a, 0x000f]);
        assert_eq!(execute_integer_method(&method, &[], 10), Ok(Some(42)));
    }

    #[test]
    fn interprets_integer_arguments_and_add_int() {
        // add-int v0, v1, v2 ; return v0
        let method = code(3, 2, &[0x0090, 0x0201, 0x000f]);
        assert_eq!(execute_integer_method(&method, &[4, 7], 10), Ok(Some(11)));
    }

    #[test]
    fn interprets_a_conditional_branch() {
        // const/4 v0, #0 ; if-eqz v0, +4 ; const/4 v0, #1 ; return v0
        // target: const/4 v0, #2 ; return v0
        let method = code(
            1,
            0,
            &[0x0012, 0x0038, 0x0004, 0x1012, 0x000f, 0x2012, 0x000f],
        );
        assert_eq!(execute_integer_method(&method, &[], 20), Ok(Some(2)));
    }

    #[test]
    fn rejects_branch_into_an_instruction_operand() {
        // goto/16 at code unit 0 has width 2; offset +1 points into its own operand.
        let method = code(0, 0, &[0x0029, 0x0001, 0x000e]);
        assert!(matches!(
            execute_integer_method(&method, &[], 10),
            Err(DexExecutionError::InvalidBranchTarget {
                offset: 0,
                target: 1,
                ..
            })
        ));
    }

    #[test]
    fn rejects_unsupported_opcodes() {
        let method = code(1, 0, &[0x001a, 0x000f]); // const-string
        assert!(matches!(
            execute_integer_method(&method, &[], 10),
            Err(DexExecutionError::UnsupportedOpcode { opcode: 0x1a, .. })
        ));
    }

    #[test]
    fn division_by_zero_returns_an_error_instead_of_panicking() {
        // div-int v0, v1, v2 ; return v0
        let method = code(3, 2, &[0x0093, 0x0201, 0x000f]);
        assert_eq!(
            execute_integer_method(&method, &[4, 0], 10),
            Err(DexExecutionError::DivisionByZero { offset: 0 })
        );
    }

    #[test]
    fn loop_stops_at_the_instruction_step_limit() {
        let method = code(0, 0, &[0x0028]); // goto +0
        assert_eq!(
            execute_integer_method(&method, &[], 25),
            Err(DexExecutionError::StepLimitExceeded { max_steps: 25 })
        );
    }

    #[test]
    fn rejects_try_catch_code_items() {
        let mut method = code(1, 0, &[0x000e]);
        method.tries_size = 1;
        assert_eq!(
            execute_integer_method(&method, &[], 10),
            Err(DexExecutionError::UnsupportedTryCatch)
        );
    }

    #[test]
    fn checks_incoming_argument_count() {
        let method = code(2, 1, &[0x000f]);
        assert_eq!(
            execute_integer_method(&method, &[], 10),
            Err(DexExecutionError::ArgumentCountMismatch {
                expected: 1,
                actual: 0,
            })
        );
    }

    #[test]
    fn rejects_invalid_registers() {
        let method = code(1, 0, &[0x1112, 0x000f]); // const/4 v1, #1
        assert!(matches!(
            execute_integer_method(&method, &[], 10),
            Err(DexExecutionError::InvalidRegister {
                register: 1,
                register_count: 1,
                ..
            })
        ));
    }

    #[test]
    fn rejects_code_items_above_the_hard_size_limit() {
        let method = code(
            0,
            0,
            &vec![0x000e; MAX_INTERPRETED_CODE_UNITS + 1],
        );
        assert_eq!(
            execute_integer_method(&method, &[], 10),
            Err(DexExecutionError::CodeTooLarge {
                code_units: MAX_INTERPRETED_CODE_UNITS + 1,
                max_code_units: MAX_INTERPRETED_CODE_UNITS,
            })
        );
    }

    #[test]
    fn return_void_has_no_integer_result() {
        let method = code(0, 0, &[0x000e]);
        assert_eq!(execute_integer_method(&method, &[], 10), Ok(None));
    }
}
