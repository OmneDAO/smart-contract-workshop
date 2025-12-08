use std::path::PathBuf;

use pysub_compiler::parser::{parse_program, ParseError};

fn read_example(relative_path: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("examples")
        .join(relative_path);

    std::fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("failed to read {}: {err}", path.display()))
}

#[test]
fn parses_hello_example() {
    let source = read_example("hello.pysub");
    parse_program(&source).expect("hello.pysub should parse successfully");
}

#[test]
fn parses_token_example() {
    let source = read_example("token.pysub");
    parse_program(&source).expect("token.pysub should parse successfully");
}

#[test]
fn detects_bad_indentation() {
    let source = "fn broken():\n  return 0\n";
    let err = parse_program(source).unwrap_err();
    assert!(matches!(err, ParseError::Indentation(_)));
}
