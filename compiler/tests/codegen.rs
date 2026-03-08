use axiom_runtime::abi;
use pysub_compiler::ir::{FunctionBody, ValueType};
use pysub_compiler::{compile_to_ir, compile_to_wasm};

#[test]
fn lowering_records_function_signatures_and_body() {
    let source =
        "fn main(sender: address, recipient: address, amount: u128, timestamp: u64, metadata: string, nonce: u64, signature: bytes, sender_pubkey: bytes, memo: string) -> u128:\n    return 0\n\nfn add(a: u128, b: u128) -> u128:\n    return a + b\n";
    let ir_module = compile_to_ir(source).expect("IR lowering");
    assert_eq!(ir_module.functions.len(), 2);

    let function = ir_module
        .functions
        .iter()
        .find(|function| function.name == "add")
        .expect("add function present");
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
    let source = "fn main(sender: address, recipient: address, amount: u128, timestamp: u64, metadata: string, nonce: u64, signature: bytes, sender_pubkey: bytes, memo: string) -> u128:\n    return 0\n\ncontract Wallet(owner: address):\n    storage balance: u128 = 0\n\n    fn balance() -> u128:\n        return 0\n";
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
        "fn main(sender: address, recipient: address, amount: u128, timestamp: u64, metadata: string, nonce: u64, signature: bytes, sender_pubkey: bytes, memo: string) -> u128:\n    return 0\n\nfn ping():\n    return\n\ncontract Wallet:\n    fn balance() -> u128:\n        return 0\n";
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
    assert!(exports.contains(&abi::LEGACY_ENTRY_EXPORT.to_string()));
    assert!(exports.contains(&abi::ENTRY_EXPORT.to_string()));
    assert!(exports.contains(&abi::contract_export("Wallet", "balance")));
}

#[test]
fn wasm_contains_add_instructions() {
    let source =
        "fn main(sender: address, recipient: address, amount: u128, timestamp: u64, metadata: string, nonce: u64, signature: bytes, sender_pubkey: bytes, memo: string) -> u128:\n    return 0\n\nfn add(a: u128, b: u128) -> u128:\n    return a + b\n";
    let wasm = compile_to_wasm(source).expect("emit wasm");
    let bodies = decode_function_ops(&wasm);
    assert_eq!(bodies.len(), 2);
    let add_body = bodies
        .into_iter()
        .find(|body| body.contains(&"i64.add".to_string()))
        .expect("add body present");
    assert_eq!(
        add_body,
        vec!["local.get 0", "local.get 1", "i64.add", "return", "end"],
        "unexpected instruction sequence: {:?}",
        add_body
    );
}

#[test]
fn wasm_handles_constant_returns() {
    let source = "fn main(sender: address, recipient: address, amount: u128, timestamp: u64, metadata: string, nonce: u64, signature: bytes, sender_pubkey: bytes, memo: string) -> u128:\n    return 0\n\nfn always_true() -> bool:\n    return true\n";
    let wasm = compile_to_wasm(source).expect("emit wasm");
    let bodies = decode_function_ops(&wasm);
    assert_eq!(bodies.len(), 2);
    let truthy_body = bodies
        .into_iter()
        .find(|body| body.contains(&"i32.const 1".to_string()))
        .expect("always_true body present");
    assert_eq!(truthy_body, vec!["i32.const 1", "return", "end"]);
}

#[test]
fn wasm_imports_system_helpers() {
    let source = "fn main(sender: address, recipient: address, amount: u128, timestamp: u64, metadata: string, nonce: u64, signature: bytes, sender_pubkey: bytes, memo: string) -> u128:\n    return get_gas_remaining()\n";
    let wasm = compile_to_wasm(source).expect("emit wasm");

    let mut imports = Vec::new();
    let parser = wasmparser::Parser::new(0);
    for payload in parser.parse_all(&wasm) {
        match payload.expect("payload") {
            wasmparser::Payload::ImportSection(section) => {
                for import in section {
                    let import = import.expect("import");
                    imports.push((import.module.to_string(), import.name.to_string()));
                }
            }
            wasmparser::Payload::End(_) => break,
            _ => {}
        }
    }

    assert!(imports.contains(&("axiom_system".to_string(), "get_gas_remaining".to_string())));

    let bodies = decode_function_ops(&wasm);
    assert_eq!(bodies.len(), 1);
    assert_eq!(bodies[0], vec!["call 0", "return", "end"]);
}

#[test]
fn wasm_imports_memory_helpers() {
    let source = "fn main(sender: address, recipient: address, amount: u128, timestamp: u64, metadata: string, nonce: u64, signature: bytes, sender_pubkey: bytes, memo: string) -> u128:\n    return 0\n\nfn alloc() -> address:\n    return deterministic_malloc(16)\n";
    let wasm = compile_to_wasm(source).expect("emit wasm");

    let mut imports = Vec::new();
    let parser = wasmparser::Parser::new(0);
    for payload in parser.parse_all(&wasm) {
        match payload.expect("payload") {
            wasmparser::Payload::ImportSection(section) => {
                for import in section {
                    let import = import.expect("import");
                    imports.push((import.module.to_string(), import.name.to_string()));
                }
            }
            wasmparser::Payload::End(_) => break,
            _ => {}
        }
    }

    assert!(imports.contains(&(
        "axiom_memory".to_string(),
        "deterministic_malloc".to_string()
    )));

    let bodies = decode_function_ops(&wasm);
    assert_eq!(bodies.len(), 2);
    let alloc_body = bodies
        .into_iter()
        .find(|body| body.contains(&"i32.const 16".to_string()))
        .expect("alloc body present");
    assert_eq!(alloc_body, vec!["i32.const 16", "call 0", "return", "end"]);
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
                        Operator::Call { function_index } => {
                            ops.push(format!("call {}", function_index));
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
