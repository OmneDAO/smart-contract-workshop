use std::path::PathBuf;

use pysub_compiler::semantics::SemanticError;
use pysub_compiler::{compile_source, CompilerError};

fn read_example(relative_path: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("examples")
        .join(relative_path);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("failed to read {}: {err}", path.display()))
}

#[test]
fn validates_token_example() {
    let source = read_example("token.pysub");
    compile_source(&source).expect("token example should be semantically valid");
}

#[test]
fn rejects_duplicate_storage_field() {
    let source = r#"
fn main() -> u128:
    return 0

contract Dup:
    storage value: u128 = 0
    storage value: u128 = 1
"#;

    let err = compile_source(source).expect_err("expected duplicate storage error");
    assert!(matches!(
        err,
        CompilerError::Semantics(SemanticError::DuplicateStorage { .. })
    ));
}

#[test]
fn rejects_invalid_map_key() {
    let source = r#"
fn main() -> u128:
    return 0

contract BadMap:
    storage ledger: map[u128, u128]
"#;

    let err = compile_source(source).expect_err("expected invalid map key error");
    assert!(matches!(
        err,
        CompilerError::Semantics(SemanticError::InvalidMapKeyType { .. })
    ));
}

#[test]
fn rejects_duplicate_parameters() {
    let source = r#"
fn main() -> u128:
    return 0

fn clash(a: u128, a: bool):
    return 0
"#;

    let err = compile_source(source).expect_err("expected duplicate parameter error");
    assert!(matches!(
        err,
        CompilerError::Semantics(SemanticError::DuplicateParameter { .. })
    ));
}

#[test]
fn rejects_reserved_entry_name() {
    let source = r#"
fn axiom_entry_main() -> u128:
    return 0

fn main() -> u128:
    return 0
"#;

    let err = compile_source(source).expect_err("expected reserved entry name error");
    assert!(matches!(
        err,
        CompilerError::Semantics(SemanticError::ReservedFunctionName { .. })
    ));
}
