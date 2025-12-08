use pysub_compiler::ir::{FunctionBody, ValueType};
use pysub_compiler::{compile_to_ir, compile_to_wasm};

#[test]
fn lowering_records_function_signatures_and_body() {
    let source = "fn add(a: u128, b: u128) -> u128:\n    return a + b\n";
    let ir_module = compile_to_ir(source).expect("IR lowering");
    assert_eq!(ir_module.functions.len(), 1);

    let function = &ir_module.functions[0];
    assert_eq!(function.name, "add");
    assert_eq!(function.params.len(), 2);
    assert_eq!(function.params[0].ty, ValueType::I64);
    assert_eq!(function.params[1].ty, ValueType::I64);
    assert_eq!(function.return_type, Some(ValueType::I64));
    matches!(&function.body, FunctionBody::Return { value: Some(_) })
        .then_some(())
        .expect("return expression captured");
}

#[test]
fn lowering_handles_contract_members() {
    let source = "contract Wallet(owner: address):\n    storage balance: u128 = 0\n\n    fn balance() -> u128:\n        return 0\n";
    let ir_module = compile_to_ir(source).expect("IR lowering");
    assert_eq!(ir_module.contracts.len(), 1);

    let contract = &ir_module.contracts[0];
    assert_eq!(contract.name, "Wallet");
    assert_eq!(contract.params.len(), 1);
    assert_eq!(contract.params[0].name, "owner");
    assert_eq!(contract.params[0].ty, ValueType::I32);
    assert_eq!(contract.storage.len(), 1);
    assert_eq!(contract.storage[0].name, "balance");
    assert_eq!(contract.storage[0].ty, ValueType::I64);
    assert_eq!(contract.functions.len(), 1);
    assert_eq!(contract.functions[0].name, "balance");
    assert_eq!(contract.functions[0].return_type, Some(ValueType::I64));
}

#[test]
fn wasm_exports_all_functions() {
    let source =
        "fn ping():\n    return\n\ncontract Wallet:\n    fn balance() -> u128:\n        return 0\n";
    let wasm = compile_to_wasm(source).expect("WASM emission");
    assert!(wasm.starts_with(b"\0asm"));

    let mut exports = Vec::new();
    let parser = wasmparser::Parser::new(0);
    for payload in parser.parse_all(&wasm) {
        match payload.expect("payload") {
            wasmparser::Payload::ExportSection(section) => {
                for export in section {
                    let export = export.expect("export");
                    if export.kind == wasmparser::ExternalKind::Func {
                        exports.push(export.name.to_string());
                    }
                }
            }
            wasmparser::Payload::End(_) => break,
            _ => {}
        }
    }

    assert!(exports.contains(&"ping".to_string()));
    assert!(exports.contains(&"Wallet::balance".to_string()));
}

#[test]
fn wasm_contains_add_instructions() {
    let source = "fn add(a: u128, b: u128) -> u128:\n    return a + b\n";
    let wasm = compile_to_wasm(source).expect("emit wasm");
    let bodies = decode_function_ops(&wasm);
    assert_eq!(bodies.len(), 1);
    assert_eq!(
        bodies[0],
        vec!["local.get 0", "local.get 1", "i64.add", "return", "end"],
        "unexpected instruction sequence: {:?}",
        bodies[0]
    );
}

#[test]
fn wasm_handles_constant_returns() {
    let source = "fn always_true() -> bool:\n    return true\n";
    let wasm = compile_to_wasm(source).expect("emit wasm");
    let bodies = decode_function_ops(&wasm);
    assert_eq!(bodies.len(), 1);
    assert_eq!(bodies[0], vec!["i32.const 1", "return", "end"]);
}

fn decode_function_ops(wasm: &[u8]) -> Vec<Vec<String>> {
    let mut bodies = Vec::new();
    let parser = wasmparser::Parser::new(0);
    for payload in parser.parse_all(wasm) {
        match payload.expect("payload") {
            wasmparser::Payload::CodeSectionEntry(body) => {
                let mut ops = Vec::new();
                let mut reader = body.get_operators_reader().expect("operators");
                while !reader.eof() {
                    use wasmparser::Operator;
                    let op = reader.read().expect("operator");
                    match op {
                        Operator::LocalGet { local_index } => {
                            ops.push(format!("local.get {}", local_index));
                        }
                        Operator::I64Add => ops.push("i64.add".into()),
                        Operator::I64Sub => ops.push("i64.sub".into()),
                        Operator::I64Mul => ops.push("i64.mul".into()),
                        Operator::I64DivU => ops.push("i64.div_u".into()),
                        Operator::I64RemU => ops.push("i64.rem_u".into()),
                        Operator::I32Add => ops.push("i32.add".into()),
                        Operator::I32Sub => ops.push("i32.sub".into()),
                        Operator::I32Mul => ops.push("i32.mul".into()),
                        Operator::I32DivU => ops.push("i32.div_u".into()),
                        Operator::I32RemU => ops.push("i32.rem_u".into()),
                        Operator::I32Const { value } => {
                            ops.push(format!("i32.const {}", value));
                        }
                        Operator::I64Const { value } => {
                            ops.push(format!("i64.const {}", value));
                        }
                        Operator::Return => ops.push("return".into()),
                        Operator::End => ops.push("end".into()),
                        _ => ops.push(format!("{:?}", op)),
                    }
                }
                bodies.push(ops);
            }
            wasmparser::Payload::End(_) => break,
            _ => {}
        }
    }
    bodies
}
