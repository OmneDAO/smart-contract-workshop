//! WebAssembly code generation for pysub compiler.

use std::collections::HashMap;

use axiom_runtime::abi;
use wasm_encoder::{
    CodeSection, EntityType, ExportSection, Function as WasmFunction, FunctionSection,
    ImportSection, Instruction, Module, TypeSection, ValType,
};

use crate::ir::{
    BinaryOp, Contract, Expr, Function as IrFunction, FunctionBody, HostFunction,
    Module as IrModule, ValueType,
};

struct FunctionBinding {
    function: IrFunction,
    export_names: Vec<String>,
}

/// Emit WebAssembly bytes from an IR module.
pub fn emit_wasm(ir_module: &IrModule) -> Vec<u8> {
    let mut wasm = Module::new();

    let mut bindings: Vec<FunctionBinding> = ir_module
        .functions
        .iter()
        .map(|function| {
            let mut export_names = vec![function.name.clone()];
            if function.name == abi::LEGACY_ENTRY_EXPORT {
                export_names.push(abi::ENTRY_EXPORT.to_string());
            }
            FunctionBinding {
                function: function.clone(),
                export_names,
            }
        })
        .collect();

    for contract in &ir_module.contracts {
        collect_contract_functions(contract, &mut bindings);
    }

    if bindings.is_empty() {
        return wasm.finish();
    }

    let host_functions = ir_module.used_host_functions();
    let mut host_function_indices = HashMap::new();

    let mut type_section = TypeSection::new();
    let mut import_section = ImportSection::new();
    let mut function_section = FunctionSection::new();
    let mut export_section = ExportSection::new();
    let mut code_section = CodeSection::new();

    let mut next_type_index: u32 = 0;

    for host_function in &host_functions {
        let type_index = next_type_index;
        append_function_type(
            &mut type_section,
            host_function.params(),
            host_function.return_type(),
        );
        next_type_index += 1;

        import_section.import(
            host_function.module(),
            host_function.field(),
            EntityType::Function(type_index),
        );

        let function_index = host_function_indices.len() as u32;
        host_function_indices.insert(*host_function, function_index);
    }

    let imported_function_count = host_function_indices.len() as u32;

    let mut defined_type_indices = Vec::with_capacity(bindings.len());

    for binding in &bindings {
        let function = &binding.function;
        let type_index = next_type_index;
        let param_types: Vec<ValueType> = function.params.iter().map(|param| param.ty).collect();
        append_function_type(&mut type_section, &param_types, function.return_type);
        next_type_index += 1;
        defined_type_indices.push(type_index);
    }

    for type_index in &defined_type_indices {
        function_section.function(*type_index);
    }

    for (func_index, binding) in bindings.iter().enumerate() {
        let resolved_index = imported_function_count + func_index as u32;

        for export_name in &binding.export_names {
            export_section.export(export_name, wasm_encoder::ExportKind::Func, resolved_index);
        }

        let mut body = WasmFunction::new(Vec::new());
        emit_function_body(&binding.function, &mut body, &host_function_indices);
        body.instruction(&Instruction::End);
        code_section.function(&body);
    }

    wasm.section(&type_section);
    if !host_functions.is_empty() {
        wasm.section(&import_section);
    }
    wasm.section(&function_section);
    wasm.section(&export_section);
    wasm.section(&code_section);

    wasm.finish()
}

fn collect_contract_functions(contract: &Contract, output: &mut Vec<FunctionBinding>) {
    for function in &contract.functions {
        let export_name = format!("{}::{}", contract.name, function.name);
        let runtime_export = abi::contract_export(&contract.name, &function.name);
        let mut export_names = Vec::with_capacity(2);
        export_names.push(export_name);
        if !export_names.iter().any(|name| name == &runtime_export) {
            export_names.push(runtime_export);
        }
        output.push(FunctionBinding {
            function: function.clone(),
            export_names,
        });
    }
}

fn emit_function_body(
    function: &IrFunction,
    body: &mut WasmFunction,
    host_function_indices: &HashMap<HostFunction, u32>,
) {
    match &function.body {
        FunctionBody::Return { value } => {
            if let Some(expr) = value {
                emit_expr(expr, body, host_function_indices);
            }
            body.instruction(&Instruction::Return);
        }
    }
}

fn emit_expr(
    expr: &Expr,
    body: &mut WasmFunction,
    host_function_indices: &HashMap<HostFunction, u32>,
) {
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
            emit_expr(left, body, host_function_indices);
            emit_expr(right, body, host_function_indices);
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
        Expr::HostCall { function, args } => {
            for arg in args {
                emit_expr(arg, body, host_function_indices);
            }
            let index = host_function_indices
                .get(function)
                .copied()
                .expect("host function must be registered in import section");
            body.instruction(&Instruction::Call(index));
        }
    };
}

fn convert_type(value_type: ValueType) -> ValType {
    match value_type {
        ValueType::I32 => ValType::I32,
        ValueType::I64 => ValType::I64,
    }
}

fn append_function_type(
    type_section: &mut TypeSection,
    params: &[ValueType],
    result: Option<ValueType>,
) {
    let param_types = params.iter().map(|ty| convert_type(*ty));
    let result_types = result.into_iter().map(convert_type);
    type_section.function(param_types, result_types);
}
