use std::path::PathBuf;

use pysub_compiler::ast::build_program;
use pysub_compiler::parser::parse_program;
use pysub_compiler::semantics::{validate_program, SemanticError};

fn read_example(relative_path: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("examples")
        .join(relative_path);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("failed to read {}: {err}", path.display()))
}

fn parse_and_build(source: &str) -> pysub_compiler::ast::Program {
    let parsed = parse_program(source).expect("parse program");
    build_program(&parsed).expect("build AST")
}

#[test]
fn validates_token_example() {
    let source = read_example("token.pysub");
    let program = parse_and_build(&source);
    validate_program(&program).expect("token example should be semantically valid");
}

#[test]
fn rejects_duplicate_storage_field() {
    let source = r#"
contract Dup:
    storage value: u128 = 0
    storage value: u128 = 1
"#;

    let program = parse_and_build(source);
    let err = validate_program(&program).expect_err("expected duplicate storage error");
    matches!(err, SemanticError::DuplicateStorage { .. })
        .then_some(())
        .expect("duplicate storage detected");
}

#[test]
fn rejects_invalid_map_key() {
    let source = r#"
contract BadMap:
    storage ledger: map[u128, u128]
"#;

    let program = parse_and_build(source);
    let err = validate_program(&program).expect_err("expected invalid map key error");
    matches!(err, SemanticError::InvalidMapKeyType { .. })
        .then_some(())
        .expect("invalid map key detected");
}

#[test]
fn rejects_duplicate_parameters() {
    let source = r#"
fn clash(a: u128, a: bool):
    return 0
"#;

    let program = parse_and_build(source);
    let err = validate_program(&program).expect_err("expected duplicate parameter error");
    matches!(err, SemanticError::DuplicateParameter { .. })
        .then_some(())
        .expect("duplicate parameter detected");
}
