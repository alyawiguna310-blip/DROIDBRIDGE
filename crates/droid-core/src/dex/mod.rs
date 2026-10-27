//! DEX file parsing.
//!
//! DEX files come from untrusted APKs, so every read is bounds-checked and
//! every failure is a [`DexError`]; parsing never panics and never allocates
//! based on an unchecked count from the file.
//!
//! # Scope
//!
//! Implemented: header/table validation, strings, class data, code items, and a
//! limited instruction decoder/integer interpreter for move, const, 32-bit
//! arithmetic, returns, unconditional jumps, and integer conditional branches.
//! The interpreter has hard limits on method code size and executed steps.
//!
//! Not implemented: the SHA-1 signature is not verified; the map list, link
//! section, annotations, static values, debug info, try/catch handlers and
//! DEX versions other than `035`-`039` are not interpreted. Files that
//! declare a link section are rejected rather than half-loaded.
//!
//! The integer interpreter is deliberately incomplete: it does not implement
//! ART, method invocation, references, arrays, fields, exceptions, native calls,
//! or Android framework services, and cannot run real Android apps by itself.

mod error;
mod file;
mod interpreter;
mod reader;
#[cfg(test)]
pub(crate) mod testutil;

pub use error::DexError;
pub use file::{
    ClassData, ClassDef, CodeItem, DexFile, DexHeader, DexLimits, EncodedField, EncodedMethod,
    FieldId, HEADER_SIZE, MethodId, MethodSignature, NO_INDEX, ProtoId,
};
pub use interpreter::{
    DecodedInstruction, DexExecutionError, IfCondition, Instruction, IntegerBinaryOp,
    MAX_INTERPRETED_CODE_UNITS, MAX_INTERPRETER_STEPS, decode_instruction, execute_integer_method,
};
