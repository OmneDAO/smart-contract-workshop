//! Abstract syntax tree construction for pysub programs.

use std::fmt;

use pest::iterators::{Pair, Pairs};
use thiserror::Error;

use crate::parser::{ParseError, ParsedProgram, Rule};

#[derive(Debug, Error)]
pub enum AstError {
    #[error("parser error: {0}")]
    Parse(#[from] ParseError),

    #[error("unexpected rule `{found:?}` while parsing {context}")]
    UnexpectedRule { context: &'static str, found: Rule },

    #[error("missing child element while parsing {0}")]
    MissingChild(&'static str),

    #[error("invalid literal `{literal}`")]
    InvalidLiteral { literal: String },

    #[error("invalid type expression `{ty}`")]
    InvalidType { ty: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Ident(String);

impl Ident {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Ident {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrimitiveType {
    U128,
    Bool,
    Bytes,
    Address,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Type {
    Primitive(PrimitiveType),
    Map { key: Box<Type>, value: Box<Type> },
}

impl fmt::Display for Type {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Type::Primitive(PrimitiveType::U128) => write!(f, "u128"),
            Type::Primitive(PrimitiveType::Bool) => write!(f, "bool"),
            Type::Primitive(PrimitiveType::Bytes) => write!(f, "bytes"),
            Type::Primitive(PrimitiveType::Address) => write!(f, "address"),
            Type::Map { key, value } => write!(f, "map[{key}, {value}]"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Program {
    pub contracts: Vec<Contract>,
    pub functions: Vec<Function>,
}

impl Program {
    pub fn new() -> Self {
        Self {
            contracts: Vec::new(),
            functions: Vec::new(),
        }
    }
}

impl Default for Program {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone)]
pub struct Contract {
    pub name: Ident,
    pub params: Vec<Param>,
    pub storage: Vec<StorageField>,
    pub functions: Vec<Function>,
}

#[derive(Debug, Clone)]
pub struct StorageField {
    pub name: Ident,
    pub ty: Type,
    pub initializer: Option<Expression>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Visibility {
    Public,
    Private,
}

#[derive(Debug, Clone)]
pub struct Function {
    pub name: Ident,
    pub visibility: Visibility,
    pub params: Vec<Param>,
    pub return_type: Option<Type>,
    pub body: Vec<Statement>,
}

#[derive(Debug, Clone)]
pub struct Param {
    pub name: Ident,
    pub ty: Type,
}

#[derive(Debug, Clone)]
pub struct IfBranch {
    pub condition: Expression,
    pub body: Vec<Statement>,
}

#[derive(Debug, Clone)]
pub enum Statement {
    Let {
        mutable: bool,
        name: Ident,
        ty: Type,
        value: Expression,
    },
    Assign {
        target: Expression,
        value: Expression,
    },
    If {
        branches: Vec<IfBranch>,
        else_branch: Option<Vec<Statement>>,
    },
    While {
        condition: Expression,
        body: Vec<Statement>,
    },
    Return(Option<Expression>),
    Break,
    Continue,
    Pass,
    Expr(Expression),
}

#[derive(Debug, Clone)]
pub enum Expression {
    Identifier(Ident),
    Literal(Literal),
    Binary {
        left: Box<Expression>,
        op: BinaryOp,
        right: Box<Expression>,
    },
    Unary {
        op: UnaryOp,
        expr: Box<Expression>,
    },
    Call {
        callee: Box<Expression>,
        args: Vec<Expression>,
    },
    Index {
        target: Box<Expression>,
        index: Box<Expression>,
    },
    Attribute {
        target: Box<Expression>,
        attribute: Ident,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
    LogicalOr,
    LogicalAnd,
    Equal,
    NotEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    Add,
    Sub,
    Mul,
    Div,
    Mod,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    Neg,
    Not,
}

#[derive(Debug, Clone)]
pub enum Literal {
    Bool(bool),
    Number(String),
    Bytes(Vec<u8>),
    String(String),
    Address(String),
}

pub fn build_program(parsed: &ParsedProgram) -> Result<Program, AstError> {
    let mut program = Program::new();
    let pairs = parsed.pairs()?;

    for pair in pairs {
        match pair.as_rule() {
            Rule::program => {
                for child in pair.into_inner() {
                    match child.as_rule() {
                        Rule::contract_def => program.contracts.push(parse_contract(child)?),
                        Rule::function_def => program.functions.push(parse_function(child)?),
                        Rule::EOI => {}
                        other => {
                            return Err(AstError::UnexpectedRule {
                                context: "program",
                                found: other,
                            });
                        }
                    }
                }
            }
            Rule::EOI => {}
            other => {
                return Err(AstError::UnexpectedRule {
                    context: "program",
                    found: other,
                });
            }
        }
    }

    Ok(program)
}

fn parse_contract(pair: Pair<Rule>) -> Result<Contract, AstError> {
    let mut inner = pair.into_inner();

    let name_pair = inner
        .next()
        .ok_or(AstError::MissingChild("contract name"))?;
    let name = parse_identifier(name_pair)?;

    let mut params = Vec::new();
    let mut storage = Vec::new();
    let mut functions = Vec::new();

    let mut next = inner
        .next()
        .ok_or(AstError::MissingChild("contract block"))?;
    if next.as_rule() == Rule::contract_params {
        params = parse_contract_params(next)?;
        next = inner
            .next()
            .ok_or(AstError::MissingChild("contract block"))?;
    }

    if next.as_rule() != Rule::block_contract {
        return Err(AstError::UnexpectedRule {
            context: "contract",
            found: next.as_rule(),
        });
    }

    for item in next.into_inner() {
        match item.as_rule() {
            Rule::contract_item => {
                let mut inner_item = item.into_inner();
                if let Some(child) = inner_item.next() {
                    match child.as_rule() {
                        Rule::storage_decl => storage.push(parse_storage_decl(child)?),
                        Rule::function_def => functions.push(parse_function(child)?),
                        other => {
                            return Err(AstError::UnexpectedRule {
                                context: "contract item",
                                found: other,
                            });
                        }
                    }
                }
            }
            other => {
                return Err(AstError::UnexpectedRule {
                    context: "contract block",
                    found: other,
                });
            }
        }
    }

    Ok(Contract {
        name,
        params,
        storage,
        functions,
    })
}

fn parse_storage_decl(pair: Pair<Rule>) -> Result<StorageField, AstError> {
    let mut inner = pair.into_inner();
    let name = parse_identifier(inner.next().ok_or(AstError::MissingChild("storage name"))?)?;
    let ty = parse_type(inner.next().ok_or(AstError::MissingChild("storage type"))?)?;
    let initializer = if let Some(value) = inner.next() {
        let expr_pair = if value.as_rule() == Rule::storage_initializer {
            value
                .into_inner()
                .next()
                .ok_or(AstError::MissingChild("storage initializer"))?
        } else {
            value
        };
        Some(parse_expression(expr_pair)?)
    } else {
        None
    };

    Ok(StorageField {
        name,
        ty,
        initializer,
    })
}

fn parse_contract_params(pair: Pair<Rule>) -> Result<Vec<Param>, AstError> {
    let mut inner = pair.into_inner();
    if let Some(param_list) = inner.next() {
        parse_param_list(param_list)
    } else {
        Ok(Vec::new())
    }
}

fn parse_function(pair: Pair<Rule>) -> Result<Function, AstError> {
    let mut inner = pair.into_inner();

    let mut visibility = Visibility::Private;
    let mut name_pair = inner
        .next()
        .ok_or(AstError::MissingChild("function name"))?;

    if name_pair.as_rule() == Rule::visibility {
        visibility = Visibility::Public;
        name_pair = inner
            .next()
            .ok_or(AstError::MissingChild("function name"))?;
    }

    let name = parse_identifier(name_pair)?;

    let mut params = Vec::new();
    let mut next = inner
        .next()
        .ok_or(AstError::MissingChild("function body or params"))?;

    if next.as_rule() == Rule::param_list {
        params = parse_param_list_from_pairs(next.into_inner())?;
        next = inner
            .next()
            .ok_or(AstError::MissingChild("function body or return"))?;
    }

    let return_type = if next.as_rule() == Rule::return_type {
        let ty_pair = next
            .into_inner()
            .next()
            .ok_or(AstError::MissingChild("return type"))?;
        let ty = parse_type(ty_pair)?;
        next = inner
            .next()
            .ok_or(AstError::MissingChild("function body"))?;
        Some(ty)
    } else {
        None
    };

    if next.as_rule() != Rule::block_function {
        return Err(AstError::UnexpectedRule {
            context: "function block",
            found: next.as_rule(),
        });
    }

    let body = parse_block(next)?;

    Ok(Function {
        name,
        visibility,
        params,
        return_type,
        body,
    })
}

fn parse_param_list(pair: Pair<Rule>) -> Result<Vec<Param>, AstError> {
    parse_param_list_from_pairs(pair.into_inner())
}

fn parse_param_list_from_pairs(pairs: Pairs<Rule>) -> Result<Vec<Param>, AstError> {
    let mut params = Vec::new();

    for pair in pairs {
        if pair.as_rule() == Rule::param {
            let mut inner = pair.into_inner();
            let name = parse_identifier(inner.next().ok_or(AstError::MissingChild("param name"))?)?;
            let ty = parse_type(inner.next().ok_or(AstError::MissingChild("param type"))?)?;
            params.push(Param { name, ty });
        }
    }

    Ok(params)
}

fn parse_block(pair: Pair<Rule>) -> Result<Vec<Statement>, AstError> {
    let mut statements = Vec::new();
    for node in pair.into_inner() {
        match node.as_rule() {
            Rule::statement => {
                let stmt_pair = node
                    .into_inner()
                    .next()
                    .ok_or(AstError::MissingChild("statement"))?;
                statements.push(parse_statement(stmt_pair)?);
            }
            Rule::let_stmt
            | Rule::assign_stmt
            | Rule::if_stmt
            | Rule::while_stmt
            | Rule::return_stmt
            | Rule::break_stmt
            | Rule::continue_stmt
            | Rule::pass_stmt
            | Rule::expr_stmt => {
                statements.push(parse_statement(node)?);
            }
            Rule::newline => {}
            other => {
                return Err(AstError::UnexpectedRule {
                    context: "block",
                    found: other,
                });
            }
        }
    }
    Ok(statements)
}

fn parse_statement(pair: Pair<Rule>) -> Result<Statement, AstError> {
    match pair.as_rule() {
        Rule::let_stmt => parse_let_stmt(pair),
        Rule::assign_stmt => parse_assign_stmt(pair),
        Rule::if_stmt => parse_if_stmt(pair),
        Rule::while_stmt => parse_while_stmt(pair),
        Rule::return_stmt => parse_return_stmt(pair),
        Rule::break_stmt => Ok(Statement::Break),
        Rule::continue_stmt => Ok(Statement::Continue),
        Rule::pass_stmt => Ok(Statement::Pass),
        Rule::expr_stmt => {
            let expr_pair = pair
                .into_inner()
                .next()
                .ok_or(AstError::MissingChild("expression"))?;
            Ok(Statement::Expr(parse_expression(expr_pair)?))
        }
        other => Err(AstError::UnexpectedRule {
            context: "statement",
            found: other,
        }),
    }
}

fn parse_let_stmt(pair: Pair<Rule>) -> Result<Statement, AstError> {
    let mut inner = pair.into_inner();

    let mut mutable = false;
    let mut name_pair = inner.next().ok_or(AstError::MissingChild("binding name"))?;

    if name_pair.as_rule() == Rule::mutability {
        mutable = true;
        name_pair = inner.next().ok_or(AstError::MissingChild("binding name"))?;
    }

    let name = parse_identifier(name_pair)?;
    let ty = parse_type(inner.next().ok_or(AstError::MissingChild("binding type"))?)?;
    let value = parse_expression(
        inner
            .next()
            .ok_or(AstError::MissingChild("binding value"))?,
    )?;

    Ok(Statement::Let {
        mutable,
        name,
        ty,
        value,
    })
}

fn parse_postfix(pair: Pair<Rule>) -> Result<Expression, AstError> {
    let mut inner = pair.into_inner();
    let mut expr = parse_primary(
        inner
            .next()
            .ok_or(AstError::MissingChild("postfix primary"))?,
    )?;

    for next in inner {
        expr = apply_postfix(expr, next)?;
    }

    Ok(expr)
}

fn apply_postfix(expr: Expression, pair: Pair<Rule>) -> Result<Expression, AstError> {
    match pair.as_rule() {
        Rule::call_args => {
            let args = parse_call_args(pair)?;
            Ok(Expression::Call {
                callee: Box::new(expr),
                args,
            })
        }
        Rule::index_access => {
            let index_pair = pair
                .into_inner()
                .next()
                .ok_or(AstError::MissingChild("index"))?;
            let index = parse_expression(index_pair)?;
            Ok(Expression::Index {
                target: Box::new(expr),
                index: Box::new(index),
            })
        }
        Rule::attribute_access => {
            let ident_pair = pair
                .into_inner()
                .next()
                .ok_or(AstError::MissingChild("attribute"))?;
            let ident = parse_identifier(ident_pair)?;
            Ok(Expression::Attribute {
                target: Box::new(expr),
                attribute: ident,
            })
        }
        Rule::postfix_op => {
            let mut inner = pair.into_inner();
            let op = inner.next().ok_or(AstError::MissingChild("postfix op"))?;
            apply_postfix(expr, op)
        }
        Rule::newline => Ok(expr),
        other => Err(AstError::UnexpectedRule {
            context: "postfix",
            found: other,
        }),
    }
}

fn parse_call_args(pair: Pair<Rule>) -> Result<Vec<Expression>, AstError> {
    let mut args = Vec::new();
    if let Some(args_pair) = pair.into_inner().next() {
        if args_pair.as_rule() == Rule::arg_list {
            for arg in args_pair.into_inner() {
                if arg.as_rule() == Rule::expr {
                    args.push(parse_expression(arg)?);
                }
            }
        }
    }
    Ok(args)
}

fn parse_assign_stmt(pair: Pair<Rule>) -> Result<Statement, AstError> {
    let mut inner = pair.into_inner();
    let target_pair = inner
        .next()
        .ok_or(AstError::MissingChild("assignment target"))?;
    let value_pair = inner
        .next()
        .ok_or(AstError::MissingChild("assignment value"))?;

    let target = parse_assign_target(target_pair)?;
    let value = parse_expression(value_pair)?;

    Ok(Statement::Assign { target, value })
}

fn parse_if_stmt(pair: Pair<Rule>) -> Result<Statement, AstError> {
    let mut inner = pair.into_inner();

    let condition = parse_expression(inner.next().ok_or(AstError::MissingChild("if condition"))?)?;
    let first_body = parse_block(inner.next().ok_or(AstError::MissingChild("if body"))?)?;

    let mut branches = vec![IfBranch {
        condition,
        body: first_body,
    }];
    let mut else_branch = None;

    while let Some(next) = inner.next() {
        match next.as_rule() {
            Rule::expr => {
                let cond = parse_expression(next)?;
                let body_pair = inner.next().ok_or(AstError::MissingChild("elif body"))?;
                let body = parse_block(body_pair)?;
                branches.push(IfBranch {
                    condition: cond,
                    body,
                });
            }
            Rule::block_function => {
                else_branch = Some(parse_block(next)?);
            }
            other => {
                return Err(AstError::UnexpectedRule {
                    context: "if",
                    found: other,
                });
            }
        }
    }

    Ok(Statement::If {
        branches,
        else_branch,
    })
}

fn parse_while_stmt(pair: Pair<Rule>) -> Result<Statement, AstError> {
    let mut inner = pair.into_inner();
    let condition = parse_expression(
        inner
            .next()
            .ok_or(AstError::MissingChild("while condition"))?,
    )?;
    let body = parse_block(inner.next().ok_or(AstError::MissingChild("while body"))?)?;
    Ok(Statement::While { condition, body })
}

fn parse_return_stmt(pair: Pair<Rule>) -> Result<Statement, AstError> {
    let mut inner = pair.into_inner();
    if let Some(expr_pair) = inner.next() {
        Ok(Statement::Return(Some(parse_expression(expr_pair)?)))
    } else {
        Ok(Statement::Return(None))
    }
}

fn parse_assign_target(pair: Pair<Rule>) -> Result<Expression, AstError> {
    if pair.as_rule() != Rule::assign_target {
        return Err(AstError::UnexpectedRule {
            context: "assign target",
            found: pair.as_rule(),
        });
    }

    let mut inner = pair.into_inner();
    let base = parse_identifier(inner.next().ok_or(AstError::MissingChild("assign base"))?)?;
    let mut expr = Expression::Identifier(base);

    for suffix in inner {
        let mut parts = suffix.into_inner();
        let child = parts
            .next()
            .ok_or(AstError::MissingChild("assign suffix"))?;
        match child.as_rule() {
            Rule::identifier => {
                let ident = parse_identifier(child)?;
                expr = Expression::Attribute {
                    target: Box::new(expr),
                    attribute: ident,
                };
            }
            // Allow any expression rule here because assign_suffix is silent, so the
            // inner pair may arrive as `expr` or a lowered expression rule (e.g. logical_or).
            _ => {
                let index = parse_expression(child)?;
                expr = Expression::Index {
                    target: Box::new(expr),
                    index: Box::new(index),
                };
            }
        }
    }

    Ok(expr)
}

fn parse_expression(pair: Pair<Rule>) -> Result<Expression, AstError> {
    match pair.as_rule() {
        Rule::expr => parse_expression(
            pair.into_inner()
                .next()
                .ok_or(AstError::MissingChild("expr"))?,
        ),
        Rule::logical_or => parse_logical_or(pair),
        Rule::logical_and => parse_logical_and(pair),
        Rule::equality => parse_equality(pair),
        Rule::comparison => parse_comparison(pair),
        Rule::additive => parse_additive(pair),
        Rule::multiplicative => parse_multiplicative(pair),
        Rule::unary => parse_unary(pair),
        Rule::postfix => parse_postfix(pair),
        Rule::primary => parse_primary(pair),
        other => Err(AstError::UnexpectedRule {
            context: "expression",
            found: other,
        }),
    }
}

fn parse_logical_or(pair: Pair<Rule>) -> Result<Expression, AstError> {
    parse_binary_expression(pair, parse_logical_and, parse_logical_or_op)
}

fn parse_logical_and(pair: Pair<Rule>) -> Result<Expression, AstError> {
    parse_binary_expression(pair, parse_equality, parse_logical_and_op)
}

fn parse_equality(pair: Pair<Rule>) -> Result<Expression, AstError> {
    parse_binary_expression(pair, parse_comparison, parse_equality_op)
}

fn parse_comparison(pair: Pair<Rule>) -> Result<Expression, AstError> {
    parse_binary_expression(pair, parse_additive, parse_comparison_op)
}

fn parse_additive(pair: Pair<Rule>) -> Result<Expression, AstError> {
    parse_binary_expression(pair, parse_multiplicative, parse_additive_op)
}

fn parse_multiplicative(pair: Pair<Rule>) -> Result<Expression, AstError> {
    parse_binary_expression(pair, parse_unary, parse_multiplicative_op)
}

fn parse_binary_expression(
    pair: Pair<Rule>,
    next: fn(Pair<Rule>) -> Result<Expression, AstError>,
    op_parser: fn(&str) -> Result<BinaryOp, AstError>,
) -> Result<Expression, AstError> {
    let mut inner = pair.into_inner();
    let mut expr = next(inner.next().ok_or(AstError::MissingChild("binary lhs"))?)?;

    while let Some(op_pair) = inner.next() {
        let op = op_parser(op_pair.as_str())?;
        let rhs_pair = inner.next().ok_or(AstError::MissingChild("binary rhs"))?;
        let rhs = next(rhs_pair)?;
        expr = Expression::Binary {
            left: Box::new(expr),
            op,
            right: Box::new(rhs),
        };
    }

    Ok(expr)
}

fn parse_logical_or_op(symbol: &str) -> Result<BinaryOp, AstError> {
    match symbol.trim() {
        "or" => Ok(BinaryOp::LogicalOr),
        other => Err(AstError::InvalidLiteral {
            literal: other.into(),
        }),
    }
}

fn parse_logical_and_op(symbol: &str) -> Result<BinaryOp, AstError> {
    match symbol.trim() {
        "and" => Ok(BinaryOp::LogicalAnd),
        other => Err(AstError::InvalidLiteral {
            literal: other.into(),
        }),
    }
}

fn parse_equality_op(symbol: &str) -> Result<BinaryOp, AstError> {
    match symbol.trim() {
        "==" => Ok(BinaryOp::Equal),
        "!=" => Ok(BinaryOp::NotEqual),
        other => Err(AstError::InvalidLiteral {
            literal: other.into(),
        }),
    }
}

fn parse_comparison_op(symbol: &str) -> Result<BinaryOp, AstError> {
    match symbol.trim() {
        "<" => Ok(BinaryOp::Less),
        "<=" => Ok(BinaryOp::LessEqual),
        ">" => Ok(BinaryOp::Greater),
        ">=" => Ok(BinaryOp::GreaterEqual),
        other => Err(AstError::InvalidLiteral {
            literal: other.into(),
        }),
    }
}

fn parse_additive_op(symbol: &str) -> Result<BinaryOp, AstError> {
    match symbol.trim() {
        "+" => Ok(BinaryOp::Add),
        "-" => Ok(BinaryOp::Sub),
        other => Err(AstError::InvalidLiteral {
            literal: other.into(),
        }),
    }
}

fn parse_multiplicative_op(symbol: &str) -> Result<BinaryOp, AstError> {
    match symbol.trim() {
        "*" => Ok(BinaryOp::Mul),
        "/" => Ok(BinaryOp::Div),
        "%" => Ok(BinaryOp::Mod),
        other => Err(AstError::InvalidLiteral {
            literal: other.into(),
        }),
    }
}

fn parse_unary(pair: Pair<Rule>) -> Result<Expression, AstError> {
    let mut inner = pair.into_inner();
    let first = inner.next().ok_or(AstError::MissingChild("unary"))?;

    match first.as_rule() {
        Rule::postfix => parse_postfix(first),
        _ => {
            let op = match first.as_str().trim() {
                "-" => UnaryOp::Neg,
                "not" => UnaryOp::Not,
                other => {
                    return Err(AstError::InvalidLiteral {
                        literal: other.into(),
                    })
                }
            };
            let expr_pair = inner.next().ok_or(AstError::MissingChild("unary expr"))?;
            Ok(Expression::Unary {
                op,
                expr: Box::new(parse_unary(expr_pair)?),
            })
        }
    }
}

fn parse_primary(pair: Pair<Rule>) -> Result<Expression, AstError> {
    let mut inner = pair.into_inner();
    let node = inner.next().ok_or(AstError::MissingChild("primary"))?;
    match node.as_rule() {
        Rule::identifier => Ok(Expression::Identifier(parse_identifier(node)?)),
        Rule::expr => parse_expression(node),
        Rule::literal
        | Rule::bool_literal
        | Rule::number_literal
        | Rule::string_literal
        | Rule::bytes_literal
        | Rule::address_literal => parse_literal(node).map(Expression::Literal),
        other => Err(AstError::UnexpectedRule {
            context: "primary",
            found: other,
        }),
    }
}

fn parse_literal(pair: Pair<Rule>) -> Result<Literal, AstError> {
    match pair.as_rule() {
        Rule::literal => {
            let value = pair
                .into_inner()
                .next()
                .ok_or(AstError::MissingChild("literal"))?;
            parse_literal(value)
        }
        Rule::bool_literal => Ok(Literal::Bool(matches!(pair.as_str(), "true"))),
        Rule::number_literal => Ok(Literal::Number(pair.as_str().trim().into())),
        Rule::string_literal => Ok(Literal::String(unescape_string(pair.as_str())?)),
        Rule::bytes_literal => Ok(Literal::Bytes(unescape_bytes(pair.as_str())?)),
        Rule::address_literal => Ok(Literal::Address(pair.as_str().trim().into())),
        other => Err(AstError::UnexpectedRule {
            context: "literal",
            found: other,
        }),
    }
}

fn parse_type(pair: Pair<Rule>) -> Result<Type, AstError> {
    match pair.as_rule() {
        Rule::ty => parse_type(
            pair.into_inner()
                .next()
                .ok_or(AstError::MissingChild("type"))?,
        ),
        Rule::primitive_type => match pair.as_str() {
            "u128" => Ok(Type::Primitive(PrimitiveType::U128)),
            "bool" => Ok(Type::Primitive(PrimitiveType::Bool)),
            "bytes" => Ok(Type::Primitive(PrimitiveType::Bytes)),
            "address" => Ok(Type::Primitive(PrimitiveType::Address)),
            other => Err(AstError::InvalidType { ty: other.into() }),
        },
        Rule::map_type => {
            let mut inner = pair.into_inner();
            let key = parse_type(inner.next().ok_or(AstError::MissingChild("map key"))?)?;
            let value = parse_type(inner.next().ok_or(AstError::MissingChild("map value"))?)?;
            Ok(Type::Map {
                key: Box::new(key),
                value: Box::new(value),
            })
        }
        other => Err(AstError::UnexpectedRule {
            context: "type",
            found: other,
        }),
    }
}

fn parse_identifier(pair: Pair<Rule>) -> Result<Ident, AstError> {
    if pair.as_rule() != Rule::identifier {
        return Err(AstError::UnexpectedRule {
            context: "identifier",
            found: pair.as_rule(),
        });
    }
    Ok(Ident::new(pair.as_str().trim()))
}

fn unescape_string(input: &str) -> Result<String, AstError> {
    let mut result = String::new();
    let mut chars = input.chars();
    chars.next();
    while let Some(ch) = chars.next() {
        if ch == '"' {
            break;
        }
        if ch == '\\' {
            let next = chars.next().ok_or_else(|| AstError::InvalidLiteral {
                literal: input.into(),
            })?;
            result.push(match next {
                'n' => '\n',
                'r' => '\r',
                't' => '\t',
                '0' => '\0',
                '\\' => '\\',
                '"' => '"',
                other => {
                    return Err(AstError::InvalidLiteral {
                        literal: other.to_string(),
                    })
                }
            });
        } else {
            result.push(ch);
        }
    }
    Ok(result)
}

fn unescape_bytes(input: &str) -> Result<Vec<u8>, AstError> {
    if !input.starts_with("b\"") || !input.ends_with('"') {
        return Err(AstError::InvalidLiteral {
            literal: input.into(),
        });
    }
    let mut result = Vec::new();
    let mut chars = input[2..].chars();
    while let Some(ch) = chars.next() {
        if ch == '"' {
            break;
        }
        if ch == '\\' {
            let next = chars.next().ok_or_else(|| AstError::InvalidLiteral {
                literal: input.into(),
            })?;
            result.push(match next {
                'n' => b'\n',
                'r' => b'\r',
                't' => b'\t',
                '0' => b'\0',
                '\\' => b'\\',
                '"' => b'"',
                other => {
                    return Err(AstError::InvalidLiteral {
                        literal: other.to_string(),
                    })
                }
            });
        } else {
            result.push(ch as u8);
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_program;

    #[test]
    fn parses_contract_and_function() {
        let src = "contract Sample:\n    storage counter: u128 = 0\n\n    fn increment():\n        counter = counter + 1\n\nfn helper(x: u128) -> u128:\n    return x\n";

        let parsed = parse_program(src).expect("parse");
        let program = build_program(&parsed).expect("build");
        assert_eq!(program.contracts.len(), 1);
        assert_eq!(program.functions.len(), 1);
        let contract = &program.contracts[0];
        assert_eq!(contract.storage.len(), 1);
        assert_eq!(contract.functions.len(), 1);
    }
}
