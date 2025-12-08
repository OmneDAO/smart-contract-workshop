//! Intermediate representation helpers for pysub compiler.

use crate::ast::Program;

/// Placeholder IR module type.
pub struct Module;

/// Lower an AST into the compiler IR.
///
/// This will be implemented alongside the IR design.
pub fn lower_to_ir(_program: &Program) -> Module {
    todo!("IR lowering not implemented yet")
}
