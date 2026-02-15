use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use thiserror::Error;

use crate::dsl::{
    self, DslBinaryOp, DslCallArg, DslExpr, DslLiteral, DslParam, DslStmt, DslStruct,
    DslType, DslUnaryOp,
};
use crate::ir::{
    BinaryOp as IrBinaryOp, Contract as IrContract, DataSegment, Expr as IrExpr,
    Function as IrFunction, FunctionBody as IrFunctionBody, Local as IrLocal,
    Module as IrModule, Param as IrParam, Stmt as IrStmt, StoreWidth, ValueType,
};
use crate::serialize;

#[derive(Debug, Deserialize)]
pub struct Manifest {
    pub contract: String,
    pub version: String,
    #[serde(default)]
    pub entrypoint_prefix: Option<String>,
    #[serde(default)]
    pub modules: Vec<ManifestModule>,
    #[serde(default)]
    pub entrypoints: Vec<ManifestEntrypoint>,
    #[serde(default)]
    pub artifacts: Option<ManifestArtifacts>,
}

#[derive(Debug, Deserialize)]
pub struct ManifestModule {
    pub name: String,
    pub path: String,
}

#[derive(Debug, Deserialize)]
pub struct ManifestEntrypoint {
    pub name: String,
    pub module: String,
}

#[derive(Debug, Deserialize)]
pub struct ManifestArtifacts {
    #[serde(default)]
    pub wasm: Option<ManifestArtifact>,
}

#[derive(Debug, Deserialize)]
pub struct ManifestArtifact {
    pub path: String,
    #[serde(default)]
    pub checksum: Option<String>,
}

#[derive(Debug)]
pub struct ManifestSummary {
    pub contract: String,
    pub version: String,
    pub entrypoint_prefix: Option<String>,
    pub module_paths: Vec<PathBuf>,
    pub entrypoints: Vec<String>,
    pub wasm_artifact: Option<PathBuf>,
}

#[derive(Debug, Error)]
pub enum ManifestCompileError {
    #[error("failed to read `{path}`: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("manifest parse error: {0}")]
    Parse(#[from] serde_yaml::Error),

    #[error("module `{module}` not found in manifest")]
    MissingModule { module: String },

    #[error("entrypoint `{entrypoint}` not found in module `{module}`")]
    MissingEntrypoint { module: String, entrypoint: String },

    #[error("invalid function signature `{signature}` in module `{module}`: {reason}")]
    InvalidSignature {
        module: String,
        signature: String,
        reason: String,
    },

    #[error("unsupported type `{ty}` in module `{module}`")]
    UnsupportedType { module: String, ty: String },

    #[error("unsupported statement `{statement}` in module `{module}`")]
    UnsupportedStatement { module: String, statement: String },

    #[error("unsupported expression `{expression}` in module `{module}`")]
    UnsupportedExpression { module: String, expression: String },

    #[error("unknown identifier `{identifier}` in module `{module}`")]
    UnknownIdentifier { module: String, identifier: String },

    #[error("type mismatch in module `{module}`: expected `{expected}`, found `{found}`")]
    TypeMismatch {
        module: String,
        expected: ValueType,
        found: ValueType,
    },
}

pub fn load_manifest(path: &Path) -> Result<Manifest, ManifestCompileError> {
    let contents = std::fs::read_to_string(path).map_err(|source| ManifestCompileError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    serde_yaml::from_str(&contents).map_err(ManifestCompileError::from)
}

pub fn summarize_manifest(path: &Path, manifest: &Manifest) -> ManifestSummary {
    let base_dir = path.parent().unwrap_or(Path::new("."));
    let module_paths = manifest
        .modules
        .iter()
        .map(|module| base_dir.join(&module.path))
        .collect();

    let entrypoints = manifest
        .entrypoints
        .iter()
        .map(|entry| format!("{}::{}", entry.module, entry.name))
        .collect();

    let wasm_artifact = manifest
        .artifacts
        .as_ref()
        .and_then(|artifacts| artifacts.wasm.as_ref())
        .map(|artifact| base_dir.join(&artifact.path));

    ManifestSummary {
        contract: manifest.contract.clone(),
        version: manifest.version.clone(),
        entrypoint_prefix: manifest.entrypoint_prefix.clone(),
        module_paths,
        entrypoints,
        wasm_artifact,
    }
}

pub fn looks_like_manifest(source: &str) -> bool {
    let mut found_contract = false;
    let mut found_version = false;

    for line in source.lines().take(40) {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if trimmed.starts_with("contract:") {
            found_contract = true;
        }
        if trimmed.starts_with("version:") {
            found_version = true;
        }
        if found_contract && found_version {
            return true;
        }
    }

    false
}

pub fn compile_manifest_to_ir(path: &Path) -> Result<IrModule, ManifestCompileError> {
    let manifest = load_manifest(path)?;
    let base_dir = path.parent().unwrap_or(Path::new("."));

    let mut data_allocator = DataAllocator::new();
    let mut module_functions: HashMap<String, Vec<IrFunction>> = HashMap::new();
    let mut module_function_names: HashMap<String, Vec<String>> = HashMap::new();
    let mut entrypoint_lookup: HashMap<(String, String), ()> = HashMap::new();
    for entry in &manifest.entrypoints {
        entrypoint_lookup.insert((entry.module.clone(), entry.name.clone()), ());
    }

    for module in &manifest.modules {
        let module_path = base_dir.join(&module.path);
        let contents = std::fs::read_to_string(&module_path).map_err(|source| {
            ManifestCompileError::Io {
                path: module_path.clone(),
                source,
            }
        })?;
        let parsed = dsl::parse_module(&contents).map_err(|err| {
            ManifestCompileError::InvalidSignature {
                module: module.name.clone(),
                signature: module.name.clone(),
                reason: err.to_string(),
            }
        })?;

        module_function_names.insert(
            module.name.clone(),
            parsed.functions.iter().map(|func| func.name.clone()).collect(),
        );

        let functions =
            lower_dsl_module(&module.name, &parsed, &entrypoint_lookup, &mut data_allocator)?;
        module_functions.insert(module.name.clone(), functions);
    }

    for entry in &manifest.entrypoints {
        let names = module_function_names.get(&entry.module).ok_or_else(|| {
            ManifestCompileError::MissingModule {
                module: entry.module.clone(),
            }
        })?;
        if !names.iter().any(|name| name == &entry.name) {
            return Err(ManifestCompileError::MissingEntrypoint {
                module: entry.module.clone(),
                entrypoint: entry.name.clone(),
            });
        }
    }

    let contract_name = manifest
        .entrypoint_prefix
        .clone()
        .unwrap_or_else(|| manifest.contract.clone());

    let mut functions = Vec::new();
    for module in &manifest.modules {
        let module_functions = module_functions.get(&module.name).ok_or_else(|| {
            ManifestCompileError::MissingModule {
                module: module.name.clone(),
            }
        })?;
        functions.extend(module_functions.iter().cloned());
    }

    Ok(IrModule {
        contracts: vec![IrContract {
            name: contract_name,
            params: Vec::new(),
            storage: Vec::new(),
            functions,
        }],
        functions: Vec::new(),
        data_segments: data_allocator.finish(),
    })
}

fn lower_dsl_module(
    module_name: &str,
    module: &dsl::DslModule,
    entrypoints: &HashMap<(String, String), ()>,
    data_allocator: &mut DataAllocator,
) -> Result<Vec<IrFunction>, ManifestCompileError> {
    let mut state_fields = HashMap::new();
    for field in &module.state_fields {
        state_fields.insert(field.name.clone(), field.ty.clone());
    }
    let mut struct_defs = HashMap::new();
    for def in &module.structs {
        struct_defs.insert(def.name.clone(), def.clone());
    }
    let mut functions = Vec::new();
    for function in &module.functions {
        let is_entry = entrypoints.contains_key(&(module_name.to_string(), function.name.clone()));
        let name = if is_entry {
            function.name.clone()
        } else {
            format!("{}_{}", module_name, function.name)
        };
        functions.push(lower_function(
            module_name,
            function,
            &name,
            data_allocator,
            &state_fields,
            &struct_defs,
        )?);
    }
    Ok(functions)
}

fn lower_function(
    module: &str,
    function: &dsl::DslFunction,
    name: &str,
    data_allocator: &mut DataAllocator,
    state_fields: &HashMap<String, DslType>,
    structs: &HashMap<String, DslStruct>,
) -> Result<IrFunction, ManifestCompileError> {
    let params = lower_params(module, &function.params)?;
    let return_type = function
        .return_type
        .as_ref()
        .map(|ty| map_type(module, ty))
        .transpose()?;

    let mut locals = Vec::new();
    let mut local_map: HashMap<String, (u32, ValueType)> = HashMap::new();
    let mut param_map: HashMap<String, (u32, ValueType)> = HashMap::new();
    let mut local_dsl_types: HashMap<String, DslType> = HashMap::new();
    let mut param_dsl_types: HashMap<String, DslType> = HashMap::new();
    for (index, param) in params.iter().enumerate() {
        param_map.insert(param.name.clone(), (index as u32, param.ty));
        if let Some(dsl_param) = function.params.get(index) {
            if let Some(ty) = &dsl_param.ty {
                param_dsl_types.insert(dsl_param.name.clone(), ty.clone());
            }
        }
    }

    let mut body = Vec::new();
    for stmt in &function.body {
        lower_stmt(
            module,
            stmt,
            name,
            &param_map,
            &mut local_map,
            &param_dsl_types,
            &mut local_dsl_types,
            &mut locals,
            &mut body,
            data_allocator,
            return_type,
            state_fields,
            structs,
        )?;
    }

    Ok(IrFunction {
        name: name.to_string(),
        params,
        return_type,
        body: IrFunctionBody::Block { locals, body },
    })
}

fn lower_params(
    module: &str,
    params: &[DslParam],
) -> Result<Vec<IrParam>, ManifestCompileError> {
    let mut lowered = Vec::new();
    for param in params {
        let ty = param
            .ty
            .as_ref()
            .map(|ty| map_type(module, ty))
            .transpose()?;
        let ty = ty.unwrap_or(ValueType::I32);
        lowered.push(IrParam {
            name: param.name.clone(),
            ty,
        });
    }
    Ok(lowered)
}

fn lower_stmt(
    module: &str,
    stmt: &DslStmt,
    function: &str,
    params: &HashMap<String, (u32, ValueType)>,
    locals: &mut HashMap<String, (u32, ValueType)>,
    param_dsl_types: &HashMap<String, DslType>,
    local_dsl_types: &mut HashMap<String, DslType>,
    local_defs: &mut Vec<IrLocal>,
    output: &mut Vec<IrStmt>,
    data_allocator: &mut DataAllocator,
    return_type: Option<ValueType>,
    state_fields: &HashMap<String, DslType>,
    structs: &HashMap<String, DslStruct>,
) -> Result<(), ManifestCompileError> {
    match stmt {
        DslStmt::Let { name, ty, value } => {
            let expected_dsl = ty.clone();
            let (mut stmts, expr, inferred_dsl) = lower_expr_with_side_effects(
                module,
                function,
                value,
                expected_dsl.clone(),
                params,
                locals,
                param_dsl_types,
                local_dsl_types,
                local_defs,
                data_allocator,
                state_fields,
                structs,
            )?;
            let inferred = expr.value_type();
            let declared = ty
                .as_ref()
                .map(|ty| map_type(module, ty))
                .transpose()?;
            if let Some(declared) = declared {
                if declared != inferred {
                    return Err(ManifestCompileError::TypeMismatch {
                        module: module.to_string(),
                        expected: declared,
                        found: inferred,
                    });
                }
                local_dsl_types.insert(name.clone(), ty.clone().unwrap());
            } else if let Some(dsl_type) = inferred_dsl.or(expected_dsl) {
                local_dsl_types.insert(name.clone(), dsl_type);
            }
            let index = (params.len() + local_defs.len()) as u32;
            local_defs.push(IrLocal {
                name: name.clone(),
                ty: declared.unwrap_or(inferred),
            });
            locals.insert(name.clone(), (index, declared.unwrap_or(inferred)));
            output.append(&mut stmts);
            output.push(IrStmt::Let { local: index, value: expr });
        }
        DslStmt::Assign { target, value } => {
            let target_name = match target {
                DslExpr::Identifier(name) => name,
                other => {
                    return Err(ManifestCompileError::UnsupportedStatement {
                        module: module.to_string(),
                        statement: format!("assign to {:?}", other),
                    })
                }
            };
            if let Some((index, ty)) = locals.get(target_name).copied() {
                let expected_dsl = local_dsl_types.get(target_name).cloned();
                let (mut stmts, expr, inferred_dsl) = lower_expr_with_side_effects(
                    module,
                    function,
                    value,
                    expected_dsl.clone(),
                    params,
                    locals,
                    param_dsl_types,
                    local_dsl_types,
                    local_defs,
                    data_allocator,
                    state_fields,
                    structs,
                )?;
                if expr.value_type() != ty {
                    return Err(ManifestCompileError::TypeMismatch {
                        module: module.to_string(),
                        expected: ty,
                        found: expr.value_type(),
                    });
                }
                if let Some(dsl_type) = inferred_dsl.or(expected_dsl) {
                    local_dsl_types.insert(target_name.clone(), dsl_type);
                }
                output.append(&mut stmts);
                output.push(IrStmt::Assign { local: index, value: expr });
            } else if state_fields.contains_key(target_name) {
                let (stmts, call) = lower_state_write_stmt(
                    module,
                    &[
                        DslCallArg::Positional(DslExpr::Literal(DslLiteral::String(
                            target_name.clone(),
                        ))),
                        DslCallArg::Positional(value.clone()),
                    ],
                    params,
                    locals,
                    param_dsl_types,
                    local_dsl_types,
                    local_defs,
                    params.len(),
                    data_allocator,
                    state_fields,
                    structs,
                )?;
                output.extend(stmts);
                output.push(IrStmt::Expr(call));
            } else {
                let (mut stmts, expr, inferred_dsl) = lower_expr_with_side_effects(
                    module,
                    function,
                    value,
                    None,
                    params,
                    locals,
                    param_dsl_types,
                    local_dsl_types,
                    local_defs,
                    data_allocator,
                    state_fields,
                    structs,
                )?;
                let index = (params.len() + local_defs.len()) as u32;
                local_defs.push(IrLocal {
                    name: target_name.clone(),
                    ty: expr.value_type(),
                });
                locals.insert(target_name.clone(), (index, expr.value_type()));
                if let Some(dsl_type) = inferred_dsl {
                    local_dsl_types.insert(target_name.clone(), dsl_type);
                }
                output.append(&mut stmts);
                output.push(IrStmt::Let { local: index, value: expr });
            }
        }
        DslStmt::If {
            branches,
            else_branch,
        } => {
            let mut else_body = Vec::new();
            if let Some(else_branch) = else_branch {
                for stmt in else_branch {
                    lower_stmt(
                        module,
                        stmt,
                        function,
                        params,
                        locals,
                        param_dsl_types,
                        local_dsl_types,
                        local_defs,
                        &mut else_body,
                        data_allocator,
                        return_type,
                        state_fields,
                        structs,
                    )?;
                }
            }

            for branch in branches.iter().rev() {
                let mut then_body = Vec::new();
                for stmt in &branch.body {
                    lower_stmt(
                        module,
                        stmt,
                        function,
                        params,
                        locals,
                        param_dsl_types,
                        local_dsl_types,
                        local_defs,
                        &mut then_body,
                        data_allocator,
                        return_type,
                        state_fields,
                        structs,
                    )?;
                }
                let condition = lower_expr(
                    module,
                    function,
                    &branch.condition,
                    params,
                    locals,
                    param_dsl_types,
                    local_dsl_types,
                    data_allocator,
                    state_fields,
                    structs,
                )?;
                else_body = vec![IrStmt::If {
                    condition,
                    then_body,
                    else_body,
                }];
            }

            output.extend(else_body);
        }
        DslStmt::While { condition, body } => {
            let condition = lower_expr(
                module,
                function,
                condition,
                params,
                locals,
                param_dsl_types,
                local_dsl_types,
                data_allocator,
                state_fields,
                structs,
            )?;
            let mut lowered_body = Vec::new();
            for stmt in body {
                lower_stmt(
                    module,
                    stmt,
                    function,
                    params,
                    locals,
                    param_dsl_types,
                    local_dsl_types,
                    local_defs,
                    &mut lowered_body,
                    data_allocator,
                    return_type,
                    state_fields,
                    structs,
                )?;
            }
            output.push(IrStmt::While {
                condition,
                body: lowered_body,
            });
        }
        DslStmt::Return(expr) => {
            let lowered = match expr {
                Some(expr) => Some(lower_expr(
                    module,
                    function,
                    expr,
                    params,
                    locals,
                    param_dsl_types,
                    local_dsl_types,
                    data_allocator,
                    state_fields,
                    structs,
                )?),
                None => None,
            };
            match (return_type, lowered.as_ref()) {
                (Some(expected), Some(found)) if expected != found.value_type() => {
                    return Err(ManifestCompileError::TypeMismatch {
                        module: module.to_string(),
                        expected,
                        found: found.value_type(),
                    });
                }
                (Some(_), None) => {
                    return Err(ManifestCompileError::TypeMismatch {
                        module: module.to_string(),
                        expected: return_type.unwrap(),
                        found: ValueType::I32,
                    });
                }
                (None, Some(_)) => {
                    return Err(ManifestCompileError::TypeMismatch {
                        module: module.to_string(),
                        expected: ValueType::I32,
                        found: lowered.as_ref().unwrap().value_type(),
                    });
                }
                _ => {}
            }
            output.push(IrStmt::Return { value: lowered });
        }
        DslStmt::Raise(_) => {
            output.push(IrStmt::Expr(IrExpr::HostCall {
                function: crate::ir::HostFunction::SystemAbort,
                args: Vec::new(),
            }));
        }
        DslStmt::Pass => {}
        DslStmt::Expr(expr) => {
            if let DslExpr::Call { callee, args } = expr {
                if let DslExpr::Identifier(name) = callee.as_ref() {
                    if name == "state_write" {
                        let (stmts, call_expr) = lower_state_write_stmt(
                            module,
                            args,
                            params,
                            locals,
                            param_dsl_types,
                            local_dsl_types,
                            local_defs,
                            params.len(),
                            data_allocator,
                            state_fields,
                            structs,
                        )?;
                        output.extend(stmts);
                        output.push(IrStmt::Expr(call_expr));
                        return Ok(());
                    }
                }
            }

            let expr = lower_expr(
                module,
                function,
                expr,
                params,
                locals,
                param_dsl_types,
                local_dsl_types,
                data_allocator,
                state_fields,
                structs,
            )?;
            output.push(IrStmt::Expr(expr));
        }
        DslStmt::Break => output.push(IrStmt::Break),
        DslStmt::Continue => output.push(IrStmt::Continue),
        DslStmt::For {
            binding,
            iterable,
            body,
        } => {
            let bindings = split_binding_names(module, binding)?;
            if let DslExpr::ListLiteral(items) = iterable {
                if bindings.len() != 1 {
                    return Err(ManifestCompileError::UnsupportedStatement {
                        module: module.to_string(),
                        statement: format!("for binding mismatch {binding:?}"),
                    });
                }
                lower_for_list_literal(
                    module,
                    function,
                    &bindings[0],
                    items,
                    body,
                    params,
                    locals,
                    param_dsl_types,
                    local_dsl_types,
                    local_defs,
                    output,
                    data_allocator,
                    return_type,
                    state_fields,
                    structs,
                )?;
            } else if bindings.len() == 1 {
                lower_for_list_iterable(
                    module,
                    function,
                    &bindings[0],
                    iterable,
                    body,
                    params,
                    locals,
                    param_dsl_types,
                    local_dsl_types,
                    local_defs,
                    output,
                    data_allocator,
                    return_type,
                    state_fields,
                    structs,
                )?;
            } else if bindings.len() == 2 {
                lower_for_map_items(
                    module,
                    function,
                    &bindings[0],
                    &bindings[1],
                    iterable,
                    body,
                    params,
                    locals,
                    param_dsl_types,
                    local_dsl_types,
                    local_defs,
                    output,
                    data_allocator,
                    return_type,
                    state_fields,
                    structs,
                )?;
            } else {
                return Err(ManifestCompileError::UnsupportedStatement {
                    module: module.to_string(),
                    statement: format!("for binding mismatch {binding:?}"),
                });
            }
        }
    }

    Ok(())
}

fn lower_expr(
    module: &str,
    function: &str,
    expr: &DslExpr,
    params: &HashMap<String, (u32, ValueType)>,
    locals: &HashMap<String, (u32, ValueType)>,
    param_dsl_types: &HashMap<String, DslType>,
    local_dsl_types: &HashMap<String, DslType>,
    data_allocator: &mut DataAllocator,
    state_fields: &HashMap<String, DslType>,
    structs: &HashMap<String, DslStruct>,
) -> Result<IrExpr, ManifestCompileError> {
    fn boolify(expr: IrExpr) -> IrExpr {
        let zero = match expr.value_type() {
            ValueType::I32 => IrExpr::ConstI32(0),
            ValueType::I64 => IrExpr::ConstI64(0),
        };
        IrExpr::Binary {
            op: IrBinaryOp::NotEqual,
            left: Box::new(expr),
            right: Box::new(zero),
            ty: ValueType::I32,
        }
    }

    match expr {
        DslExpr::Identifier(name) => {
            if let Some((index, ty)) = locals.get(name).copied() {
                return Ok(IrExpr::Local { index, ty });
            }
            if let Some((index, ty)) = params.get(name).copied() {
                return Ok(IrExpr::Param { index, ty });
            }
            if let Some(field_ty) = state_fields.get(name) {
                let key_bytes = serialize::encode_string(name);
                let key_ptr = data_allocator.allocate(key_bytes.clone());
                let out_len_ptr = data_allocator.allocate(vec![0, 0, 0, 0]);
                if is_pointer_type(field_ty) {
                    return Ok(IrExpr::StateReadRaw {
                        key_ptr,
                        key_len: key_bytes.len() as u32,
                        out_len_ptr,
                    });
                }
                let ty = map_type(module, field_ty)?;
                return Ok(IrExpr::StateRead {
                    key_ptr,
                    key_len: key_bytes.len() as u32,
                    out_len_ptr,
                    ty,
                });
            }
            Err(ManifestCompileError::UnknownIdentifier {
                module: module.to_string(),
                identifier: name.clone(),
            })
        }
        DslExpr::Literal(literal) => lower_literal(module, literal, data_allocator),
        DslExpr::ListLiteral(_) | DslExpr::MapLiteral(_) => {
            Err(ManifestCompileError::UnsupportedExpression {
                module: module.to_string(),
                expression: format!("literal {expr:?}"),
            })
        }
        DslExpr::Attribute { target, attribute } => {
            let (base_expr, base_type) = resolve_value_expr(
                module,
                target,
                params,
                locals,
                param_dsl_types,
                local_dsl_types,
                data_allocator,
                state_fields,
            )?;
            if let DslType::Optional(_) = base_type {
                return match attribute.as_str() {
                    "is_some" => Ok(optional_is_some_expr(base_expr)),
                    "is_none" => Ok(optional_is_none_expr(base_expr)),
                    _ => Err(ManifestCompileError::UnsupportedExpression {
                        module: module.to_string(),
                        expression: format!("unsupported optional attribute {attribute}"),
                    }),
                };
            }
            let struct_name = match base_type {
                DslType::Custom(name) => name,
                _ => {
                    return Err(ManifestCompileError::UnsupportedExpression {
                        module: module.to_string(),
                        expression: format!("attribute on non-struct {base_type:?}"),
                    })
                }
            };
            let struct_def = structs.get(&struct_name).ok_or_else(|| {
                ManifestCompileError::UnsupportedType {
                    module: module.to_string(),
                    ty: struct_name.clone(),
                }
            })?;
            return lower_struct_field_expr(module, base_expr, struct_def, attribute);
        }
        DslExpr::Binary { left, op, right } => {
            let left = lower_expr(
                module,
                function,
                left,
                params,
                locals,
                param_dsl_types,
                local_dsl_types,
                data_allocator,
                state_fields,
                structs,
            )?;
            let right = lower_expr(
                module,
                function,
                right,
                params,
                locals,
                param_dsl_types,
                local_dsl_types,
                data_allocator,
                state_fields,
                structs,
            )?;
            let left_ty = left.value_type();
            let right_ty = right.value_type();
            if left_ty != right_ty {
                return Err(ManifestCompileError::TypeMismatch {
                    module: module.to_string(),
                    expected: left_ty,
                    found: right_ty,
                });
            }

            let (op, result_ty) = match op {
                DslBinaryOp::Add => (IrBinaryOp::Add, left_ty),
                DslBinaryOp::Sub => (IrBinaryOp::Sub, left_ty),
                DslBinaryOp::Mul => (IrBinaryOp::Mul, left_ty),
                DslBinaryOp::Div => (IrBinaryOp::DivUInt, left_ty),
                DslBinaryOp::Mod => (IrBinaryOp::RemUInt, left_ty),
                DslBinaryOp::Equal => (IrBinaryOp::Equal, ValueType::I32),
                DslBinaryOp::NotEqual => (IrBinaryOp::NotEqual, ValueType::I32),
                DslBinaryOp::Less => (IrBinaryOp::Less, ValueType::I32),
                DslBinaryOp::LessEqual => (IrBinaryOp::LessEqual, ValueType::I32),
                DslBinaryOp::Greater => (IrBinaryOp::Greater, ValueType::I32),
                DslBinaryOp::GreaterEqual => (IrBinaryOp::GreaterEqual, ValueType::I32),
                DslBinaryOp::LogicalAnd | DslBinaryOp::LogicalOr => {
                    let left_bool = boolify(left);
                    let right_bool = boolify(right);
                    let expr = match op {
                        DslBinaryOp::LogicalAnd => IrExpr::Binary {
                            op: IrBinaryOp::Mul,
                            left: Box::new(left_bool),
                            right: Box::new(right_bool),
                            ty: ValueType::I32,
                        },
                        DslBinaryOp::LogicalOr => {
                            let sum = IrExpr::Binary {
                                op: IrBinaryOp::Add,
                                left: Box::new(left_bool),
                                right: Box::new(right_bool),
                                ty: ValueType::I32,
                            };
                            IrExpr::Binary {
                                op: IrBinaryOp::NotEqual,
                                left: Box::new(sum),
                                right: Box::new(IrExpr::ConstI32(0)),
                                ty: ValueType::I32,
                            }
                        }
                        _ => unreachable!(),
                    };
                    return Ok(expr);
                }
            };

            Ok(IrExpr::Binary {
                op,
                left: Box::new(left),
                right: Box::new(right),
                ty: result_ty,
            })
        }
        DslExpr::Unary { op, expr } => {
            let expr = lower_expr(
                module,
                function,
                expr,
                params,
                locals,
                param_dsl_types,
                local_dsl_types,
                data_allocator,
                state_fields,
                structs,
            )?;
            match op {
                DslUnaryOp::Neg => Ok(IrExpr::Binary {
                    op: IrBinaryOp::Sub,
                    left: Box::new(IrExpr::ConstI64(0)),
                    right: Box::new(expr),
                    ty: ValueType::I64,
                }),
                DslUnaryOp::Not => Ok(boolify(expr)),
            }
        }
        DslExpr::Call { callee, args } => {
            if let DslExpr::Attribute { target, attribute } = callee.as_ref() {
                return lower_attribute_call(
                    module,
                    attribute,
                    target,
                    args,
                    params,
                    locals,
                    param_dsl_types,
                    local_dsl_types,
                    data_allocator,
                    state_fields,
                    structs,
                );
            }
            if let DslExpr::Identifier(name) = callee.as_ref() {
                if name == "state_read" {
                    return lower_state_read(module, args, data_allocator, structs);
                }
                if let Some(host) = crate::ir::HostFunction::from_identifier(name) {
                    let expected = host.params();
                    let positional = call_args_to_positional(module, args)?;
                    if positional.len() != expected.len() {
                        return Err(ManifestCompileError::UnsupportedExpression {
                            module: module.to_string(),
                            expression: format!("{name} arity mismatch"),
                        });
                    }
                    let mut lowered_args = Vec::new();
                    for (arg, expected_ty) in positional.iter().zip(expected.iter()) {
                        let lowered = lower_expr(
                            module,
                            function,
                            arg,
                            params,
                            locals,
                            param_dsl_types,
                            local_dsl_types,
                            data_allocator,
                            state_fields,
                            structs,
                        )?;
                        if lowered.value_type() != *expected_ty {
                            return Err(ManifestCompileError::TypeMismatch {
                                module: module.to_string(),
                                expected: *expected_ty,
                                found: lowered.value_type(),
                            });
                        }
                        lowered_args.push(lowered);
                    }
                    return Ok(IrExpr::HostCall { function: host, args: lowered_args });
                }
            }

            Err(ManifestCompileError::UnsupportedExpression {
                module: module.to_string(),
                expression: format!("call {:?}", callee),
            })
        }
        DslExpr::Index { .. } => Err(ManifestCompileError::UnsupportedExpression {
            module: module.to_string(),
            expression: format!("{:?}", expr),
        }),
    }
}

fn lower_expr_with_side_effects(
    module: &str,
    function: &str,
    expr: &DslExpr,
    expected_dsl: Option<DslType>,
    params: &HashMap<String, (u32, ValueType)>,
    locals: &HashMap<String, (u32, ValueType)>,
    param_dsl_types: &HashMap<String, DslType>,
    local_dsl_types: &HashMap<String, DslType>,
    local_defs: &mut Vec<IrLocal>,
    data_allocator: &mut DataAllocator,
    state_fields: &HashMap<String, DslType>,
    structs: &HashMap<String, DslStruct>,
) -> Result<(Vec<IrStmt>, IrExpr, Option<DslType>), ManifestCompileError> {
    match expr {
        DslExpr::Literal(DslLiteral::None) => {
            let opt_type = match expected_dsl.clone() {
                Some(DslType::Optional(inner)) => DslType::Optional(inner),
                _ => DslType::Optional(Box::new(DslType::Any)),
            };
            let (stmts, ptr, _len) = lower_optional_none_buffer(
                module,
                params.len(),
                local_defs,
            )?;
            return Ok((stmts, ptr, Some(opt_type)));
        }
        DslExpr::Call { callee, args } => {
            if let DslExpr::Identifier(name) = callee.as_ref() {
                if name == "Some" {
                    let positional = call_args_to_positional(module, args)?;
                    let opt_type = match expected_dsl.clone() {
                        Some(DslType::Optional(inner)) => DslType::Optional(inner),
                        _ => {
                            let inferred = positional.first().and_then(|arg| {
                                infer_dsl_type_from_expr(
                                    arg,
                                    param_dsl_types,
                                    local_dsl_types,
                                    state_fields,
                                    structs,
                                )
                            });
                            DslType::Optional(Box::new(inferred.unwrap_or(DslType::Any)))
                        }
                    };
                    let (stmts, ptr, _len) = lower_optional_buffer(
                        module,
                        args,
                        params,
                        locals,
                        param_dsl_types,
                        local_dsl_types,
                        local_defs,
                        params.len(),
                        data_allocator,
                        state_fields,
                        structs,
                    )?;
                    return Ok((stmts, ptr, Some(opt_type)));
                }
                if let Some(def) = structs.get(name) {
                    let (stmts, ptr, _len) = lower_struct_buffer(
                        module,
                        def,
                        args,
                        params,
                        locals,
                        param_dsl_types,
                        local_dsl_types,
                        local_defs,
                        params.len(),
                        data_allocator,
                        state_fields,
                        structs,
                    )?;
                    return Ok((stmts, ptr, Some(DslType::Custom(name.clone()))));
                }
            }
        }
        _ => {}
    }

    let lowered = lower_expr(
        module,
        function,
        expr,
        params,
        locals,
        param_dsl_types,
        local_dsl_types,
        data_allocator,
        state_fields,
        structs,
    )?;
    let inferred = infer_dsl_type_from_expr(expr, param_dsl_types, local_dsl_types, state_fields, structs);
    Ok((Vec::new(), lowered, inferred))
}

fn infer_dsl_type_from_expr(
    expr: &DslExpr,
    param_dsl_types: &HashMap<String, DslType>,
    local_dsl_types: &HashMap<String, DslType>,
    state_fields: &HashMap<String, DslType>,
    structs: &HashMap<String, DslStruct>,
) -> Option<DslType> {
    match expr {
        DslExpr::Literal(DslLiteral::Bool(_)) => Some(DslType::Bool),
        DslExpr::Literal(DslLiteral::Number(_)) => Some(DslType::Int { bits: 64 }),
        DslExpr::Literal(DslLiteral::String(_)) => Some(DslType::String),
        DslExpr::Literal(DslLiteral::Bytes(_)) => Some(DslType::Bytes),
        DslExpr::Literal(DslLiteral::None) => Some(DslType::Optional(Box::new(DslType::Any))),
        DslExpr::Identifier(name) => lookup_dsl_type(name, param_dsl_types, local_dsl_types, state_fields),
        DslExpr::ListLiteral(items) => {
            let mut item_type = None;
            for item in items {
                let ty = infer_dsl_type_from_expr(item, param_dsl_types, local_dsl_types, state_fields, structs);
                if item_type.is_none() {
                    item_type = ty;
                } else if item_type != ty {
                    item_type = Some(DslType::Any);
                    break;
                }
            }
            Some(DslType::List(Box::new(item_type.unwrap_or(DslType::Any))))
        }
        DslExpr::MapLiteral(entries) => {
            let mut key_type = None;
            let mut value_type = None;
            for (key, value) in entries {
                let key_ty = infer_dsl_type_from_expr(key, param_dsl_types, local_dsl_types, state_fields, structs);
                let value_ty = infer_dsl_type_from_expr(value, param_dsl_types, local_dsl_types, state_fields, structs);
                if key_type.is_none() {
                    key_type = key_ty;
                } else if key_type != key_ty {
                    key_type = Some(DslType::Any);
                }
                if value_type.is_none() {
                    value_type = value_ty;
                } else if value_type != value_ty {
                    value_type = Some(DslType::Any);
                }
            }
            Some(DslType::Map {
                key: Box::new(key_type.unwrap_or(DslType::Any)),
                value: Box::new(value_type.unwrap_or(DslType::Any)),
            })
        }
        DslExpr::Call { callee, args } => {
            if let DslExpr::Identifier(name) = callee.as_ref() {
                if name == "Some" {
                    let inner = call_args_to_positional("infer", args)
                        .ok()
                        .and_then(|positional| positional.first().cloned())
                        .and_then(|arg| infer_dsl_type_from_expr(&arg, param_dsl_types, local_dsl_types, state_fields, structs))
                        .unwrap_or(DslType::Any);
                    return Some(DslType::Optional(Box::new(inner)));
                }
                if structs.contains_key(name) {
                    return Some(DslType::Custom(name.clone()));
                }
            }
            None
        }
        _ => None,
    }
}

fn length_prefixed_len_expr(ptr: IrExpr) -> IrExpr {
    add_i32(load_i32(ptr), IrExpr::ConstI32(4))
}

fn optional_len_expr(
    module: &str,
    inner: &DslType,
    ptr: IrExpr,
) -> Result<IrExpr, ManifestCompileError> {
    let payload_len = match inner {
        DslType::Uint { .. } | DslType::Int { .. } => IrExpr::ConstI32(8),
        DslType::Bool => IrExpr::ConstI32(1),
        _ if is_pointer_type(inner) => {
            let payload_ptr = add_i32(ptr.clone(), IrExpr::ConstI32(1));
            length_prefixed_len_expr(payload_ptr)
        }
        _ => {
            return Err(ManifestCompileError::UnsupportedType {
                module: module.to_string(),
                ty: inner.to_string(),
            })
        }
    };
    let some_len = add_i32(payload_len, IrExpr::ConstI32(1));
    Ok(IrExpr::Select {
        condition: Box::new(optional_is_some_expr(ptr)),
        if_true: Box::new(some_len),
        if_false: Box::new(IrExpr::ConstI32(1)),
        ty: ValueType::I32,
    })
}

fn list_total_length(
    module: &str,
    list_ptr: IrExpr,
    element_type: &DslType,
    local_defs: &mut Vec<IrLocal>,
    params_len: usize,
) -> Result<(Vec<IrStmt>, IrExpr), ManifestCompileError> {
    let mut stmts = Vec::new();
    let count_idx = add_temp_local(local_defs, params_len, ValueType::I32, "__list_count");
    stmts.push(IrStmt::Let {
        local: count_idx,
        value: load_i32(list_ptr.clone()),
    });
    let total_idx = add_temp_local(local_defs, params_len, ValueType::I32, "__list_len");
    stmts.push(IrStmt::Let {
        local: total_idx,
        value: IrExpr::ConstI32(4),
    });
    let cursor_idx = add_temp_local(local_defs, params_len, ValueType::I32, "__list_cursor");
    stmts.push(IrStmt::Let {
        local: cursor_idx,
        value: add_i32(list_ptr.clone(), IrExpr::ConstI32(4)),
    });
    let idx_idx = add_temp_local(local_defs, params_len, ValueType::I32, "__list_index");
    stmts.push(IrStmt::Let {
        local: idx_idx,
        value: IrExpr::ConstI32(0),
    });

    let step_expr = match element_type {
        DslType::Uint { .. } | DslType::Int { .. } => IrExpr::ConstI32(8),
        DslType::Bool => IrExpr::ConstI32(1),
        _ if is_pointer_type(element_type) && !matches!(element_type, DslType::Optional(_)) => {
            length_prefixed_len_expr(IrExpr::Local {
                index: cursor_idx,
                ty: ValueType::I32,
            })
        }
        _ => {
            return Err(ManifestCompileError::UnsupportedType {
                module: module.to_string(),
                ty: element_type.to_string(),
            })
        }
    };

    let condition = IrExpr::Binary {
        op: IrBinaryOp::Less,
        left: Box::new(IrExpr::Local {
            index: idx_idx,
            ty: ValueType::I32,
        }),
        right: Box::new(IrExpr::Local {
            index: count_idx,
            ty: ValueType::I32,
        }),
        ty: ValueType::I32,
    };

    let mut body = Vec::new();
    body.push(IrStmt::Assign {
        local: total_idx,
        value: add_i32(
            IrExpr::Local {
                index: total_idx,
                ty: ValueType::I32,
            },
            step_expr.clone(),
        ),
    });
    body.push(IrStmt::Assign {
        local: cursor_idx,
        value: add_i32(
            IrExpr::Local {
                index: cursor_idx,
                ty: ValueType::I32,
            },
            step_expr,
        ),
    });
    body.push(IrStmt::Assign {
        local: idx_idx,
        value: add_i32(
            IrExpr::Local {
                index: idx_idx,
                ty: ValueType::I32,
            },
            IrExpr::ConstI32(1),
        ),
    });

    stmts.push(IrStmt::While { condition, body });

    Ok((
        stmts,
        IrExpr::Local {
            index: total_idx,
            ty: ValueType::I32,
        },
    ))
}

fn emit_copy_bytes(
    params_len: usize,
    local_defs: &mut Vec<IrLocal>,
    src_ptr: IrExpr,
    dest_ptr: IrExpr,
    len_expr: IrExpr,
) -> Vec<IrStmt> {
    let mut stmts = Vec::new();
    let idx_idx = add_temp_local(local_defs, params_len, ValueType::I32, "__copy_idx");
    stmts.push(IrStmt::Let {
        local: idx_idx,
        value: IrExpr::ConstI32(0),
    });
    let condition = IrExpr::Binary {
        op: IrBinaryOp::Less,
        left: Box::new(IrExpr::Local {
            index: idx_idx,
            ty: ValueType::I32,
        }),
        right: Box::new(len_expr),
        ty: ValueType::I32,
    };
    let mut body = Vec::new();
    let offset = IrExpr::Local {
        index: idx_idx,
        ty: ValueType::I32,
    };
    let byte_expr = IrExpr::LoadI8 {
        address: Box::new(add_i32(src_ptr, offset.clone())),
    };
    body.push(IrStmt::Store {
        address: add_i32(dest_ptr, offset.clone()),
        value: byte_expr,
        width: StoreWidth::I8,
    });
    body.push(IrStmt::Assign {
        local: idx_idx,
        value: add_i32(offset, IrExpr::ConstI32(1)),
    });
    stmts.push(IrStmt::While { condition, body });
    stmts
}

fn lower_optional_none_buffer(
    _module: &str,
    params_len: usize,
    local_defs: &mut Vec<IrLocal>,
) -> Result<(Vec<IrStmt>, IrExpr, IrExpr), ManifestCompileError> {
    let mut stmts = Vec::new();
    let ptr_idx = add_temp_local(local_defs, params_len, ValueType::I32, "__opt_ptr");
    stmts.push(IrStmt::Let {
        local: ptr_idx,
        value: IrExpr::HostCall {
            function: crate::ir::HostFunction::MemoryDeterministicMalloc,
            args: vec![IrExpr::ConstI32(1)],
        },
    });
    stmts.push(IrStmt::Store {
        address: IrExpr::Local {
            index: ptr_idx,
            ty: ValueType::I32,
        },
        value: IrExpr::ConstI32(0),
        width: StoreWidth::I8,
    });
    Ok((
        stmts,
        IrExpr::Local {
            index: ptr_idx,
            ty: ValueType::I32,
        },
        IrExpr::ConstI32(1),
    ))
}

fn lower_optional_buffer(
    module: &str,
    args: &[DslCallArg],
    params: &HashMap<String, (u32, ValueType)>,
    locals: &HashMap<String, (u32, ValueType)>,
    param_dsl_types: &HashMap<String, DslType>,
    local_dsl_types: &HashMap<String, DslType>,
    local_defs: &mut Vec<IrLocal>,
    params_len: usize,
    data_allocator: &mut DataAllocator,
    state_fields: &HashMap<String, DslType>,
    structs: &HashMap<String, DslStruct>,
) -> Result<(Vec<IrStmt>, IrExpr, IrExpr), ManifestCompileError> {
    let positional = call_args_to_positional(module, args)?;
    if positional.len() != 1 {
        return Err(ManifestCompileError::UnsupportedExpression {
            module: module.to_string(),
            expression: "Some arity".to_string(),
        });
    }

    let (mut stmts, inner_ptr, inner_len) = lower_value_buffer(
        module,
        &positional[0],
        params,
        locals,
        param_dsl_types,
        local_dsl_types,
        local_defs,
        params_len,
        data_allocator,
        state_fields,
        structs,
    )?;

    let inner_ptr_idx = add_temp_local(local_defs, params_len, ValueType::I32, "__opt_inner_ptr");
    stmts.push(IrStmt::Let {
        local: inner_ptr_idx,
        value: inner_ptr,
    });
    let inner_len_idx = add_temp_local(local_defs, params_len, ValueType::I32, "__opt_inner_len");
    stmts.push(IrStmt::Let {
        local: inner_len_idx,
        value: inner_len,
    });

    let total_len_expr = add_i32(
        IrExpr::Local {
            index: inner_len_idx,
            ty: ValueType::I32,
        },
        IrExpr::ConstI32(1),
    );
    let ptr_idx = add_temp_local(local_defs, params_len, ValueType::I32, "__opt_ptr");
    stmts.push(IrStmt::Let {
        local: ptr_idx,
        value: IrExpr::HostCall {
            function: crate::ir::HostFunction::MemoryDeterministicMalloc,
            args: vec![total_len_expr.clone()],
        },
    });
    stmts.push(IrStmt::Store {
        address: IrExpr::Local {
            index: ptr_idx,
            ty: ValueType::I32,
        },
        value: IrExpr::ConstI32(1),
        width: StoreWidth::I8,
    });

    let copy_stmts = emit_copy_bytes(
        params_len,
        local_defs,
        IrExpr::Local {
            index: inner_ptr_idx,
            ty: ValueType::I32,
        },
        add_i32(
            IrExpr::Local {
                index: ptr_idx,
                ty: ValueType::I32,
            },
            IrExpr::ConstI32(1),
        ),
        IrExpr::Local {
            index: inner_len_idx,
            ty: ValueType::I32,
        },
    );
    stmts.extend(copy_stmts);

    Ok((
        stmts,
        IrExpr::Local {
            index: ptr_idx,
            ty: ValueType::I32,
        },
        total_len_expr,
    ))
}

fn lower_struct_buffer(
    module: &str,
    def: &DslStruct,
    args: &[DslCallArg],
    params: &HashMap<String, (u32, ValueType)>,
    locals: &HashMap<String, (u32, ValueType)>,
    param_dsl_types: &HashMap<String, DslType>,
    local_dsl_types: &HashMap<String, DslType>,
    local_defs: &mut Vec<IrLocal>,
    params_len: usize,
    data_allocator: &mut DataAllocator,
    state_fields: &HashMap<String, DslType>,
    structs: &HashMap<String, DslStruct>,
) -> Result<(Vec<IrStmt>, IrExpr, IrExpr), ManifestCompileError> {
    let mut field_values: HashMap<String, DslExpr> = HashMap::new();
    for arg in args {
        match arg {
            DslCallArg::Named { name, value } => {
                field_values.insert(name.clone(), value.clone());
            }
            DslCallArg::Positional(_) => {
                return Err(ManifestCompileError::UnsupportedExpression {
                    module: module.to_string(),
                    expression: format!("positional args in {}", def.name),
                })
            }
        }
    }

    let mut stmts = Vec::new();
    let payload_len_idx = add_temp_local(local_defs, params_len, ValueType::I32, "__struct_payload");
    stmts.push(IrStmt::Let {
        local: payload_len_idx,
        value: IrExpr::ConstI32(0),
    });

    let mut field_info = Vec::new();
    for field in &def.fields {
        let value = field_values.remove(&field.name).ok_or_else(|| {
            ManifestCompileError::UnsupportedExpression {
                module: module.to_string(),
                expression: format!("missing field {} in {}", field.name, def.name),
            }
        })?;

        let (mut value_stmts, value_ptr, value_len) = lower_value_buffer(
            module,
            &value,
            params,
            locals,
            param_dsl_types,
            local_dsl_types,
            local_defs,
            params_len,
            data_allocator,
            state_fields,
            structs,
        )?;
        stmts.append(&mut value_stmts);

        let len_idx = add_temp_local(local_defs, params_len, ValueType::I32, "__field_len");
        stmts.push(IrStmt::Let {
            local: len_idx,
            value: value_len,
        });
        let ptr_idx = add_temp_local(local_defs, params_len, ValueType::I32, "__field_ptr");
        stmts.push(IrStmt::Let {
            local: ptr_idx,
            value: value_ptr,
        });
        stmts.push(IrStmt::Assign {
            local: payload_len_idx,
            value: add_i32(
                add_i32(
                    IrExpr::Local {
                        index: payload_len_idx,
                        ty: ValueType::I32,
                    },
                    IrExpr::Local {
                        index: len_idx,
                        ty: ValueType::I32,
                    },
                ),
                IrExpr::ConstI32(4),
            ),
        });
        field_info.push((len_idx, ptr_idx));
    }

    if !field_values.is_empty() {
        return Err(ManifestCompileError::UnsupportedExpression {
            module: module.to_string(),
            expression: format!("extra fields in {}", def.name),
        });
    }

    let total_len_expr = add_i32(
        IrExpr::Local {
            index: payload_len_idx,
            ty: ValueType::I32,
        },
        IrExpr::ConstI32(4),
    );
    let struct_ptr_idx = add_temp_local(local_defs, params_len, ValueType::I32, "__struct_ptr");
    stmts.push(IrStmt::Let {
        local: struct_ptr_idx,
        value: IrExpr::HostCall {
            function: crate::ir::HostFunction::MemoryDeterministicMalloc,
            args: vec![total_len_expr.clone()],
        },
    });
    stmts.push(IrStmt::Store {
        address: IrExpr::Local {
            index: struct_ptr_idx,
            ty: ValueType::I32,
        },
        value: IrExpr::Local {
            index: payload_len_idx,
            ty: ValueType::I32,
        },
        width: StoreWidth::I32,
    });
    let cursor_idx = add_temp_local(local_defs, params_len, ValueType::I32, "__struct_cursor");
    stmts.push(IrStmt::Let {
        local: cursor_idx,
        value: add_i32(
            IrExpr::Local {
                index: struct_ptr_idx,
                ty: ValueType::I32,
            },
            IrExpr::ConstI32(4),
        ),
    });

    for (len_idx, ptr_idx) in field_info {
        stmts.push(IrStmt::Store {
            address: IrExpr::Local {
                index: cursor_idx,
                ty: ValueType::I32,
            },
            value: IrExpr::Local {
                index: len_idx,
                ty: ValueType::I32,
            },
            width: StoreWidth::I32,
        });
        stmts.push(IrStmt::Assign {
            local: cursor_idx,
            value: add_i32(
                IrExpr::Local {
                    index: cursor_idx,
                    ty: ValueType::I32,
                },
                IrExpr::ConstI32(4),
            ),
        });
        let copy_stmts = emit_copy_bytes(
            params_len,
            local_defs,
            IrExpr::Local {
                index: ptr_idx,
                ty: ValueType::I32,
            },
            IrExpr::Local {
                index: cursor_idx,
                ty: ValueType::I32,
            },
            IrExpr::Local {
                index: len_idx,
                ty: ValueType::I32,
            },
        );
        stmts.extend(copy_stmts);
        stmts.push(IrStmt::Assign {
            local: cursor_idx,
            value: add_i32(
                IrExpr::Local {
                    index: cursor_idx,
                    ty: ValueType::I32,
                },
                IrExpr::Local {
                    index: len_idx,
                    ty: ValueType::I32,
                },
            ),
        });
    }

    Ok((
        stmts,
        IrExpr::Local {
            index: struct_ptr_idx,
            ty: ValueType::I32,
        },
        total_len_expr,
    ))
}

fn split_binding_names(module: &str, binding: &str) -> Result<Vec<String>, ManifestCompileError> {
    let names: Vec<String> = binding
        .split(',')
        .map(|name| name.trim())
        .filter(|name| !name.is_empty())
        .map(|name| name.to_string())
        .collect();
    if names.is_empty() {
        return Err(ManifestCompileError::UnsupportedStatement {
            module: module.to_string(),
            statement: "for binding missing".to_string(),
        });
    }
    Ok(names)
}

fn is_pointer_type(ty: &DslType) -> bool {
    matches!(
        ty,
        DslType::Bytes
            | DslType::String
            | DslType::Address
            | DslType::Optional(_)
            | DslType::List(_)
            | DslType::Map { .. }
            | DslType::Custom(_)
            | DslType::Any
    )
}

fn lookup_dsl_type(
    name: &str,
    param_dsl_types: &HashMap<String, DslType>,
    local_dsl_types: &HashMap<String, DslType>,
    state_fields: &HashMap<String, DslType>,
) -> Option<DslType> {
    local_dsl_types
        .get(name)
        .cloned()
        .or_else(|| param_dsl_types.get(name).cloned())
        .or_else(|| state_fields.get(name).cloned())
}

fn ensure_binding_local(
    module: &str,
    name: &str,
    dsl_type: &DslType,
    params_len: usize,
    locals: &mut HashMap<String, (u32, ValueType)>,
    local_dsl_types: &mut HashMap<String, DslType>,
    local_defs: &mut Vec<IrLocal>,
) -> Result<(u32, bool), ManifestCompileError> {
    let value_type = map_type(module, dsl_type)?;
    if let Some((index, ty)) = locals.get(name).copied() {
        if ty != value_type {
            return Err(ManifestCompileError::TypeMismatch {
                module: module.to_string(),
                expected: ty,
                found: value_type,
            });
        }
        return Ok((index, false));
    }

    let index = (params_len + local_defs.len()) as u32;
    local_defs.push(IrLocal {
        name: name.to_string(),
        ty: value_type,
    });
    locals.insert(name.to_string(), (index, value_type));
    local_dsl_types.insert(name.to_string(), dsl_type.clone());
    Ok((index, true))
}

fn add_i32(left: IrExpr, right: IrExpr) -> IrExpr {
    IrExpr::Binary {
        op: IrBinaryOp::Add,
        left: Box::new(left),
        right: Box::new(right),
        ty: ValueType::I32,
    }
}

fn load_i32(address: IrExpr) -> IrExpr {
    IrExpr::LoadI32 {
        address: Box::new(address),
    }
}

fn load_i64(address: IrExpr) -> IrExpr {
    IrExpr::LoadI64 {
        address: Box::new(address),
    }
}

fn lower_for_list_literal(
    module: &str,
    function: &str,
    binding: &str,
    items: &[DslExpr],
    body: &[DslStmt],
    params: &HashMap<String, (u32, ValueType)>,
    locals: &mut HashMap<String, (u32, ValueType)>,
    param_dsl_types: &HashMap<String, DslType>,
    local_dsl_types: &mut HashMap<String, DslType>,
    local_defs: &mut Vec<IrLocal>,
    output: &mut Vec<IrStmt>,
    data_allocator: &mut DataAllocator,
    return_type: Option<ValueType>,
    state_fields: &HashMap<String, DslType>,
    structs: &HashMap<String, DslStruct>,
) -> Result<(), ManifestCompileError> {
    let mut binding_index = locals.get(binding).map(|(idx, _)| *idx);
    let mut binding_ty = locals.get(binding).map(|(_, ty)| *ty);
    let mut initialized = binding_index.is_some();

    for item in items {
        let item_expr = lower_expr(
            module,
            function,
            item,
            params,
            locals,
            param_dsl_types,
            local_dsl_types,
            data_allocator,
            state_fields,
            structs,
        )?;

        let local_idx = if let Some(index) = binding_index {
            index
        } else {
            let index = (params.len() + local_defs.len()) as u32;
            let ty = item_expr.value_type();
            local_defs.push(IrLocal {
                name: binding.to_string(),
                ty,
            });
            locals.insert(binding.to_string(), (index, ty));
            binding_index = Some(index);
            binding_ty = Some(ty);
            index
        };

        let ty = binding_ty.unwrap_or(item_expr.value_type());
        if ty != item_expr.value_type() {
            return Err(ManifestCompileError::TypeMismatch {
                module: module.to_string(),
                expected: ty,
                found: item_expr.value_type(),
            });
        }

        if initialized {
            output.push(IrStmt::Assign {
                local: local_idx,
                value: item_expr,
            });
        } else {
            output.push(IrStmt::Let {
                local: local_idx,
                value: item_expr,
            });
            initialized = true;
        }

        for stmt in body {
            lower_stmt(
                module,
                stmt,
                function,
                params,
                locals,
                param_dsl_types,
                local_dsl_types,
                local_defs,
                output,
                data_allocator,
                return_type,
                state_fields,
                structs,
            )?;
        }
    }

    Ok(())
}

fn lower_for_list_iterable(
    module: &str,
    function: &str,
    binding: &str,
    iterable: &DslExpr,
    body: &[DslStmt],
    params: &HashMap<String, (u32, ValueType)>,
    locals: &mut HashMap<String, (u32, ValueType)>,
    param_dsl_types: &HashMap<String, DslType>,
    local_dsl_types: &mut HashMap<String, DslType>,
    local_defs: &mut Vec<IrLocal>,
    output: &mut Vec<IrStmt>,
    data_allocator: &mut DataAllocator,
    return_type: Option<ValueType>,
    state_fields: &HashMap<String, DslType>,
    structs: &HashMap<String, DslStruct>,
) -> Result<(), ManifestCompileError> {
    let (list_ptr, element_type) = resolve_list_iterable(
        module,
        iterable,
        params,
        locals,
        param_dsl_types,
        local_dsl_types,
        state_fields,
        data_allocator,
        structs,
    )?;

    let (binding_idx, binding_new) = ensure_binding_local(
        module,
        binding,
        &element_type,
        params.len(),
        locals,
        local_dsl_types,
        local_defs,
    )?;

    let list_ptr_idx = add_temp_local(local_defs, params.len(), ValueType::I32, "__list_ptr");
    output.push(IrStmt::Let {
        local: list_ptr_idx,
        value: list_ptr,
    });

    let count_idx = add_temp_local(local_defs, params.len(), ValueType::I32, "__list_count");
    output.push(IrStmt::Let {
        local: count_idx,
        value: load_i32(IrExpr::Local {
            index: list_ptr_idx,
            ty: ValueType::I32,
        }),
    });

    let cursor_idx = add_temp_local(local_defs, params.len(), ValueType::I32, "__list_cursor");
    output.push(IrStmt::Let {
        local: cursor_idx,
        value: add_i32(
            IrExpr::Local {
                index: list_ptr_idx,
                ty: ValueType::I32,
            },
            IrExpr::ConstI32(4),
        ),
    });

    let idx_local = add_temp_local(local_defs, params.len(), ValueType::I32, "__list_index");
    output.push(IrStmt::Let {
        local: idx_local,
        value: IrExpr::ConstI32(0),
    });

    let condition = IrExpr::Binary {
        op: IrBinaryOp::Less,
        left: Box::new(IrExpr::Local {
            index: idx_local,
            ty: ValueType::I32,
        }),
        right: Box::new(IrExpr::Local {
            index: count_idx,
            ty: ValueType::I32,
        }),
        ty: ValueType::I32,
    };

    let mut loop_body = Vec::new();
    let cursor_expr = IrExpr::Local {
        index: cursor_idx,
        ty: ValueType::I32,
    };

    let (value_expr, next_cursor_expr) = list_element_expr(
        module,
        &element_type,
        cursor_expr.clone(),
    )?;

    loop_body.push(if binding_new {
        IrStmt::Let {
            local: binding_idx,
            value: value_expr,
        }
    } else {
        IrStmt::Assign {
            local: binding_idx,
            value: value_expr,
        }
    });

    for stmt in body {
        lower_stmt(
            module,
            stmt,
            function,
            params,
            locals,
            param_dsl_types,
            local_dsl_types,
            local_defs,
            &mut loop_body,
            data_allocator,
            return_type,
            state_fields,
            structs,
        )?;
    }

    loop_body.push(IrStmt::Assign {
        local: cursor_idx,
        value: next_cursor_expr,
    });
    loop_body.push(IrStmt::Assign {
        local: idx_local,
        value: add_i32(
            IrExpr::Local {
                index: idx_local,
                ty: ValueType::I32,
            },
            IrExpr::ConstI32(1),
        ),
    });

    output.push(IrStmt::While {
        condition,
        body: loop_body,
    });

    Ok(())
}

fn lower_for_map_items(
    module: &str,
    function: &str,
    key_binding: &str,
    value_binding: &str,
    iterable: &DslExpr,
    body: &[DslStmt],
    params: &HashMap<String, (u32, ValueType)>,
    locals: &mut HashMap<String, (u32, ValueType)>,
    param_dsl_types: &HashMap<String, DslType>,
    local_dsl_types: &mut HashMap<String, DslType>,
    local_defs: &mut Vec<IrLocal>,
    output: &mut Vec<IrStmt>,
    data_allocator: &mut DataAllocator,
    return_type: Option<ValueType>,
    state_fields: &HashMap<String, DslType>,
    structs: &HashMap<String, DslStruct>,
) -> Result<(), ManifestCompileError> {
    let (map_ptr, key_type, value_type) = resolve_map_items_iterable(
        module,
        iterable,
        params,
        locals,
        param_dsl_types,
        local_dsl_types,
        state_fields,
        data_allocator,
    )?;

    let (key_idx, key_new) = ensure_binding_local(
        module,
        key_binding,
        &key_type,
        params.len(),
        locals,
        local_dsl_types,
        local_defs,
    )?;
    let (value_idx, value_new) = ensure_binding_local(
        module,
        value_binding,
        &value_type,
        params.len(),
        locals,
        local_dsl_types,
        local_defs,
    )?;

    let map_ptr_idx = add_temp_local(local_defs, params.len(), ValueType::I32, "__map_ptr");
    output.push(IrStmt::Let {
        local: map_ptr_idx,
        value: map_ptr,
    });

    let count_idx = add_temp_local(local_defs, params.len(), ValueType::I32, "__map_count");
    output.push(IrStmt::Let {
        local: count_idx,
        value: load_i32(IrExpr::Local {
            index: map_ptr_idx,
            ty: ValueType::I32,
        }),
    });

    let cursor_idx = add_temp_local(local_defs, params.len(), ValueType::I32, "__map_cursor");
    output.push(IrStmt::Let {
        local: cursor_idx,
        value: add_i32(
            IrExpr::Local {
                index: map_ptr_idx,
                ty: ValueType::I32,
            },
            IrExpr::ConstI32(4),
        ),
    });

    let idx_local = add_temp_local(local_defs, params.len(), ValueType::I32, "__map_index");
    output.push(IrStmt::Let {
        local: idx_local,
        value: IrExpr::ConstI32(0),
    });

    let key_len_idx = add_temp_local(local_defs, params.len(), ValueType::I32, "__map_key_len");
    let value_ptr_idx = add_temp_local(local_defs, params.len(), ValueType::I32, "__map_value_ptr");
    let value_len_idx = add_temp_local(local_defs, params.len(), ValueType::I32, "__map_value_len");

    let condition = IrExpr::Binary {
        op: IrBinaryOp::Less,
        left: Box::new(IrExpr::Local {
            index: idx_local,
            ty: ValueType::I32,
        }),
        right: Box::new(IrExpr::Local {
            index: count_idx,
            ty: ValueType::I32,
        }),
        ty: ValueType::I32,
    };

    let mut loop_body = Vec::new();
    let cursor_expr = IrExpr::Local {
        index: cursor_idx,
        ty: ValueType::I32,
    };

    loop_body.push(IrStmt::Assign {
        local: key_len_idx,
        value: load_i32(cursor_expr.clone()),
    });

    let value_ptr_expr = add_i32(
        add_i32(cursor_expr.clone(), IrExpr::ConstI32(4)),
        IrExpr::Local {
            index: key_len_idx,
            ty: ValueType::I32,
        },
    );

    loop_body.push(IrStmt::Assign {
        local: value_ptr_idx,
        value: value_ptr_expr,
    });
    loop_body.push(IrStmt::Assign {
        local: value_len_idx,
        value: load_i32(IrExpr::Local {
            index: value_ptr_idx,
            ty: ValueType::I32,
        }),
    });

    let key_value_expr = map_element_expr(
        module,
        &key_type,
        cursor_expr.clone(),
    )?;
    let value_value_expr = map_element_expr(
        module,
        &value_type,
        IrExpr::Local {
            index: value_ptr_idx,
            ty: ValueType::I32,
        },
    )?;

    loop_body.push(if key_new {
        IrStmt::Let {
            local: key_idx,
            value: key_value_expr,
        }
    } else {
        IrStmt::Assign {
            local: key_idx,
            value: key_value_expr,
        }
    });
    loop_body.push(if value_new {
        IrStmt::Let {
            local: value_idx,
            value: value_value_expr,
        }
    } else {
        IrStmt::Assign {
            local: value_idx,
            value: value_value_expr,
        }
    });

    for stmt in body {
        lower_stmt(
            module,
            stmt,
            function,
            params,
            locals,
            param_dsl_types,
            local_dsl_types,
            local_defs,
            &mut loop_body,
            data_allocator,
            return_type,
            state_fields,
            structs,
        )?;
    }

    let next_cursor_expr = add_i32(
        add_i32(
            IrExpr::Local {
                index: value_ptr_idx,
                ty: ValueType::I32,
            },
            IrExpr::ConstI32(4),
        ),
        IrExpr::Local {
            index: value_len_idx,
            ty: ValueType::I32,
        },
    );
    loop_body.push(IrStmt::Assign {
        local: cursor_idx,
        value: next_cursor_expr,
    });
    loop_body.push(IrStmt::Assign {
        local: idx_local,
        value: add_i32(
            IrExpr::Local {
                index: idx_local,
                ty: ValueType::I32,
            },
            IrExpr::ConstI32(1),
        ),
    });

    output.push(IrStmt::While {
        condition,
        body: loop_body,
    });

    Ok(())
}

fn list_element_expr(
    module: &str,
    element_type: &DslType,
    cursor_expr: IrExpr,
) -> Result<(IrExpr, IrExpr), ManifestCompileError> {
    match element_type {
        DslType::Uint { .. } | DslType::Int { .. } => Ok((
            load_i64(cursor_expr.clone()),
            add_i32(cursor_expr, IrExpr::ConstI32(8)),
        )),
        DslType::Bytes | DslType::String | DslType::Address => {
            let len_expr = load_i32(cursor_expr.clone());
            let next_cursor = add_i32(
                add_i32(cursor_expr.clone(), IrExpr::ConstI32(4)),
                len_expr,
            );
            Ok((cursor_expr, next_cursor))
        }
        DslType::Bool
        | DslType::Optional(_)
        | DslType::List(_)
        | DslType::Map { .. }
        | DslType::Custom(_)
        | DslType::Any => Err(ManifestCompileError::UnsupportedType {
            module: module.to_string(),
            ty: element_type.to_string(),
        }),
    }
}

fn map_element_expr(
    module: &str,
    element_type: &DslType,
    value_ptr_expr: IrExpr,
) -> Result<IrExpr, ManifestCompileError> {
    match element_type {
        DslType::Uint { .. } | DslType::Int { .. } => Ok(load_i64(add_i32(
            value_ptr_expr,
            IrExpr::ConstI32(4),
        ))),
        DslType::Bytes | DslType::String | DslType::Address => Ok(value_ptr_expr),
        DslType::Bool => Err(ManifestCompileError::UnsupportedType {
            module: module.to_string(),
            ty: element_type.to_string(),
        }),
        _ if is_pointer_type(element_type) => Ok(value_ptr_expr),
        _ => Err(ManifestCompileError::UnsupportedType {
            module: module.to_string(),
            ty: element_type.to_string(),
        }),
    }
}

fn resolve_list_iterable(
    module: &str,
    iterable: &DslExpr,
    params: &HashMap<String, (u32, ValueType)>,
    locals: &HashMap<String, (u32, ValueType)>,
    param_dsl_types: &HashMap<String, DslType>,
    local_dsl_types: &HashMap<String, DslType>,
    state_fields: &HashMap<String, DslType>,
    data_allocator: &mut DataAllocator,
    structs: &HashMap<String, DslStruct>,
) -> Result<(IrExpr, DslType), ManifestCompileError> {
    let name = match iterable {
        DslExpr::Identifier(name) => name,
        DslExpr::Attribute { target, attribute } => {
            let (base_expr, base_type) = resolve_value_expr(
                module,
                target,
                params,
                locals,
                param_dsl_types,
                local_dsl_types,
                data_allocator,
                state_fields,
            )?;
            let struct_name = match base_type {
                DslType::Custom(name) => name,
                _ => {
                    return Err(ManifestCompileError::UnsupportedType {
                        module: module.to_string(),
                        ty: base_type.to_string(),
                    })
                }
            };
            let struct_def = structs.get(&struct_name).ok_or_else(|| {
                ManifestCompileError::UnsupportedType {
                    module: module.to_string(),
                    ty: struct_name.clone(),
                }
            })?;
            let (field_ty, field_ptr) =
                resolve_struct_field_expr(module, base_expr, struct_def, attribute)?;
            let element_type = match field_ty {
                DslType::List(inner) => *inner,
                _ => {
                    return Err(ManifestCompileError::UnsupportedType {
                        module: module.to_string(),
                        ty: field_ty.to_string(),
                    })
                }
            };
            return Ok((field_ptr, element_type));
        }
        _ => {
            return Err(ManifestCompileError::UnsupportedStatement {
                module: module.to_string(),
                statement: format!("for in non-list iterable {iterable:?}"),
            })
        }
    };

    let dsl_type = lookup_dsl_type(name, param_dsl_types, local_dsl_types, state_fields)
        .ok_or_else(|| ManifestCompileError::UnknownIdentifier {
            module: module.to_string(),
            identifier: name.clone(),
        })?;

    let element_type = match dsl_type {
        DslType::List(inner) => *inner,
        _ => {
            return Err(ManifestCompileError::UnsupportedType {
                module: module.to_string(),
                ty: dsl_type.to_string(),
            })
        }
    };

    if let Some((index, ty)) = locals.get(name).copied() {
        if ty != ValueType::I32 {
            return Err(ManifestCompileError::TypeMismatch {
                module: module.to_string(),
                expected: ValueType::I32,
                found: ty,
            });
        }
        return Ok((IrExpr::Local { index, ty }, element_type));
    }
    if let Some((index, ty)) = params.get(name).copied() {
        if ty != ValueType::I32 {
            return Err(ManifestCompileError::TypeMismatch {
                module: module.to_string(),
                expected: ValueType::I32,
                found: ty,
            });
        }
        return Ok((IrExpr::Param { index, ty }, element_type));
    }
    if state_fields.contains_key(name) {
        let key_bytes = serialize::encode_string(name);
        let key_ptr = data_allocator.allocate(key_bytes.clone());
        let out_len_ptr = data_allocator.allocate(vec![0, 0, 0, 0]);
        return Ok((
            IrExpr::StateReadRaw {
                key_ptr,
                key_len: key_bytes.len() as u32,
                out_len_ptr,
            },
            element_type,
        ));
    }

    Err(ManifestCompileError::UnknownIdentifier {
        module: module.to_string(),
        identifier: name.clone(),
    })
}

fn resolve_map_items_iterable(
    module: &str,
    iterable: &DslExpr,
    params: &HashMap<String, (u32, ValueType)>,
    locals: &HashMap<String, (u32, ValueType)>,
    param_dsl_types: &HashMap<String, DslType>,
    local_dsl_types: &HashMap<String, DslType>,
    state_fields: &HashMap<String, DslType>,
    data_allocator: &mut DataAllocator,
) -> Result<(IrExpr, DslType, DslType), ManifestCompileError> {
    let target = match iterable {
        DslExpr::Call { callee, args } => {
            if !args.is_empty() {
                return Err(ManifestCompileError::UnsupportedExpression {
                    module: module.to_string(),
                    expression: "map.items() takes no args".to_string(),
                });
            }
            match callee.as_ref() {
                DslExpr::Attribute { target, attribute } if attribute == "items" => target,
                _ => {
                    return Err(ManifestCompileError::UnsupportedExpression {
                        module: module.to_string(),
                        expression: format!("for in non-map iterable {iterable:?}"),
                    })
                }
            }
        }
        _ => {
            return Err(ManifestCompileError::UnsupportedExpression {
                module: module.to_string(),
                expression: format!("for in non-map iterable {iterable:?}"),
            })
        }
    };

    let name = match target.as_ref() {
        DslExpr::Identifier(name) => name,
        _ => {
            return Err(ManifestCompileError::UnsupportedExpression {
                module: module.to_string(),
                expression: "map.items() target must be identifier".to_string(),
            })
        }
    };

    let dsl_type = lookup_dsl_type(name, param_dsl_types, local_dsl_types, state_fields)
        .ok_or_else(|| ManifestCompileError::UnknownIdentifier {
            module: module.to_string(),
            identifier: name.clone(),
        })?;

    let (key_type, value_type) = match dsl_type {
        DslType::Map { key, value } => (*key, *value),
        _ => {
            return Err(ManifestCompileError::UnsupportedType {
                module: module.to_string(),
                ty: dsl_type.to_string(),
            })
        }
    };

    if let Some((index, ty)) = locals.get(name).copied() {
        if ty != ValueType::I32 {
            return Err(ManifestCompileError::TypeMismatch {
                module: module.to_string(),
                expected: ValueType::I32,
                found: ty,
            });
        }
        return Ok((IrExpr::Local { index, ty }, key_type, value_type));
    }
    if let Some((index, ty)) = params.get(name).copied() {
        if ty != ValueType::I32 {
            return Err(ManifestCompileError::TypeMismatch {
                module: module.to_string(),
                expected: ValueType::I32,
                found: ty,
            });
        }
        return Ok((IrExpr::Param { index, ty }, key_type, value_type));
    }
    if state_fields.contains_key(name) {
        let key_bytes = serialize::encode_string(name);
        let key_ptr = data_allocator.allocate(key_bytes.clone());
        let out_len_ptr = data_allocator.allocate(vec![0, 0, 0, 0]);
        return Ok((
            IrExpr::StateReadRaw {
                key_ptr,
                key_len: key_bytes.len() as u32,
                out_len_ptr,
            },
            key_type,
            value_type,
        ));
    }

    Err(ManifestCompileError::UnknownIdentifier {
        module: module.to_string(),
        identifier: name.clone(),
    })
}

fn add_temp_local(
    local_defs: &mut Vec<IrLocal>,
    params_len: usize,
    ty: ValueType,
    name: &str,
) -> u32 {
    let index = (params_len + local_defs.len()) as u32;
    local_defs.push(IrLocal {
        name: name.to_string(),
        ty,
    });
    index
}

fn lower_literal(
    module: &str,
    literal: &DslLiteral,
    data_allocator: &mut DataAllocator,
) -> Result<IrExpr, ManifestCompileError> {
    match literal {
        DslLiteral::Bool(value) => Ok(IrExpr::ConstI32(if *value { 1 } else { 0 })),
        DslLiteral::Number(raw) => {
            let value = if let Some(hex) = raw.strip_prefix("0x") {
                i64::from_str_radix(hex, 16).map_err(|_| ManifestCompileError::UnsupportedExpression {
                    module: module.to_string(),
                    expression: raw.clone(),
                })?
            } else {
                raw.parse::<i64>().map_err(|_| ManifestCompileError::UnsupportedExpression {
                    module: module.to_string(),
                    expression: raw.clone(),
                })?
            };
            Ok(IrExpr::ConstI64(value))
        }
        DslLiteral::String(value) => {
            let bytes = serialize::encode_string(value);
            let offset = data_allocator.allocate(bytes);
            Ok(IrExpr::ConstI32(offset as i32))
        }
        DslLiteral::Bytes(value) => {
            let bytes = serialize::encode_bytes(value);
            let offset = data_allocator.allocate(bytes);
            Ok(IrExpr::ConstI32(offset as i32))
        }
        DslLiteral::None => Err(ManifestCompileError::UnsupportedExpression {
            module: module.to_string(),
            expression: "None".to_string(),
        }),
    }
}

fn resolve_value_expr(
    module: &str,
    expr: &DslExpr,
    params: &HashMap<String, (u32, ValueType)>,
    locals: &HashMap<String, (u32, ValueType)>,
    param_dsl_types: &HashMap<String, DslType>,
    local_dsl_types: &HashMap<String, DslType>,
    data_allocator: &mut DataAllocator,
    state_fields: &HashMap<String, DslType>,
) -> Result<(IrExpr, DslType), ManifestCompileError> {
    let name = match expr {
        DslExpr::Identifier(name) => name,
        _ => {
            return Err(ManifestCompileError::UnsupportedExpression {
                module: module.to_string(),
                expression: format!("unsupported target {expr:?}"),
            })
        }
    };

    if let Some((index, ty)) = locals.get(name).copied() {
        let dsl_type = local_dsl_types.get(name).cloned().ok_or_else(|| {
            ManifestCompileError::UnsupportedExpression {
                module: module.to_string(),
                expression: format!("missing type for {name}"),
            }
        })?;
        return Ok((IrExpr::Local { index, ty }, dsl_type));
    }
    if let Some((index, ty)) = params.get(name).copied() {
        let dsl_type = param_dsl_types.get(name).cloned().ok_or_else(|| {
            ManifestCompileError::UnsupportedExpression {
                module: module.to_string(),
                expression: format!("missing type for {name}"),
            }
        })?;
        return Ok((IrExpr::Param { index, ty }, dsl_type));
    }
    if let Some(field_ty) = state_fields.get(name) {
        let key_bytes = serialize::encode_string(name);
        let key_ptr = data_allocator.allocate(key_bytes.clone());
        let out_len_ptr = data_allocator.allocate(vec![0, 0, 0, 0]);
        if is_pointer_type(field_ty) {
            return Ok((
                IrExpr::StateReadRaw {
                    key_ptr,
                    key_len: key_bytes.len() as u32,
                    out_len_ptr,
                },
                field_ty.clone(),
            ));
        }
        let ty = map_type(module, field_ty)?;
        return Ok((
            IrExpr::StateRead {
                key_ptr,
                key_len: key_bytes.len() as u32,
                out_len_ptr,
                ty,
            },
            field_ty.clone(),
        ));
    }

    Err(ManifestCompileError::UnknownIdentifier {
        module: module.to_string(),
        identifier: name.clone(),
    })
}

fn resolve_struct_field_expr(
    module: &str,
    base_expr: IrExpr,
    struct_def: &DslStruct,
    field_name: &str,
) -> Result<(DslType, IrExpr), ManifestCompileError> {
    let mut cursor = add_i32(base_expr, IrExpr::ConstI32(4));
    for field in &struct_def.fields {
        let field_len = load_i32(cursor.clone());
        let field_ptr = add_i32(cursor.clone(), IrExpr::ConstI32(4));
        if field.name == field_name {
            return Ok((field.ty.clone(), field_ptr));
        }
        cursor = add_i32(field_ptr, field_len);
    }

    Err(ManifestCompileError::UnsupportedExpression {
        module: module.to_string(),
        expression: format!("unknown field {field_name} in {}", struct_def.name),
    })
}

fn lower_struct_field_expr(
    module: &str,
    base_expr: IrExpr,
    struct_def: &DslStruct,
    field_name: &str,
) -> Result<IrExpr, ManifestCompileError> {
    let (field_ty, field_ptr) =
        resolve_struct_field_expr(module, base_expr, struct_def, field_name)?;
    match field_ty {
        DslType::Uint { .. } | DslType::Int { .. } => Ok(load_i64(field_ptr)),
        DslType::Bool => Ok(IrExpr::LoadI8 {
            address: Box::new(field_ptr),
        }),
        _ if is_pointer_type(&field_ty) => Ok(field_ptr),
        _ => Err(ManifestCompileError::UnsupportedType {
            module: module.to_string(),
            ty: field_ty.to_string(),
        }),
    }
}

fn lower_attribute_call(
    module: &str,
    attribute: &str,
    target: &DslExpr,
    args: &[DslCallArg],
    params: &HashMap<String, (u32, ValueType)>,
    locals: &HashMap<String, (u32, ValueType)>,
    param_dsl_types: &HashMap<String, DslType>,
    local_dsl_types: &HashMap<String, DslType>,
    data_allocator: &mut DataAllocator,
    state_fields: &HashMap<String, DslType>,
    structs: &HashMap<String, DslStruct>,
) -> Result<IrExpr, ManifestCompileError> {
    let (base_expr, base_type) = resolve_value_expr(
        module,
        target,
        params,
        locals,
        param_dsl_types,
        local_dsl_types,
        data_allocator,
        state_fields,
    )?;

    let DslType::Optional(inner) = base_type else {
        return Err(ManifestCompileError::UnsupportedExpression {
            module: module.to_string(),
            expression: format!("attribute call on non-optional {attribute}"),
        });
    };

    let positional = call_args_to_positional(module, args)?;
    match attribute {
        "is_some" => {
            if !positional.is_empty() {
                return Err(ManifestCompileError::UnsupportedExpression {
                    module: module.to_string(),
                    expression: "is_some takes no args".to_string(),
                });
            }
            Ok(optional_is_some_expr(base_expr))
        }
        "unwrap" => {
            if !positional.is_empty() {
                return Err(ManifestCompileError::UnsupportedExpression {
                    module: module.to_string(),
                    expression: "unwrap takes no args".to_string(),
                });
            }
            Ok(optional_unwrap_expr(module, *inner, base_expr)?)
        }
        "unwrap_or" => {
            if positional.len() != 1 {
                return Err(ManifestCompileError::UnsupportedExpression {
                    module: module.to_string(),
                    expression: "unwrap_or takes one arg".to_string(),
                });
            }
            let default_expr = lower_expr(
                module,
                "unwrap_or",
                &positional[0],
                params,
                locals,
                param_dsl_types,
                local_dsl_types,
                data_allocator,
                state_fields,
                structs,
            )?;
            let expected = map_type(module, &inner)?;
            if default_expr.value_type() != expected {
                return Err(ManifestCompileError::TypeMismatch {
                    module: module.to_string(),
                    expected,
                    found: default_expr.value_type(),
                });
            }
            let unwrap_expr = optional_unwrap_expr(module, *inner, base_expr.clone())?;
            Ok(IrExpr::Select {
                condition: Box::new(optional_is_some_expr(base_expr)),
                if_true: Box::new(unwrap_expr),
                if_false: Box::new(default_expr),
                ty: expected,
            })
        }
        _ => Err(ManifestCompileError::UnsupportedExpression {
            module: module.to_string(),
            expression: format!("unsupported attribute call {attribute}"),
        }),
    }
}

fn optional_is_some_expr(base_expr: IrExpr) -> IrExpr {
    IrExpr::Binary {
        op: IrBinaryOp::NotEqual,
        left: Box::new(IrExpr::LoadI8 {
            address: Box::new(base_expr),
        }),
        right: Box::new(IrExpr::ConstI32(0)),
        ty: ValueType::I32,
    }
}

fn optional_is_none_expr(base_expr: IrExpr) -> IrExpr {
    IrExpr::Binary {
        op: IrBinaryOp::Equal,
        left: Box::new(IrExpr::LoadI8 {
            address: Box::new(base_expr),
        }),
        right: Box::new(IrExpr::ConstI32(0)),
        ty: ValueType::I32,
    }
}

fn optional_unwrap_expr(
    module: &str,
    inner: DslType,
    base_expr: IrExpr,
) -> Result<IrExpr, ManifestCompileError> {
    let payload_ptr = add_i32(base_expr, IrExpr::ConstI32(1));
    match inner {
        DslType::Uint { .. } | DslType::Int { .. } => Ok(load_i64(payload_ptr)),
        DslType::Bool => Ok(IrExpr::LoadI8 {
            address: Box::new(payload_ptr),
        }),
        _ if is_pointer_type(&inner) => Ok(payload_ptr),
        _ => Err(ManifestCompileError::UnsupportedType {
            module: module.to_string(),
            ty: inner.to_string(),
        }),
    }
}

fn lower_state_read(
    module: &str,
    args: &[DslCallArg],
    data_allocator: &mut DataAllocator,
    structs: &HashMap<String, DslStruct>,
) -> Result<IrExpr, ManifestCompileError> {
    let args = call_args_to_positional(module, args)?;
    if args.len() != 1 {
        return Err(ManifestCompileError::UnsupportedExpression {
            module: module.to_string(),
            expression: "state_read arity".to_string(),
        });
    }

    let key_bytes = literal_to_bytes(module, &args[0], structs)?;
    let key_ptr = data_allocator.allocate(key_bytes.clone());
    let out_len_ptr = data_allocator.allocate(vec![0, 0, 0, 0]);

    Ok(IrExpr::StateRead {
        key_ptr,
        key_len: key_bytes.len() as u32,
        out_len_ptr,
        ty: ValueType::I64,
    })
}

fn lower_state_write_stmt(
    module: &str,
    args: &[DslCallArg],
    params: &HashMap<String, (u32, ValueType)>,
    locals: &HashMap<String, (u32, ValueType)>,
    param_dsl_types: &HashMap<String, DslType>,
    local_dsl_types: &HashMap<String, DslType>,
    local_defs: &mut Vec<IrLocal>,
    params_len: usize,
    data_allocator: &mut DataAllocator,
    state_fields: &HashMap<String, DslType>,
    structs: &HashMap<String, DslStruct>,
) -> Result<(Vec<IrStmt>, IrExpr), ManifestCompileError> {
    let args = call_args_to_positional(module, args)?;
    if args.len() != 2 {
        return Err(ManifestCompileError::UnsupportedExpression {
            module: module.to_string(),
            expression: "state_write arity".to_string(),
        });
    }

    let (mut stmts, key_ptr, key_len) = lower_value_buffer(
        module,
        &args[0],
        params,
        locals,
        param_dsl_types,
        local_dsl_types,
        local_defs,
        params_len,
        data_allocator,
        state_fields,
        structs,
    )?;

    let (value_stmts, value_ptr, value_len) = lower_value_buffer(
        module,
        &args[1],
        params,
        locals,
        param_dsl_types,
        local_dsl_types,
        local_defs,
        params_len,
        data_allocator,
        state_fields,
        structs,
    )?;
    stmts.extend(value_stmts);

    let call = IrExpr::HostCall {
        function: crate::ir::HostFunction::StdStateWrite,
        args: vec![key_ptr, key_len, value_ptr, value_len],
    };

    Ok((stmts, call))
}

fn literal_to_bytes(
    module: &str,
    expr: &DslExpr,
    structs: &HashMap<String, DslStruct>,
) -> Result<Vec<u8>, ManifestCompileError> {
    match expr {
        DslExpr::Literal(DslLiteral::String(value)) => Ok(serialize::encode_string(value)),
        DslExpr::Literal(DslLiteral::Bytes(value)) => Ok(serialize::encode_bytes(value)),
        DslExpr::Literal(DslLiteral::Bool(value)) => Ok(serialize::encode_bool(*value)),
        DslExpr::Literal(DslLiteral::Number(value)) => {
            let parsed = value.parse::<i64>().map_err(|_| ManifestCompileError::UnsupportedExpression {
                module: module.to_string(),
                expression: value.clone(),
            })?;
            Ok(serialize::encode_i64(parsed))
        }
        DslExpr::Literal(DslLiteral::None) => Ok(serialize::encode_optional(None)),
        DslExpr::ListLiteral(entries) => {
            let mut encoded = Vec::new();
            for entry in entries {
                encoded.push(literal_to_bytes(module, entry, structs)?);
            }
            Ok(serialize::encode_list(&encoded))
        }
        DslExpr::MapLiteral(entries) => {
            let mut encoded = Vec::new();
            for (key, value) in entries {
                let key_bytes = literal_to_bytes(module, key, structs)?;
                let value_bytes = literal_to_bytes(module, value, structs)?;
                encoded.push((key_bytes, value_bytes));
            }
            Ok(serialize::encode_map(&encoded))
        }
        DslExpr::Call { callee, args } => {
            if let DslExpr::Identifier(name) = callee.as_ref() {
                if name == "Some" {
                    let args = call_args_to_positional(module, args)?;
                    if args.len() != 1 {
                        return Err(ManifestCompileError::UnsupportedExpression {
                            module: module.to_string(),
                            expression: "Some arity".to_string(),
                        });
                    }
                    let inner = literal_to_bytes(module, &args[0], structs)?;
                    return Ok(serialize::encode_optional(Some(&inner)));
                }
                if let Some(def) = structs.get(name) {
                    return encode_struct_literal(module, def, args, structs);
                }
            }
            Err(ManifestCompileError::UnsupportedExpression {
                module: module.to_string(),
                expression: format!("non-literal state value {expr:?}"),
            })
        }
        _ => Err(ManifestCompileError::UnsupportedExpression {
            module: module.to_string(),
            expression: format!("non-literal state value {expr:?}"),
        }),
    }
}

fn encode_struct_literal(
    module: &str,
    def: &DslStruct,
    args: &[DslCallArg],
    structs: &HashMap<String, DslStruct>,
) -> Result<Vec<u8>, ManifestCompileError> {
    let mut field_values: HashMap<String, DslExpr> = HashMap::new();
    for arg in args {
        match arg {
            DslCallArg::Named { name, value } => {
                field_values.insert(name.clone(), value.clone());
            }
            DslCallArg::Positional(_) => {
                return Err(ManifestCompileError::UnsupportedExpression {
                    module: module.to_string(),
                    expression: format!("positional args in {}", def.name),
                })
            }
        }
    }

    let mut payload = Vec::new();
    for field in &def.fields {
        let value = field_values.remove(&field.name).ok_or_else(|| {
            ManifestCompileError::UnsupportedExpression {
                module: module.to_string(),
                expression: format!("missing field {} in {}", field.name, def.name),
            }
        })?;
        let encoded = literal_to_bytes(module, &value, structs)?;
        payload.extend_from_slice(&(encoded.len() as u32).to_le_bytes());
        payload.extend_from_slice(&encoded);
    }

    if !field_values.is_empty() {
        return Err(ManifestCompileError::UnsupportedExpression {
            module: module.to_string(),
            expression: format!("extra fields in {}", def.name),
        });
    }

    let mut out = Vec::with_capacity(4 + payload.len());
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(&payload);
    Ok(out)
}

fn call_args_to_positional(
    module: &str,
    args: &[DslCallArg],
) -> Result<Vec<DslExpr>, ManifestCompileError> {
    let mut positional = Vec::new();
    for arg in args {
        match arg {
            DslCallArg::Positional(expr) => positional.push(expr.clone()),
            DslCallArg::Named { .. } => {
                return Err(ManifestCompileError::UnsupportedExpression {
                    module: module.to_string(),
                    expression: "named args not supported here".to_string(),
                })
            }
        }
    }
    Ok(positional)
}

fn lower_value_buffer(
    module: &str,
    expr: &DslExpr,
    params: &HashMap<String, (u32, ValueType)>,
    locals: &HashMap<String, (u32, ValueType)>,
    param_dsl_types: &HashMap<String, DslType>,
    local_dsl_types: &HashMap<String, DslType>,
    local_defs: &mut Vec<IrLocal>,
    params_len: usize,
    data_allocator: &mut DataAllocator,
    state_fields: &HashMap<String, DslType>,
    structs: &HashMap<String, DslStruct>,
) -> Result<(Vec<IrStmt>, IrExpr, IrExpr), ManifestCompileError> {
    if let Ok(bytes) = literal_to_bytes(module, expr, structs) {
        let ptr = data_allocator.allocate(bytes.clone());
        return Ok((
            Vec::new(),
            IrExpr::ConstI32(ptr as i32),
            IrExpr::ConstI32(bytes.len() as i32),
        ));
    }

    if let DslExpr::Call { callee, args } = expr {
        if let DslExpr::Identifier(name) = callee.as_ref() {
            if name == "Some" {
                return lower_optional_buffer(
                    module,
                    args,
                    params,
                    locals,
                    param_dsl_types,
                    local_dsl_types,
                    local_defs,
                    params_len,
                    data_allocator,
                    state_fields,
                    structs,
                );
            }
            if let Some(def) = structs.get(name) {
                return lower_struct_buffer(
                    module,
                    def,
                    args,
                    params,
                    locals,
                    param_dsl_types,
                    local_dsl_types,
                    local_defs,
                    params_len,
                    data_allocator,
                    state_fields,
                    structs,
                );
            }
        }
    }

    let (index, ty, dsl_type) = match expr {
        DslExpr::Identifier(name) => locals
            .get(name)
            .copied()
            .map(|(idx, ty)| (idx, ty, local_dsl_types.get(name).cloned()))
            .or_else(|| {
                params
                    .get(name)
                    .copied()
                    .map(|(idx, ty)| (idx, ty, param_dsl_types.get(name).cloned()))
            })
            .ok_or_else(|| ManifestCompileError::UnknownIdentifier {
                module: module.to_string(),
                identifier: name.clone(),
            })?,
        _ => {
            return Err(ManifestCompileError::UnsupportedExpression {
                module: module.to_string(),
                expression: format!("state value {expr:?}"),
            })
        }
    };

    let value_expr = match expr {
        DslExpr::Identifier(name) => {
            if let Some((_, _)) = locals.get(name) {
                IrExpr::Local { index, ty }
            } else {
                IrExpr::Param { index, ty }
            }
        }
        _ => IrExpr::Local { index, ty },
    };

    if let Some(dsl_type) = dsl_type.as_ref() {
        match dsl_type {
            DslType::String | DslType::Bytes | DslType::Address | DslType::Custom(_) | DslType::Any => {
                let len_expr = length_prefixed_len_expr(value_expr.clone());
                return Ok((Vec::new(), value_expr, len_expr));
            }
            DslType::Optional(inner) => {
                let len_expr = optional_len_expr(module, inner, value_expr.clone())?;
                return Ok((Vec::new(), value_expr, len_expr));
            }
            DslType::List(inner) => {
                let (stmts, len_expr) = list_total_length(
                    module,
                    value_expr.clone(),
                    inner,
                    local_defs,
                    params_len,
                )?;
                return Ok((stmts, value_expr, len_expr));
            }
            DslType::Map { .. } => {
                return Err(ManifestCompileError::UnsupportedExpression {
                    module: module.to_string(),
                    expression: format!("non-literal map value {expr:?}"),
                })
            }
            _ => {}
        }
    }

    let (width, size) = match ty {
        ValueType::I32 => (StoreWidth::I32, 4u32),
        ValueType::I64 => (StoreWidth::I64, 8u32),
    };

    let mut stmts = Vec::new();
    let ptr = data_allocator.allocate(vec![0; size as usize]);
    stmts.push(IrStmt::Store {
        address: IrExpr::ConstI32(ptr as i32),
        value: value_expr,
        width,
    });

    Ok((
        stmts,
        IrExpr::ConstI32(ptr as i32),
        IrExpr::ConstI32(size as i32),
    ))
}

struct DataAllocator {
    next_offset: u32,
    segments: Vec<DataSegment>,
}

impl DataAllocator {
    fn new() -> Self {
        Self {
            next_offset: 0,
            segments: Vec::new(),
        }
    }

    fn allocate(&mut self, bytes: Vec<u8>) -> u32 {
        let offset = align_u32(self.next_offset, 4);
        self.next_offset = offset.saturating_add(bytes.len() as u32);
        self.segments.push(DataSegment { offset, bytes });
        offset
    }

    fn finish(self) -> Vec<DataSegment> {
        self.segments
    }
}

fn align_u32(value: u32, align: u32) -> u32 {
    if align == 0 {
        return value;
    }
    let remainder = value % align;
    if remainder == 0 {
        value
    } else {
        value + (align - remainder)
    }
}

fn map_type(_module: &str, ty: &DslType) -> Result<ValueType, ManifestCompileError> {
    match ty {
        DslType::Bool => Ok(ValueType::I32),
        DslType::Bytes | DslType::String | DslType::Address => Ok(ValueType::I32),
        DslType::Uint { .. } | DslType::Int { .. } => Ok(ValueType::I64),
        DslType::Optional(_)
        | DslType::List(_)
        | DslType::Map { .. }
        | DslType::Custom(_)
        | DslType::Any => Ok(ValueType::I32),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsl::{DslExpr, DslFunction, DslModule, DslParam, DslStateField, DslStmt, DslType};
    use crate::ir::{Expr as IrExpr, HostFunction, Stmt as IrStmt, StoreWidth, ValueType};

    fn empty_entrypoints() -> HashMap<(String, String), ()> {
        HashMap::new()
    }

    #[test]
    fn lowers_state_read_for_state_field() {
        let module = DslModule {
            name: "token".to_string(),
            state_fields: vec![DslStateField {
                name: "counter".to_string(),
                ty: DslType::Uint { bits: 64 },
            }],
            structs: Vec::new(),
            functions: vec![DslFunction {
                name: "get_counter".to_string(),
                params: Vec::new(),
                return_type: Some(DslType::Uint { bits: 64 }),
                body: vec![DslStmt::Return(Some(DslExpr::Identifier("counter".to_string())))],
            }],
        };

        let mut data_allocator = DataAllocator::new();
        let functions = lower_dsl_module("token", &module, &empty_entrypoints(), &mut data_allocator)
            .expect("lower module");
        let segments = data_allocator.finish();

        let body = match &functions[0].body {
            IrFunctionBody::Block { body, .. } => body,
            _ => panic!("expected block body"),
        };

        match body.last() {
            Some(IrStmt::Return { value: Some(IrExpr::StateRead { ty, .. }) }) => {
                assert_eq!(*ty, ValueType::I64);
            }
            other => panic!("unexpected return: {other:?}"),
        }

        let key_bytes = serialize::encode_string("counter");
        assert_eq!(segments.len(), 2);
        assert!(segments.iter().any(|segment| segment.bytes == key_bytes));
        assert!(segments.iter().any(|segment| segment.bytes == vec![0, 0, 0, 0]));
    }

    #[test]
    fn lowers_state_write_with_param_buffer() {
        let module = DslModule {
            name: "token".to_string(),
            state_fields: vec![DslStateField {
                name: "balance".to_string(),
                ty: DslType::Uint { bits: 64 },
            }],
            structs: Vec::new(),
            functions: vec![DslFunction {
                name: "set_balance".to_string(),
                params: vec![DslParam {
                    name: "amount".to_string(),
                    ty: Some(DslType::Uint { bits: 64 }),
                }],
                return_type: None,
                body: vec![DslStmt::Assign {
                    target: DslExpr::Identifier("balance".to_string()),
                    value: DslExpr::Identifier("amount".to_string()),
                }],
            }],
        };

        let mut data_allocator = DataAllocator::new();
        let functions = lower_dsl_module("token", &module, &empty_entrypoints(), &mut data_allocator)
            .expect("lower module");
        let segments = data_allocator.finish();

        let body = match &functions[0].body {
            IrFunctionBody::Block { body, .. } => body,
            _ => panic!("expected block body"),
        };
        assert_eq!(body.len(), 2);

        let (store_ptr, store_width) = match &body[0] {
            IrStmt::Store { address, value, width } => {
                assert_eq!(*width, StoreWidth::I64);
                assert!(matches!(value, IrExpr::Param { index: 0, ty: ValueType::I64 }));
                match address {
                    IrExpr::ConstI32(ptr) => (*ptr, *width),
                    _ => panic!("expected const store address"),
                }
            }
            other => panic!("unexpected first stmt: {other:?}"),
        };

        match &body[1] {
            IrStmt::Expr(IrExpr::HostCall { function, args }) => {
                assert_eq!(*function, HostFunction::StdStateWrite);
                assert_eq!(args.len(), 4);
                assert!(matches!(args[0], IrExpr::ConstI32(_)));
                assert!(matches!(args[1], IrExpr::ConstI32(_)));
                assert!(matches!(args[2], IrExpr::ConstI32(ptr) if ptr == store_ptr));
                assert!(matches!(args[3], IrExpr::ConstI32(8)));
            }
            other => panic!("unexpected host call stmt: {other:?}"),
        }

        let key_bytes = serialize::encode_string("balance");
        assert!(segments.iter().any(|segment| segment.bytes == key_bytes));
        assert!(segments.iter().any(|segment| segment.bytes == vec![0; 8]));
        assert_eq!(store_width, StoreWidth::I64);
    }
}

