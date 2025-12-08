//! WebAssembly code generation for pysub compiler.

use wasm_encoder::{
    CodeSection, ExportSection, Function as WasmFunction, FunctionSection, Instruction, Module,
    TypeSection, ValType,
};

use crate::ir::{
    BinaryOp, Contract, Expr, Function as IrFunction, FunctionBody, Module as IrModule, ValueType,
};

/// Emit WebAssembly bytes from an IR module.
pub fn emit_wasm(ir_module: &IrModule) -> Vec<u8> {
    let mut wasm = Module::new();

    let mut all_functions: Vec<(IrFunction, String)> = ir_module
        .functions
        .iter()
        .map(|function| (function.clone(), function.name.clone()))
        .collect();

    for contract in &ir_module.contracts {
        collect_contract_functions(contract, &mut all_functions);
    }

    if all_functions.is_empty() {
        return wasm.finish();
    }

    let mut type_section = TypeSection::new();
    let mut function_section = FunctionSection::new();
    let mut export_section = ExportSection::new();
    let mut code_section = CodeSection::new();

    for (func_index, (function, export_name)) in all_functions.iter().enumerate() {
        let params = function
            .params
            .iter()
            .map(|param| convert_type(param.ty))
            .collect::<Vec<_>>();
        let results = function
            .return_type
            .iter()
            .map(|ty| convert_type(*ty))
            .collect::<Vec<_>>();

        type_section.function(params.into_iter(), results.into_iter());
        function_section.function(func_index as u32);
        export_section.export(
            export_name,
            wasm_encoder::ExportKind::Func,
            func_index as u32,
        );

        let mut body = WasmFunction::new(Vec::new());
        emit_function_body(function, &mut body);
        body.instruction(&Instruction::End);
        code_section.function(&body);
    }

    wasm.section(&type_section);
    wasm.section(&function_section);
    wasm.section(&export_section);
    wasm.section(&code_section);

    wasm.finish()
}

fn collect_contract_functions(contract: &Contract, output: &mut Vec<(IrFunction, String)>) {
    for function in &contract.functions {
        let export_name = format!("{}::{}", contract.name, function.name);
        output.push((function.clone(), export_name));
    }
}

fn emit_function_body(function: &IrFunction, body: &mut WasmFunction) {
    match &function.body {
        FunctionBody::Return { value } => {
            if let Some(expr) = value {
                emit_expr(expr, body);
            }
            body.instruction(&Instruction::Return);
        }
    }
}

fn emit_expr(expr: &Expr, body: &mut WasmFunction) {
    match expr {
        Expr::Param { index, .. } => {
            body.instruction(&Instruction::LocalGet(*index));
        }
        Expr::ConstI32(value) => {
            body.instruction(&Instruction::I32Const(*value));
        }
        Expr::ConstI64(value) => {
            body.instruction(&Instruction::I64Const(*value));
        }
        Expr::Binary {
            op,
            left,
            right,
            ty,
        } => {
            emit_expr(left, body);
            emit_expr(right, body);
            match (op, ty) {
                (BinaryOp::Add, ValueType::I32) => body.instruction(&Instruction::I32Add),
                (BinaryOp::Add, ValueType::I64) => body.instruction(&Instruction::I64Add),
                (BinaryOp::Sub, ValueType::I32) => body.instruction(&Instruction::I32Sub),
                (BinaryOp::Sub, ValueType::I64) => body.instruction(&Instruction::I64Sub),
                (BinaryOp::Mul, ValueType::I32) => body.instruction(&Instruction::I32Mul),
                (BinaryOp::Mul, ValueType::I64) => body.instruction(&Instruction::I64Mul),
                (BinaryOp::DivUInt, ValueType::I32) => body.instruction(&Instruction::I32DivU),
                (BinaryOp::DivUInt, ValueType::I64) => body.instruction(&Instruction::I64DivU),
                (BinaryOp::RemUInt, ValueType::I32) => body.instruction(&Instruction::I32RemU),
                (BinaryOp::RemUInt, ValueType::I64) => body.instruction(&Instruction::I64RemU),
            };
        }
    };
}

fn convert_type(value_type: ValueType) -> ValType {
    match value_type {
        ValueType::I32 => ValType::I32,
        ValueType::I64 => ValType::I64,
    }
}
