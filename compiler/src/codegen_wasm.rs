//! WebAssembly code generation for pysub compiler.

use std::collections::HashMap;

use axiom_runtime::abi;
use wasm_encoder::{
    BlockType, CodeSection, ConstExpr, DataSection, EntityType, ExportSection,
    Function as WasmFunction, FunctionSection, ImportSection, Instruction, MemArg, MemoryType,
    Module, TypeSection, ValType,
};

use crate::ir::{
    BinaryOp, Contract, Expr, Function as IrFunction, FunctionBody, HostFunction, Local,
    Module as IrModule, StoreWidth, Stmt, ValueType,
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
    let mut data_section = DataSection::new();

    let mut next_type_index: u32 = 0;

    let needs_memory = module_needs_memory(ir_module);
    if needs_memory {
        let max_end = ir_module
            .data_segments
            .iter()
            .map(|segment| segment.offset as usize + segment.bytes.len())
            .max()
            .unwrap_or(0);
        let minimum_pages = if max_end == 0 {
            1
        } else {
            ((max_end + 65535) / 65536) as u64
        };
        import_section.import(
            "env",
            "memory",
            EntityType::Memory(MemoryType {
                minimum: minimum_pages,
                maximum: None,
                memory64: false,
                shared: false,
            }),
        );
        for segment in &ir_module.data_segments {
            data_section.active(
                0,
                &ConstExpr::i32_const(segment.offset as i32),
                segment.bytes.clone(),
            );
        }
    }

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
    if !host_functions.is_empty() || needs_memory {
        wasm.section(&import_section);
    }
    wasm.section(&function_section);
    wasm.section(&export_section);
    wasm.section(&code_section);
    if needs_memory && !ir_module.data_segments.is_empty() {
        wasm.section(&data_section);
    }

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
        FunctionBody::Block { locals, body: stmts } => {
            let mut locals_builder = Vec::new();
            for local in locals {
                locals_builder.push((1, convert_type(local.ty)));
            }
            *body = WasmFunction::new(locals_builder);
            let mut loop_stack = Vec::new();
            emit_statements(
                function,
                locals,
                stmts,
                body,
                host_function_indices,
                &mut loop_stack,
            );
            body.instruction(&Instruction::Return);
        }
    }
}

fn emit_statements(
    function: &IrFunction,
    locals: &[Local],
    stmts: &[Stmt],
    body: &mut WasmFunction,
    host_function_indices: &HashMap<HostFunction, u32>,
    loop_stack: &mut Vec<LoopLabels>,
) {
    for stmt in stmts {
        emit_statement(function, locals, stmt, body, host_function_indices, loop_stack);
    }
}

fn emit_statement(
    function: &IrFunction,
    locals: &[Local],
    stmt: &Stmt,
    body: &mut WasmFunction,
    host_function_indices: &HashMap<HostFunction, u32>,
    loop_stack: &mut Vec<LoopLabels>,
) {
    match stmt {
        Stmt::Let { local, value } | Stmt::Assign { local, value } => {
            emit_expr(value, body, host_function_indices);
            body.instruction(&Instruction::LocalSet(*local));
        }
        Stmt::Store { address, value, width } => {
            emit_expr(address, body, host_function_indices);
            emit_expr(value, body, host_function_indices);
            match width {
                StoreWidth::I8 => body.instruction(&Instruction::I32Store8(MemArg {
                    offset: 0,
                    align: 0,
                    memory_index: 0,
                })),
                StoreWidth::I16 => body.instruction(&Instruction::I32Store16(MemArg {
                    offset: 0,
                    align: 1,
                    memory_index: 0,
                })),
                StoreWidth::I32 => body.instruction(&Instruction::I32Store(MemArg {
                    offset: 0,
                    align: 2,
                    memory_index: 0,
                })),
                StoreWidth::I64 => body.instruction(&Instruction::I64Store(MemArg {
                    offset: 0,
                    align: 3,
                    memory_index: 0,
                })),
            };
        }
        Stmt::Expr(expr) => {
            emit_expr(expr, body, host_function_indices);
            body.instruction(&Instruction::Drop);
        }
        Stmt::Return { value } => {
            if let Some(expr) = value {
                emit_expr(expr, body, host_function_indices);
            }
            body.instruction(&Instruction::Return);
        }
        Stmt::If {
            condition,
            then_body,
            else_body,
        } => {
            emit_expr(condition, body, host_function_indices);
            emit_truthy_i32(condition, body);
            body.instruction(&Instruction::If(BlockType::Empty));
            emit_statements(function, locals, then_body, body, host_function_indices, loop_stack);
            if !else_body.is_empty() {
                body.instruction(&Instruction::Else);
                emit_statements(
                    function,
                    locals,
                    else_body,
                    body,
                    host_function_indices,
                    loop_stack,
                );
            }
            body.instruction(&Instruction::End);
        }
        Stmt::While { condition, body: loop_body } => {
            body.instruction(&Instruction::Block(BlockType::Empty));
            body.instruction(&Instruction::Loop(BlockType::Empty));
            loop_stack.push(LoopLabels {
                break_depth: 1,
                continue_depth: 0,
            });
            emit_expr(condition, body, host_function_indices);
            emit_zero_check_i32(condition, body);
            body.instruction(&Instruction::BrIf(1));
            emit_statements(function, locals, loop_body, body, host_function_indices, loop_stack);
            body.instruction(&Instruction::Br(0));
            loop_stack.pop();
            body.instruction(&Instruction::End);
            body.instruction(&Instruction::End);
        }
        Stmt::Break => {
            if let Some(labels) = loop_stack.last() {
                body.instruction(&Instruction::Br(labels.break_depth));
            }
        }
        Stmt::Continue => {
            if let Some(labels) = loop_stack.last() {
                body.instruction(&Instruction::Br(labels.continue_depth));
            }
        }
    }
}

fn emit_truthy_i32(condition: &Expr, body: &mut WasmFunction) {
    match condition.value_type() {
        ValueType::I32 => {
            body.instruction(&Instruction::I32Const(0));
            body.instruction(&Instruction::I32Ne);
        }
        ValueType::I64 => {
            body.instruction(&Instruction::I64Const(0));
            body.instruction(&Instruction::I64Ne);
        }
    }
}

fn emit_zero_check_i32(condition: &Expr, body: &mut WasmFunction) {
    match condition.value_type() {
        ValueType::I32 => {
            body.instruction(&Instruction::I32Eqz);
        }
        ValueType::I64 => {
            body.instruction(&Instruction::I64Eqz);
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct LoopLabels {
    break_depth: u32,
    continue_depth: u32,
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
        Expr::Local { index, .. } => {
            body.instruction(&Instruction::LocalGet(*index));
        }
        Expr::ConstI32(value) => {
            body.instruction(&Instruction::I32Const(*value));
        }
        Expr::ConstI64(value) => {
            body.instruction(&Instruction::I64Const(*value));
        }
        Expr::LoadI32 { address } => {
            emit_expr(address, body, host_function_indices);
            body.instruction(&Instruction::I32Load(MemArg {
                offset: 0,
                align: 2,
                memory_index: 0,
            }));
        }
        Expr::LoadI8 { address } => {
            emit_expr(address, body, host_function_indices);
            body.instruction(&Instruction::I32Load8U(MemArg {
                offset: 0,
                align: 0,
                memory_index: 0,
            }));
        }
        Expr::LoadI64 { address } => {
            emit_expr(address, body, host_function_indices);
            body.instruction(&Instruction::I64Load(MemArg {
                offset: 0,
                align: 3,
                memory_index: 0,
            }));
        }
        Expr::StateRead {
            key_ptr,
            key_len,
            out_len_ptr,
            ty,
        } => {
            let host = HostFunction::StdStateRead;
            let index = host_function_indices
                .get(&host)
                .copied()
                .expect("state_read must be registered in import section");
            body.instruction(&Instruction::I32Const(*key_ptr as i32));
            body.instruction(&Instruction::I32Const(*key_len as i32));
            body.instruction(&Instruction::I32Const(*out_len_ptr as i32));
            body.instruction(&Instruction::Call(index));

            match ty {
                ValueType::I32 => body.instruction(&Instruction::I32Load(MemArg {
                    offset: 0,
                    align: 2,
                    memory_index: 0,
                })),
                ValueType::I64 => body.instruction(&Instruction::I64Load(MemArg {
                    offset: 0,
                    align: 3,
                    memory_index: 0,
                })),
            };
        }
        Expr::StateReadRaw {
            key_ptr,
            key_len,
            out_len_ptr,
        } => {
            let host = HostFunction::StdStateRead;
            let index = host_function_indices
                .get(&host)
                .copied()
                .expect("state_read must be registered in import section");
            body.instruction(&Instruction::I32Const(*key_ptr as i32));
            body.instruction(&Instruction::I32Const(*key_len as i32));
            body.instruction(&Instruction::I32Const(*out_len_ptr as i32));
            body.instruction(&Instruction::Call(index));
        }
        Expr::Binary {
            op,
            left,
            right,
            ty,
        } => {
            emit_expr(left, body, host_function_indices);
            emit_expr(right, body, host_function_indices);
            let operand_type = left.value_type();
            match (op, ty, operand_type) {
                (BinaryOp::Add, ValueType::I32, _) => body.instruction(&Instruction::I32Add),
                (BinaryOp::Add, ValueType::I64, _) => body.instruction(&Instruction::I64Add),
                (BinaryOp::Sub, ValueType::I32, _) => body.instruction(&Instruction::I32Sub),
                (BinaryOp::Sub, ValueType::I64, _) => body.instruction(&Instruction::I64Sub),
                (BinaryOp::Mul, ValueType::I32, _) => body.instruction(&Instruction::I32Mul),
                (BinaryOp::Mul, ValueType::I64, _) => body.instruction(&Instruction::I64Mul),
                (BinaryOp::DivUInt, ValueType::I32, _) => body.instruction(&Instruction::I32DivU),
                (BinaryOp::DivUInt, ValueType::I64, _) => body.instruction(&Instruction::I64DivU),
                (BinaryOp::RemUInt, ValueType::I32, _) => body.instruction(&Instruction::I32RemU),
                (BinaryOp::RemUInt, ValueType::I64, _) => body.instruction(&Instruction::I64RemU),
                (BinaryOp::Equal, _, ValueType::I32) => body.instruction(&Instruction::I32Eq),
                (BinaryOp::Equal, _, ValueType::I64) => body.instruction(&Instruction::I64Eq),
                (BinaryOp::NotEqual, _, ValueType::I32) => body.instruction(&Instruction::I32Ne),
                (BinaryOp::NotEqual, _, ValueType::I64) => body.instruction(&Instruction::I64Ne),
                (BinaryOp::Less, _, ValueType::I32) => body.instruction(&Instruction::I32LtS),
                (BinaryOp::Less, _, ValueType::I64) => body.instruction(&Instruction::I64LtS),
                (BinaryOp::LessEqual, _, ValueType::I32) => body.instruction(&Instruction::I32LeS),
                (BinaryOp::LessEqual, _, ValueType::I64) => body.instruction(&Instruction::I64LeS),
                (BinaryOp::Greater, _, ValueType::I32) => body.instruction(&Instruction::I32GtS),
                (BinaryOp::Greater, _, ValueType::I64) => body.instruction(&Instruction::I64GtS),
                (BinaryOp::GreaterEqual, _, ValueType::I32) => body.instruction(&Instruction::I32GeS),
                (BinaryOp::GreaterEqual, _, ValueType::I64) => body.instruction(&Instruction::I64GeS),
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
        Expr::Select {
            condition,
            if_true,
            if_false,
            ..
        } => {
            emit_expr(if_true, body, host_function_indices);
            emit_expr(if_false, body, host_function_indices);
            emit_expr(condition, body, host_function_indices);
            emit_truthy_i32(condition, body);
            body.instruction(&Instruction::Select);
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

fn module_needs_memory(module: &IrModule) -> bool {
    if !module.data_segments.is_empty() {
        return true;
    }

    for function in &module.functions {
        if function_needs_memory(function) {
            return true;
        }
    }
    for contract in &module.contracts {
        for function in &contract.functions {
            if function_needs_memory(function) {
                return true;
            }
        }
    }

    false
}

fn function_needs_memory(function: &IrFunction) -> bool {
    match &function.body {
        FunctionBody::Return { value } => value
            .as_ref()
            .map(expr_needs_memory)
            .unwrap_or(false),
        FunctionBody::Block { body, .. } => body.iter().any(stmt_needs_memory),
    }
}

fn stmt_needs_memory(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::Store { .. } => true,
        Stmt::Let { value, .. } | Stmt::Assign { value, .. } | Stmt::Expr(value) => {
            expr_needs_memory(value)
        }
        Stmt::Return { value } => value.as_ref().map(expr_needs_memory).unwrap_or(false),
        Stmt::If {
            condition,
            then_body,
            else_body,
        } => {
            expr_needs_memory(condition)
                || then_body.iter().any(stmt_needs_memory)
                || else_body.iter().any(stmt_needs_memory)
        }
        Stmt::While { condition, body } => {
            expr_needs_memory(condition) || body.iter().any(stmt_needs_memory)
        }
        Stmt::Break | Stmt::Continue => false,
    }
}

fn expr_needs_memory(expr: &Expr) -> bool {
    match expr {
        Expr::LoadI32 { .. } => true,
        Expr::LoadI8 { .. } => true,
        Expr::LoadI64 { .. } => true,
        Expr::StateRead { .. } => true,
        Expr::StateReadRaw { .. } => true,
        Expr::Binary { left, right, .. } => expr_needs_memory(left) || expr_needs_memory(right),
        Expr::HostCall { args, .. } => args.iter().any(expr_needs_memory),
        Expr::Select {
            condition,
            if_true,
            if_false,
            ..
        } => {
            expr_needs_memory(condition)
                || expr_needs_memory(if_true)
                || expr_needs_memory(if_false)
        }
        Expr::Param { .. } | Expr::Local { .. } | Expr::ConstI32(_) | Expr::ConstI64(_) => false,
    }
}
