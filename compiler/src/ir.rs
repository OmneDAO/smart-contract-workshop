//! Intermediate representation helpers for pysub compiler.

use std::collections::HashMap;

use thiserror::Error;

use crate::ast::{
    BinaryOp as AstBinaryOp, Expression, Function as AstFunction, Literal, PrimitiveType, Program,
    Statement, Type,
};

#[derive(Debug, Error)]
pub enum IrError {
    #[error("unsupported statement `{kind}` in function `{function}`")]
    UnsupportedStatement {
        function: String,
        kind: &'static str,
    },

    #[error("function `{function}` is missing a return statement")]
    MissingReturn { function: String },

    #[error("function `{function}` must return a value")]
    MissingReturnValue { function: String },

    #[error("function `{function}` should not return a value")]
    UnexpectedReturnValue { function: String },

    #[error("unknown identifier `{identifier}` in function `{function}`")]
    UnknownIdentifier {
        function: String,
        identifier: String,
    },

    #[error("literal `{literal}` is out of range in function `{function}`")]
    LiteralOutOfRange { function: String, literal: String },

    #[error("unsupported binary operator `{op:?}` in function `{function}`")]
    UnsupportedBinary { function: String, op: AstBinaryOp },

    #[error("type mismatch in function `{function}`: expected `{expected}`, found `{found}`")]
    TypeMismatch {
        function: String,
        expected: ValueType,
        found: ValueType,
    },

    #[error("unsupported expression in function `{function}`")]
    UnsupportedExpression { function: String },
}

#[derive(Debug, Clone, Default)]
pub struct Module {
    pub contracts: Vec<Contract>,
    pub functions: Vec<Function>,
}

impl Module {
    pub fn total_function_count(&self) -> usize {
        let contract_fn_total: usize = self
            .contracts
            .iter()
            .map(|contract| contract.functions.len())
            .sum();
        self.functions.len() + contract_fn_total
    }
}

#[derive(Debug, Clone)]
pub struct Contract {
    pub name: String,
    pub params: Vec<Param>,
    pub storage: Vec<Field>,
    pub functions: Vec<Function>,
}

#[derive(Debug, Clone)]
pub struct Field {
    pub name: String,
    pub ty: ValueType,
}

#[derive(Debug, Clone)]
pub struct Function {
    pub name: String,
    pub params: Vec<Param>,
    pub return_type: Option<ValueType>,
    pub body: FunctionBody,
}

#[derive(Debug, Clone)]
pub struct Param {
    pub name: String,
    pub ty: ValueType,
}

#[derive(Debug, Clone)]
pub enum FunctionBody {
    Return { value: Option<Expr> },
}

#[derive(Debug, Clone)]
pub enum Expr {
    Param {
        index: u32,
        ty: ValueType,
    },
    ConstI32(i32),
    ConstI64(i64),
    Binary {
        op: BinaryOp,
        left: Box<Expr>,
        right: Box<Expr>,
        ty: ValueType,
    },
}

impl Expr {
    pub fn value_type(&self) -> ValueType {
        match self {
            Expr::Param { ty, .. } => *ty,
            Expr::ConstI32(_) => ValueType::I32,
            Expr::ConstI64(_) => ValueType::I64,
            Expr::Binary { ty, .. } => *ty,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    DivUInt,
    RemUInt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueType {
    I32,
    I64,
}

impl std::fmt::Display for ValueType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ValueType::I32 => write!(f, "i32"),
            ValueType::I64 => write!(f, "i64"),
        }
    }
}

pub fn lower_to_ir(program: &Program) -> Result<Module, IrError> {
    Ok(Module {
        functions: program
            .functions
            .iter()
            .map(|function| lower_function(function, None))
            .collect::<Result<_, _>>()?,
        contracts: program
            .contracts
            .iter()
            .map(|contract| {
                Ok(Contract {
                    name: contract.name.as_str().to_owned(),
                    params: convert_params(&contract.params),
                    storage: contract
                        .storage
                        .iter()
                        .map(|field| Field {
                            name: field.name.as_str().to_owned(),
                            ty: convert_type(&field.ty),
                        })
                        .collect(),
                    functions: contract
                        .functions
                        .iter()
                        .map(|function| lower_function(function, Some(contract.name.as_str())))
                        .collect::<Result<_, _>>()?,
                })
            })
            .collect::<Result<_, _>>()?,
    })
}

fn lower_function(function: &AstFunction, contract: Option<&str>) -> Result<Function, IrError> {
    let mut params = Vec::new();
    let mut param_lookup = HashMap::new();
    for (index, param) in function.params.iter().enumerate() {
        let ty = convert_type(&param.ty);
        params.push(Param {
            name: param.name.as_str().to_owned(),
            ty,
        });
        param_lookup.insert(param.name.as_str().to_owned(), (index as u32, ty));
    }

    let function_name = if let Some(contract_name) = contract {
        format!("{}::{}", contract_name, function.name)
    } else {
        function.name.as_str().to_owned()
    };

    let return_type = function.return_type.as_ref().map(convert_type);
    let body = lower_statements(&function.body, &param_lookup, return_type, &function_name)?;

    Ok(Function {
        name: function.name.as_str().to_owned(),
        params,
        return_type,
        body,
    })
}

fn lower_statements(
    statements: &[Statement],
    params: &HashMap<String, (u32, ValueType)>,
    return_type: Option<ValueType>,
    function_name: &str,
) -> Result<FunctionBody, IrError> {
    let mut return_value: Option<Option<Expr>> = None;

    for statement in statements {
        match statement {
            Statement::Return(expr) => {
                let lowered = match expr {
                    Some(expr) => {
                        let lowered = lower_expression(expr, params, function_name)?;
                        Some(lowered)
                    }
                    None => None,
                };
                return_value = Some(lowered);
                break;
            }
            Statement::Pass => {}
            Statement::Expr(_) => {}
            other => {
                return Err(IrError::UnsupportedStatement {
                    function: function_name.to_owned(),
                    kind: statement_kind(other),
                });
            }
        }
    }

    match (return_type, return_value) {
        (Some(expected), Some(Some(expr))) => {
            let found = expr.value_type();
            if found != expected {
                return Err(IrError::TypeMismatch {
                    function: function_name.to_owned(),
                    expected,
                    found,
                });
            }
            Ok(FunctionBody::Return { value: Some(expr) })
        }
        (Some(_), Some(None)) => Err(IrError::MissingReturnValue {
            function: function_name.to_owned(),
        }),
        (Some(_), None) => Err(IrError::MissingReturn {
            function: function_name.to_owned(),
        }),
        (None, Some(Some(_))) => Err(IrError::UnexpectedReturnValue {
            function: function_name.to_owned(),
        }),
        (None, Some(None)) | (None, None) => Ok(FunctionBody::Return { value: None }),
    }
}

fn lower_expression(
    expr: &Expression,
    params: &HashMap<String, (u32, ValueType)>,
    function_name: &str,
) -> Result<Expr, IrError> {
    match expr {
        Expression::Identifier(ident) => {
            let Some((index, ty)) = params.get(ident.as_str()) else {
                return Err(IrError::UnknownIdentifier {
                    function: function_name.to_owned(),
                    identifier: ident.as_str().to_owned(),
                });
            };
            Ok(Expr::Param {
                index: *index,
                ty: *ty,
            })
        }
        Expression::Literal(literal) => lower_literal(literal, function_name),
        Expression::Binary { left, op, right } => {
            let left_expr = lower_expression(left, params, function_name)?;
            let right_expr = lower_expression(right, params, function_name)?;
            let left_ty = left_expr.value_type();
            let right_ty = right_expr.value_type();
            if left_ty != right_ty {
                return Err(IrError::TypeMismatch {
                    function: function_name.to_owned(),
                    expected: left_ty,
                    found: right_ty,
                });
            }

            let op = match op {
                AstBinaryOp::Add => BinaryOp::Add,
                AstBinaryOp::Sub => BinaryOp::Sub,
                AstBinaryOp::Mul => BinaryOp::Mul,
                AstBinaryOp::Div => BinaryOp::DivUInt,
                AstBinaryOp::Mod => BinaryOp::RemUInt,
                other => {
                    return Err(IrError::UnsupportedBinary {
                        function: function_name.to_owned(),
                        op: *other,
                    })
                }
            };

            Ok(Expr::Binary {
                op,
                left: Box::new(left_expr),
                right: Box::new(right_expr),
                ty: left_ty,
            })
        }
        _ => Err(IrError::UnsupportedExpression {
            function: function_name.to_owned(),
        }),
    }
}

fn lower_literal(literal: &Literal, function_name: &str) -> Result<Expr, IrError> {
    match literal {
        Literal::Bool(value) => Ok(Expr::ConstI32(if *value { 1 } else { 0 })),
        Literal::Number(raw) => {
            let value = if let Some(hex) = raw.strip_prefix("0x") {
                u64::from_str_radix(hex, 16).map_err(|_| IrError::LiteralOutOfRange {
                    function: function_name.to_owned(),
                    literal: raw.clone(),
                })?
            } else {
                raw.parse::<u64>().map_err(|_| IrError::LiteralOutOfRange {
                    function: function_name.to_owned(),
                    literal: raw.clone(),
                })?
            };
            if value > i64::MAX as u64 {
                return Err(IrError::LiteralOutOfRange {
                    function: function_name.to_owned(),
                    literal: raw.clone(),
                });
            }
            Ok(Expr::ConstI64(value as i64))
        }
        _ => Err(IrError::UnsupportedExpression {
            function: function_name.to_owned(),
        }),
    }
}

fn convert_params(params: &[crate::ast::Param]) -> Vec<Param> {
    params
        .iter()
        .map(|param| Param {
            name: param.name.as_str().to_owned(),
            ty: convert_type(&param.ty),
        })
        .collect()
}

fn convert_type(ty: &Type) -> ValueType {
    match ty {
        Type::Primitive(PrimitiveType::U128) => ValueType::I64,
        Type::Primitive(PrimitiveType::Bool)
        | Type::Primitive(PrimitiveType::Bytes)
        | Type::Primitive(PrimitiveType::Address) => ValueType::I32,
        Type::Map { .. } => ValueType::I32,
    }
}

fn statement_kind(statement: &Statement) -> &'static str {
    match statement {
        Statement::Let { .. } => "let",
        Statement::Assign { .. } => "assign",
        Statement::If { .. } => "if",
        Statement::While { .. } => "while",
        Statement::Break => "break",
        Statement::Continue => "continue",
        Statement::Pass => "pass",
        Statement::Return(_) => "return",
        Statement::Expr(_) => "expr",
    }
}
