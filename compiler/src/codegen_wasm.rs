//! WebAssembly code generation for pysub compiler.

use crate::ir::Module;

/// Emit WebAssembly bytes from an IR module.
///
/// This will be implemented once IR lowering is available.
pub fn emit_wasm(_module: &Module) -> Vec<u8> {
    todo!("WASM code generation not implemented yet")
}
