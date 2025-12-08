pub mod ast;
pub mod codegen_wasm;
pub mod ir;
pub mod parser;
pub mod semantics;

use std::path::{Path, PathBuf};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum CompilerError {
    #[error("parse error: {0}")]
    Parse(#[from] parser::ParseError),

    #[error("ast error: {0}")]
    Ast(#[from] ast::AstError),

    #[error("semantic error: {0}")]
    Semantics(#[from] semantics::SemanticError),

    #[error("ir lowering error: {0}")]
    Ir(#[from] ir::IrError),

    #[error("failed to read `{path}`: {source}")]
    Io {
        #[source]
        source: std::io::Error,
        path: PathBuf,
    },
}

/// Parse, build the AST, and run semantic validation for the provided source code.
pub fn compile_source(source: &str) -> Result<ast::Program, CompilerError> {
    let parsed = parser::parse_program(source)?;
    let program = ast::build_program(&parsed)?;
    semantics::validate_program(&program)?;
    Ok(program)
}

/// Compile a pysub source file. Returns the AST when successful.
pub fn compile_file(path: impl AsRef<Path>) -> Result<ast::Program, CompilerError> {
    let path_ref = path.as_ref();
    let source = std::fs::read_to_string(path_ref).map_err(|source| CompilerError::Io {
        source,
        path: path_ref.to_path_buf(),
    })?;
    compile_source(&source)
}

/// Compile pysub source to the intermediate representation.
///
/// This currently returns a placeholder IR module until lowering is implemented.
pub fn compile_to_ir(source: &str) -> Result<ir::Module, CompilerError> {
    let program = compile_source(source)?;
    Ok(ir::lower_to_ir(&program)?)
}

/// Compile pysub source directly to WebAssembly bytes.
///
/// Code generation is not yet implemented; this function exists to wire future
/// stages into the existing compile helpers.
pub fn compile_to_wasm(source: &str) -> Result<Vec<u8>, CompilerError> {
    let module = compile_to_ir(source)?;
    Ok(codegen_wasm::emit_wasm(&module))
}
