//! Parser for the YAML-style pysub DSL used in Blox contracts.
//!
//! This parser is intentionally strict and line-oriented. It focuses on
//! module-level sections plus function bodies with indentation-sensitive blocks.

use std::fmt;

use thiserror::Error;

#[derive(Debug, Clone)]
pub struct DslModule {
    pub name: String,
    pub state_fields: Vec<DslStateField>,
    pub structs: Vec<DslStruct>,
    pub functions: Vec<DslFunction>,
}

#[derive(Debug, Clone)]
pub struct DslStruct {
    pub name: String,
    pub fields: Vec<DslStructField>,
}

#[derive(Debug, Clone)]
pub struct DslStructField {
    pub name: String,
    pub ty: DslType,
}

#[derive(Debug, Clone)]
pub struct DslStateField {
    pub name: String,
    pub ty: DslType,
}

#[derive(Debug, Clone)]
pub struct DslFunction {
    pub name: String,
    pub params: Vec<DslParam>,
    pub return_type: Option<DslType>,
    pub body: Vec<DslStmt>,
}

#[derive(Debug, Clone)]
pub struct DslParam {
    pub name: String,
    pub ty: Option<DslType>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DslType {
    Bool,
    Bytes,
    String,
    Address,
    Uint { bits: u16 },
    Int { bits: u16 },
    Optional(Box<DslType>),
    List(Box<DslType>),
    Map { key: Box<DslType>, value: Box<DslType> },
    Any,
    Custom(String),
}

#[derive(Debug, Clone)]
pub enum DslStmt {
    Let {
        name: String,
        ty: Option<DslType>,
        value: DslExpr,
    },
    Assign {
        target: DslExpr,
        value: DslExpr,
    },
    If {
        branches: Vec<DslIfBranch>,
        else_branch: Option<Vec<DslStmt>>,
    },
    While {
        condition: DslExpr,
        body: Vec<DslStmt>,
    },
    For {
        binding: String,
        iterable: DslExpr,
        body: Vec<DslStmt>,
    },
    Return(Option<DslExpr>),
    Raise(DslExpr),
    Break,
    Continue,
    Pass,
    Expr(DslExpr),
}

#[derive(Debug, Clone)]
pub struct DslIfBranch {
    pub condition: DslExpr,
    pub body: Vec<DslStmt>,
}

#[derive(Debug, Clone)]
pub enum DslExpr {
    Identifier(String),
    Literal(DslLiteral),
    ListLiteral(Vec<DslExpr>),
    MapLiteral(Vec<(DslExpr, DslExpr)>),
    Binary {
        left: Box<DslExpr>,
        op: DslBinaryOp,
        right: Box<DslExpr>,
    },
    Unary {
        op: DslUnaryOp,
        expr: Box<DslExpr>,
    },
    Call {
        callee: Box<DslExpr>,
        args: Vec<DslCallArg>,
    },
    Index {
        target: Box<DslExpr>,
        index: Box<DslExpr>,
    },
    Attribute {
        target: Box<DslExpr>,
        attribute: String,
    },
}

#[derive(Debug, Clone)]
pub enum DslLiteral {
    Bool(bool),
    Number(String),
    Bytes(Vec<u8>),
    String(String),
    None,
}

#[derive(Debug, Clone)]
pub enum DslCallArg {
    Positional(DslExpr),
    Named { name: String, value: DslExpr },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DslBinaryOp {
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
pub enum DslUnaryOp {
    Neg,
    Not,
}

#[derive(Debug, Error)]
pub enum DslParseError {
    #[error("missing module name")]
    MissingModule,

    #[error("unexpected top-level content: {0}")]
    UnexpectedTopLevel(String),

    #[error("invalid function signature `{signature}`: {reason}")]
    InvalidSignature { signature: String, reason: String },

    #[error("statement parse error: {0}")]
    Statement(String),

    #[error("expression parse error: {0}")]
    Expression(String),
}

pub fn parse_module(contents: &str) -> Result<DslModule, DslParseError> {
    let mut module_name = None;
    let mut state_fields = Vec::new();
    let mut structs = Vec::new();
    let mut functions: Vec<DslFunction> = Vec::new();

    let mut lines = contents.lines().enumerate().peekable();
    let mut in_functions = false;
    let mut in_state = false;
    let mut in_structs = false;

    while let Some((line_no, raw_line)) = lines.next() {
        let trimmed_end = raw_line.trim_end();
        if trimmed_end.is_empty() {
            continue;
        }
        let trimmed_start = trimmed_end.trim_start();
        if trimmed_start.starts_with('#') {
            continue;
        }

        let indent = raw_line.chars().take_while(|c| *c == ' ').count();
        let is_top_level = indent == 0;

        if is_top_level && trimmed_start.starts_with("module:") {
            module_name = Some(trimmed_start.trim_start_matches("module:").trim().to_string());
            continue;
        }

        if is_top_level && trimmed_start.starts_with("state:") {
            in_state = true;
            in_functions = false;
            in_structs = false;
            continue;
        }

        if is_top_level && trimmed_start.starts_with("structs:") {
            in_structs = true;
            in_state = false;
            in_functions = false;
            continue;
        }

        if is_top_level && trimmed_start.starts_with("functions:") {
            in_functions = true;
            in_state = false;
            in_structs = false;
            continue;
        }

        if in_structs {
            if trimmed_start.ends_with(':') && !trimmed_start.contains('(') {
                let struct_name = trimmed_start.trim_end_matches(':').trim();
                if struct_name.is_empty() {
                    return Err(DslParseError::Statement("invalid struct name".to_string()));
                }
                let fields = collect_struct_fields(&mut lines);
                let parsed_fields = parse_struct_fields(fields)?;
                structs.push(DslStruct {
                    name: struct_name.to_string(),
                    fields: parsed_fields,
                });
                continue;
            }
            if is_top_level {
                in_structs = false;
            }
        }

        if in_state {
            let name_type = trimmed_start
                .split_once(':')
                .ok_or_else(|| DslParseError::Statement(format!(
                    "invalid state entry: {trimmed_start}"
                )))?;
            let name = name_type.0.trim();
            let ty = name_type.1.trim();
            if name.is_empty() || ty.is_empty() {
                return Err(DslParseError::Statement(format!(
                    "invalid state entry: {trimmed_start}"
                )));
            }
            state_fields.push(DslStateField {
                name: name.to_string(),
                ty: parse_type(ty)?,
            });
            continue;
        }

        if is_top_level && in_functions {
            if trimmed_start.ends_with(':') && !trimmed_start.contains('(') {
                in_functions = false;
                continue;
            }

            let (signature, body_lines) = collect_signature_and_body(line_no, raw_line, &mut lines);
            let function = parse_function(signature, body_lines)?;
            functions.push(function);
            continue;
        }

        if is_top_level
            && module_name.is_none()
            && !trimmed_start.starts_with("imports:")
            && !trimmed_start.starts_with("state:")
        {
            return Err(DslParseError::UnexpectedTopLevel(trimmed_start.to_string()));
        }
    }

    let name = module_name.ok_or(DslParseError::MissingModule)?;
    Ok(DslModule {
        name,
        state_fields,
        structs,
        functions,
    })
}

fn collect_struct_fields<'a>(
    lines: &mut std::iter::Peekable<impl Iterator<Item = (usize, &'a str)>>,
) -> Vec<(usize, String)> {
    let mut field_lines = Vec::new();
    while let Some((next_no, next_line)) = lines.peek().cloned() {
        let trimmed = next_line.trim_end();
        if trimmed.is_empty() {
            lines.next();
            continue;
        }
        let indent = next_line.chars().take_while(|c| *c == ' ').count();
        if indent == 0 {
            break;
        }
        field_lines.push((next_no, next_line.to_string()));
        lines.next();
    }
    field_lines
}

fn parse_struct_fields(
    lines: Vec<(usize, String)>,
) -> Result<Vec<DslStructField>, DslParseError> {
    let mut fields = Vec::new();
    for (line_no, line) in lines {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let (name, ty) = trimmed
            .split_once(':')
            .ok_or_else(|| DslParseError::Statement(format!(
                "line {}: invalid struct field",
                line_no + 1
            )))?;
        let name = name.trim();
        let ty = ty.trim();
        if name.is_empty() || ty.is_empty() {
            return Err(DslParseError::Statement(format!(
                "line {}: invalid struct field",
                line_no + 1
            )));
        }
        fields.push(DslStructField {
            name: name.to_string(),
            ty: parse_type(ty)?,
        });
    }
    Ok(fields)
}

fn collect_signature_and_body<'a>(
    _line_no: usize,
    first_line: &'a str,
    lines: &mut std::iter::Peekable<impl Iterator<Item = (usize, &'a str)>>,
) -> (String, Vec<(usize, String)>) {
    let mut signature = first_line.trim().to_string();
    let mut body_lines = Vec::new();

    while !signature.trim_end().ends_with(':') || !signature.contains(')') {
        if let Some((_, next_line)) = lines.next() {
            signature.push(' ');
            signature.push_str(next_line.trim());
        } else {
            break;
        }
    }

    while let Some((next_no, next_line)) = lines.peek().cloned() {
        let trimmed = next_line.trim_end();
        if trimmed.is_empty() {
            lines.next();
            continue;
        }

        let indent = next_line.chars().take_while(|c| *c == ' ').count();
        if indent == 0 {
            break;
        }
        body_lines.push((next_no, next_line.to_string()));
        lines.next();
    }

    (format!("{}", signature), body_lines)
}

fn parse_function(
    signature: String,
    body_lines: Vec<(usize, String)>,
) -> Result<DslFunction, DslParseError> {
    let signature = signature.trim();
    let signature = signature
        .strip_suffix(':')
        .ok_or_else(|| DslParseError::InvalidSignature {
            signature: signature.to_string(),
            reason: "missing ':'".to_string(),
        })?;

    let (head, return_type) = if let Some((left, right)) = signature.split_once("->") {
        (left.trim(), Some(parse_type(right.trim())?))
    } else {
        (signature, None)
    };

    let open_paren = head.find('(').ok_or_else(|| DslParseError::InvalidSignature {
        signature: signature.to_string(),
        reason: "missing '('".to_string(),
    })?;
    let close_paren = head.rfind(')').ok_or_else(|| DslParseError::InvalidSignature {
        signature: signature.to_string(),
        reason: "missing ')'".to_string(),
    })?;

    let mut name = head[..open_paren].trim();
    if let Some(stripped) = name.strip_prefix("pub ") {
        name = stripped.trim_start();
    }
    if let Some(stripped) = name.strip_prefix("fn ") {
        name = stripped.trim_start();
    }
    if name.is_empty() {
        return Err(DslParseError::InvalidSignature {
            signature: signature.to_string(),
            reason: "missing function name".to_string(),
        });
    }

    let params_raw = head[open_paren + 1..close_paren].trim();
    let params = parse_params(params_raw)?;
    let body = parse_block(body_lines)?;

    Ok(DslFunction {
        name: name.to_string(),
        params,
        return_type,
        body,
    })
}

fn parse_params(params_raw: &str) -> Result<Vec<DslParam>, DslParseError> {
    if params_raw.is_empty() {
        return Ok(Vec::new());
    }

    let mut params = Vec::new();
    for raw in split_params(params_raw) {
        let param = raw.trim();
        if param.is_empty() {
            continue;
        }
        let (name, ty) = if let Some((left, right)) = param.split_once(':') {
            (left.trim(), Some(parse_type(right.trim())?))
        } else {
            (param, None)
        };
        if name.is_empty() {
            return Err(DslParseError::InvalidSignature {
                signature: params_raw.to_string(),
                reason: "parameter missing name".to_string(),
            });
        }
        params.push(DslParam {
            name: name.to_string(),
            ty,
        });
    }

    Ok(params)
}

fn split_params(params_raw: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut angle_depth = 0usize;
    let mut bracket_depth = 0usize;
    let mut paren_depth = 0usize;

    for ch in params_raw.chars() {
        match ch {
            '<' => angle_depth += 1,
            '>' => angle_depth = angle_depth.saturating_sub(1),
            '[' => bracket_depth += 1,
            ']' => bracket_depth = bracket_depth.saturating_sub(1),
            '(' => paren_depth += 1,
            ')' => paren_depth = paren_depth.saturating_sub(1),
            ',' if angle_depth == 0 && bracket_depth == 0 && paren_depth == 0 => {
                parts.push(current.trim().to_string());
                current.clear();
                continue;
            }
            _ => {}
        }
        current.push(ch);
    }

    if !current.trim().is_empty() {
        parts.push(current.trim().to_string());
    }

    parts
}

fn parse_type(raw: &str) -> Result<DslType, DslParseError> {
    let ty = raw.trim();
    if ty.is_empty() {
        return Err(DslParseError::InvalidSignature {
            signature: raw.to_string(),
            reason: "type is missing".to_string(),
        });
    }

    if let Some(inner) = ty.strip_prefix("optional<") {
        let inner = inner.trim_end_matches('>');
        return Ok(DslType::Optional(Box::new(parse_type(inner)?)));
    }

    if let Some(inner) = ty.strip_prefix("list<") {
        let inner = inner.trim_end_matches('>');
        return Ok(DslType::List(Box::new(parse_type(inner)?)));
    }

    if let Some(inner) = ty.strip_prefix("map<") {
        let inner = inner.trim_end_matches('>');
        let mut parts = split_params(inner);
        if parts.len() != 2 {
            return Err(DslParseError::InvalidSignature {
                signature: raw.to_string(),
                reason: "map expects two type arguments".to_string(),
            });
        }
        let value = parts.pop().unwrap();
        let key = parts.pop().unwrap();
        return Ok(DslType::Map {
            key: Box::new(parse_type(&key)?),
            value: Box::new(parse_type(&value)?),
        });
    }

    let lower = ty.to_ascii_lowercase();
    match lower.as_str() {
        "bool" | "boolean" => Ok(DslType::Bool),
        "bytes" => Ok(DslType::Bytes),
        "string" => Ok(DslType::String),
        "address" => Ok(DslType::Address),
        "any" => Ok(DslType::Any),
        _ => {
            if let Some(bits) = lower.strip_prefix("uint") {
                let bits: u16 = bits.parse().map_err(|_| DslParseError::InvalidSignature {
                    signature: raw.to_string(),
                    reason: "invalid uint width".to_string(),
                })?;
                return Ok(DslType::Uint { bits });
            }
            if let Some(bits) = lower.strip_prefix("int") {
                let bits: u16 = bits.parse().map_err(|_| DslParseError::InvalidSignature {
                    signature: raw.to_string(),
                    reason: "invalid int width".to_string(),
                })?;
                return Ok(DslType::Int { bits });
            }

            Ok(DslType::Custom(ty.to_string()))
        }
    }
}

fn parse_block(lines: Vec<(usize, String)>) -> Result<Vec<DslStmt>, DslParseError> {
    let mut parser = BlockParser::new(lines);
    parser.parse_block(0)
}

struct BlockParser {
    lines: Vec<(usize, String)>,
    index: usize,
}

impl BlockParser {
    fn new(lines: Vec<(usize, String)>) -> Self {
        Self { lines, index: 0 }
    }

    fn parse_block(&mut self, indent: usize) -> Result<Vec<DslStmt>, DslParseError> {
        let mut statements = Vec::new();
        let mut block_indent: Option<usize> = None;

        while let Some((_, line)) = self.peek_line() {
            let trimmed = line.trim_end();
            if trimmed.is_empty() {
                self.index += 1;
                continue;
            }
            let current_indent = line.chars().take_while(|c| *c == ' ').count();
            if current_indent <= indent && block_indent.is_some() {
                break;
            }
            if current_indent <= indent && block_indent.is_none() {
                break;
            }

            if block_indent.is_none() {
                block_indent = Some(current_indent);
            }

            if current_indent != block_indent.unwrap_or(current_indent) {
                return Err(DslParseError::Statement(format!(
                    "unexpected indent: {trimmed}"
                )));
            }

            let (line_no, line_text) = self.next_line().unwrap();
            let line_text = line_text.trim().to_string();
            if line_text.is_empty() {
                continue;
            }

            let stmt = self.parse_statement(line_no, &line_text, block_indent.unwrap_or(indent))?;
            statements.push(stmt);
        }

        Ok(statements)
    }

    fn parse_statement(
        &mut self,
        line_no: usize,
        line: &str,
        indent: usize,
    ) -> Result<DslStmt, DslParseError> {
        let mut line = line.to_string();
        if let Some(rest) = line.strip_prefix("return") {
            let expr = rest.trim();
            if expr.is_empty() {
                return Ok(DslStmt::Return(None));
            }
            let expr_line = self.collect_continuation(expr.to_string())?;
            let parsed = parse_expr(&expr_line).map_err(|err| {
                DslParseError::Expression(format!(
                    "line {}: {} in `{}`",
                    line_no + 1,
                    err,
                    expr_line
                ))
            })?;
            return Ok(DslStmt::Return(Some(parsed)));
        }

        if let Some(rest) = line.strip_prefix("raise") {
            let expr = rest.trim();
            if expr.is_empty() {
                return Err(DslParseError::Statement(format!(
                    "line {}: raise missing expression",
                    line_no + 1
                )));
            }
            let expr_line = self.collect_continuation(expr.to_string())?;
            let parsed = parse_expr(&expr_line).map_err(|err| {
                DslParseError::Expression(format!(
                    "line {}: {} in `{}`",
                    line_no + 1,
                    err,
                    expr_line
                ))
            })?;
            return Ok(DslStmt::Raise(parsed));
        }

        if line == "break" {
            return Ok(DslStmt::Break);
        }

        if line == "continue" {
            return Ok(DslStmt::Continue);
        }

        if line == "pass" {
            return Ok(DslStmt::Pass);
        }

        if let Some(condition) = line.strip_prefix("if ") {
            let condition = condition.trim_end_matches(':').trim();
            let body = self.parse_inline_body(indent)?;
            let mut branches = vec![DslIfBranch {
                condition: parse_expr(condition)?,
                body,
            }];
            let mut else_branch = None;

            while let Some((_, next_line)) =
                self.peek_line().map(|(idx, line)| (idx, line.to_string()))
            {
                let next_trimmed = next_line.trim();
                let next_indent = next_line.chars().take_while(|c| *c == ' ').count();
                if next_indent != indent {
                    break;
                }
                if let Some(elif_cond) = next_trimmed.strip_prefix("elif ") {
                    self.index += 1;
                    let cond = elif_cond.trim_end_matches(':').trim();
                    let body = self.parse_block(indent)?;
                    branches.push(DslIfBranch {
                        condition: parse_expr(cond)?,
                        body,
                    });
                    continue;
                }
                if next_trimmed == "else:" {
                    self.index += 1;
                    let body = self.parse_block(indent)?;
                    else_branch = Some(body);
                    break;
                }
                break;
            }

            return Ok(DslStmt::If {
                branches,
                else_branch,
            });
        }

        if let Some(condition) = line.strip_prefix("while ") {
            let condition = condition.trim_end_matches(':').trim();
            let body = self.parse_inline_body(indent)?;
            return Ok(DslStmt::While {
                condition: parse_expr(condition)?,
                body,
            });
        }

        if let Some(rest) = line.strip_prefix("for ") {
            let rest = rest.trim_end_matches(':').trim();
            let (binding, iterable) = rest
                .split_once(" in ")
                .ok_or_else(|| DslParseError::Statement(format!(
                    "line {}: invalid for-loop syntax",
                    line_no + 1
                )))?;
            let body = self.parse_inline_body(indent)?;
            let iterable_expr = parse_expr(iterable.trim()).map_err(|err| {
                DslParseError::Expression(format!(
                    "line {}: {} in `{}`",
                    line_no + 1,
                    err,
                    iterable.trim()
                ))
            })?;
            return Ok(DslStmt::For {
                binding: binding.trim().to_string(),
                iterable: iterable_expr,
                body,
            });
        }

        if let Some(rest) = line.strip_prefix("let ") {
            let rest = rest.trim();
            let rest = self.collect_continuation(rest.to_string())?;
            let (left, right) = rest.split_once('=').ok_or_else(|| {
                DslParseError::Statement(format!(
                    "line {}: let statement missing '='",
                    line_no + 1
                ))
            })?;
            let (name, ty) = if let Some((raw_name, raw_ty)) = left.split_once(':') {
                let name = raw_name.trim();
                let ty = raw_ty.trim();
                if name.is_empty() || ty.is_empty() {
                    return Err(DslParseError::Statement(format!(
                        "line {}: invalid let binding",
                        line_no + 1
                    )));
                }
                (name, Some(parse_type(ty)?))
            } else {
                let name = left.trim();
                if name.is_empty() {
                    return Err(DslParseError::Statement(format!(
                        "line {}: invalid let binding",
                        line_no + 1
                    )));
                }
                (name, None)
            };

            let value = parse_expr(right.trim()).map_err(|err| {
                DslParseError::Expression(format!(
                    "line {}: {} in `{}`",
                    line_no + 1,
                    err,
                    right.trim()
                ))
            })?;

            return Ok(DslStmt::Let {
                name: name.to_string(),
                ty,
                value,
            });
        }

        line = self.collect_continuation(line)?;

        if let Some((left, op, right)) = split_augmented_assignment(&line) {
            let target = parse_expr(left).map_err(|err| {
                DslParseError::Expression(format!(
                    "line {}: {} in `{}`",
                    line_no + 1,
                    err,
                    left
                ))
            })?;
            let rhs = parse_expr(right).map_err(|err| {
                DslParseError::Expression(format!(
                    "line {}: {} in `{}`",
                    line_no + 1,
                    err,
                    right
                ))
            })?;
            let op = match op {
                "+=" => DslBinaryOp::Add,
                "-=" => DslBinaryOp::Sub,
                _ => {
                    return Err(DslParseError::Statement(format!(
                        "line {}: unsupported assignment operator {op}",
                        line_no + 1
                    )))
                }
            };
            let value = DslExpr::Binary {
                left: Box::new(target.clone()),
                op,
                right: Box::new(rhs),
            };
            return Ok(DslStmt::Assign { target, value });
        }

        if let Some((left, right)) = split_assignment(&line) {
            let target = parse_expr(left).map_err(|err| {
                DslParseError::Expression(format!(
                    "line {}: {} in `{}`",
                    line_no + 1,
                    err,
                    left
                ))
            })?;
            let value = parse_expr(right).map_err(|err| {
                DslParseError::Expression(format!(
                    "line {}: {} in `{}`",
                    line_no + 1,
                    err,
                    right
                ))
            })?;
            return Ok(DslStmt::Assign { target, value });
        }

        let parsed = parse_expr(&line).map_err(|err| {
            DslParseError::Expression(format!(
                "line {}: {} in `{}`",
                line_no + 1,
                err,
                line
            ))
        })?;
        Ok(DslStmt::Expr(parsed))
    }

    fn peek_line(&self) -> Option<(usize, &str)> {
        self.lines
            .get(self.index)
            .map(|(line_no, line)| (*line_no, line.as_str()))
    }

    fn next_line(&mut self) -> Option<(usize, String)> {
        if self.index >= self.lines.len() {
            return None;
        }
        let (line_no, line) = self.lines[self.index].clone();
        self.index += 1;
        Some((line_no, line))
    }

    fn collect_continuation(&mut self, mut line: String) -> Result<String, DslParseError> {
        while needs_continuation(&line) {
            let Some(next_line) = self.peek_line().map(|(_, line)| line.to_string()) else {
                return Err(DslParseError::Expression(
                    "unexpected end of expression".to_string(),
                ));
            };
            let trimmed = next_line.trim();
            self.index += 1;
            if trimmed.is_empty() {
                continue;
            }
            line.push(' ');
            line.push_str(trimmed);
        }

        Ok(line)
    }

    fn parse_inline_body(&mut self, indent: usize) -> Result<Vec<DslStmt>, DslParseError> {
        let mut cursor = self.index;
        while let Some((_, line)) = self.lines.get(cursor) {
            if line.trim_end().is_empty() {
                cursor += 1;
                continue;
            }
            let next_indent = line.chars().take_while(|c| *c == ' ').count();
            if next_indent > indent {
                return self.parse_block(indent);
            }
            if next_indent < indent {
                return Err(DslParseError::Statement("unexpected block end".to_string()));
            }
            let (line_no, line_text) = self
                .next_line()
                .ok_or_else(|| DslParseError::Statement("missing block body".to_string()))?;
            let stmt = self.parse_statement(line_no, line_text.trim(), indent)?;
            return Ok(vec![stmt]);
        }

        Err(DslParseError::Statement("missing block body".to_string()))
    }
}

fn needs_continuation(line: &str) -> bool {
    let mut depth = 0usize;
    let mut in_string = false;
    let mut prev = '\0';

    for ch in line.chars() {
        match ch {
            '"' if prev != '\\' => in_string = !in_string,
            '(' | '[' | '{' if !in_string => depth += 1,
            ')' | ']' | '}' if !in_string && depth > 0 => depth -= 1,
            _ => {}
        }
        prev = ch;
    }

    depth > 0
}

fn split_assignment(line: &str) -> Option<(&str, &str)> {
    let mut depth = 0usize;
    let mut in_string = false;
    let mut prev = '\0';

    for (idx, ch) in line.char_indices() {
        match ch {
            '"' if prev != '\\' => in_string = !in_string,
            '(' | '[' | '{' if !in_string => depth += 1,
            ')' | ']' | '}' if !in_string && depth > 0 => depth -= 1,
            '=' if depth == 0 && !in_string => {
                let before = line[..idx].trim();
                let after = line[idx + 1..].trim();
                if before.is_empty() || after.is_empty() {
                    return None;
                }
                return Some((before, after));
            }
            _ => {}
        }
        prev = ch;
    }

    None
}

fn split_augmented_assignment(line: &str) -> Option<(&str, &str, &str)> {
    let mut depth = 0usize;
    let mut in_string = false;
    let mut prev = '\0';

    let bytes = line.as_bytes();
    let mut idx = 0usize;
    while idx + 1 < bytes.len() {
        let ch = bytes[idx] as char;
        match ch {
            '"' if prev != '\\' => in_string = !in_string,
            '(' | '[' | '{' if !in_string => depth += 1,
            ')' | ']' | '}' if !in_string && depth > 0 => depth -= 1,
            _ => {}
        }

        if depth == 0 && !in_string {
            let two = &line[idx..idx + 2];
            if two == "+=" || two == "-=" {
                let before = line[..idx].trim();
                let after = line[idx + 2..].trim();
                if before.is_empty() || after.is_empty() {
                    return None;
                }
                return Some((before, two, after));
            }
        }

        prev = ch;
        idx += 1;
    }

    None
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    Ident(String),
    Number(String),
    String(String),
    Bytes(Vec<u8>),
    True,
    False,
    None,
    Symbol(char),
    Operator(String),
    Eof,
}

#[derive(Clone)]
struct Tokenizer<'a> {
    input: &'a str,
    chars: std::str::CharIndices<'a>,
    peeked: Option<(usize, char)>,
}

impl<'a> Tokenizer<'a> {
    fn new(input: &'a str) -> Self {
        Self {
            input,
            chars: input.char_indices(),
            peeked: None,
        }
    }

    fn next_token(&mut self) -> Result<Token, DslParseError> {
        self.skip_whitespace();
        let Some((idx, ch)) = self.next_char() else {
            return Ok(Token::Eof);
        };

        if ch.is_ascii_alphabetic() || ch == '_' {
            let mut end = idx + ch.len_utf8();
            while let Some((next_idx, next_ch)) = self.peek_char() {
                if next_ch.is_ascii_alphanumeric() || next_ch == '_' {
                    self.next_char();
                    end = next_idx + next_ch.len_utf8();
                } else {
                    break;
                }
            }
            let word = &self.input[idx..end];
            return Ok(match word {
                "true" | "True" => Token::True,
                "false" | "False" => Token::False,
                "None" => Token::None,
                "and" | "or" | "not" => Token::Operator(word.to_string()),
                _ => Token::Ident(word.to_string()),
            });
        }

        if ch.is_ascii_digit() {
            let mut end = idx + ch.len_utf8();
            while let Some((next_idx, next_ch)) = self.peek_char() {
                if next_ch.is_ascii_hexdigit() || next_ch == '_' || next_ch == 'x' {
                    self.next_char();
                    end = next_idx + next_ch.len_utf8();
                } else {
                    break;
                }
            }
            let number = self.input[idx..end].replace('_', "");
            return Ok(Token::Number(number));
        }

        if ch == '"' {
            let mut out = String::new();
            while let Some((_, next_ch)) = self.next_char() {
                if next_ch == '"' {
                    break;
                }
                if next_ch == '\\' {
                    if let Some((_, esc)) = self.next_char() {
                        out.push(match esc {
                            'n' => '\n',
                            'r' => '\r',
                            't' => '\t',
                            '0' => '\0',
                            '"' => '"',
                            '\\' => '\\',
                            other => other,
                        });
                        continue;
                    }
                }
                out.push(next_ch);
            }
            return Ok(Token::String(out));
        }

        if ch == 'b' {
            if let Some((_, '"')) = self.peek_char() {
                self.next_char();
                let mut bytes = Vec::new();
                while let Some((_, next_ch)) = self.next_char() {
                    if next_ch == '"' {
                        break;
                    }
                    if next_ch == '\\' {
                        if let Some((_, esc)) = self.next_char() {
                            bytes.push(match esc {
                                'n' => b'\n',
                                'r' => b'\r',
                                't' => b'\t',
                                '0' => b'\0',
                                '"' => b'"',
                                '\\' => b'\\',
                                other => other as u8,
                            });
                            continue;
                        }
                    }
                    bytes.push(next_ch as u8);
                }
                return Ok(Token::Bytes(bytes));
            }
        }

        let op = match ch {
            '=' | '!' | '<' | '>' => {
                if let Some((_, '=')) = self.peek_char() {
                    self.next_char();
                    Some(format!("{ch}="))
                } else {
                    Some(ch.to_string())
                }
            }
            _ => None,
        };

        if let Some(op) = op {
            return Ok(Token::Operator(op));
        }

        if "+-*/%(),.:[]{}".contains(ch) {
            return Ok(Token::Symbol(ch));
        }

        Err(DslParseError::Expression(format!(
            "unexpected character `{}`",
            ch
        )))
    }

    fn skip_whitespace(&mut self) {
        while let Some((_, ch)) = self.peek_char() {
            if ch.is_whitespace() {
                self.next_char();
            } else {
                break;
            }
        }
    }

    fn peek_char(&mut self) -> Option<(usize, char)> {
        if self.peeked.is_none() {
            self.peeked = self.chars.next();
        }
        self.peeked
    }

    fn next_char(&mut self) -> Option<(usize, char)> {
        if let Some(ch) = self.peeked.take() {
            return Some(ch);
        }
        self.chars.next()
    }
}

struct ExprParser<'a> {
    tokenizer: Tokenizer<'a>,
    lookahead: Token,
}

impl<'a> ExprParser<'a> {
    fn new(input: &'a str) -> Result<Self, DslParseError> {
        let mut tokenizer = Tokenizer::new(input);
        let lookahead = tokenizer.next_token()?;
        Ok(Self { tokenizer, lookahead })
    }

    fn parse(mut self) -> Result<DslExpr, DslParseError> {
        let expr = self.parse_or()?;
        Ok(expr)
    }

    fn parse_or(&mut self) -> Result<DslExpr, DslParseError> {
        let mut expr = self.parse_and()?;
        while matches!(self.lookahead, Token::Operator(ref op) if op == "or") {
            self.bump()?;
            let rhs = self.parse_and()?;
            expr = DslExpr::Binary {
                left: Box::new(expr),
                op: DslBinaryOp::LogicalOr,
                right: Box::new(rhs),
            };
        }
        Ok(expr)
    }

    fn parse_and(&mut self) -> Result<DslExpr, DslParseError> {
        let mut expr = self.parse_equality()?;
        while matches!(self.lookahead, Token::Operator(ref op) if op == "and") {
            self.bump()?;
            let rhs = self.parse_equality()?;
            expr = DslExpr::Binary {
                left: Box::new(expr),
                op: DslBinaryOp::LogicalAnd,
                right: Box::new(rhs),
            };
        }
        Ok(expr)
    }

    fn parse_equality(&mut self) -> Result<DslExpr, DslParseError> {
        let mut expr = self.parse_comparison()?;
        loop {
            match self.lookahead {
                Token::Operator(ref op) if op == "==" => {
                    self.bump()?;
                    let rhs = self.parse_comparison()?;
                    expr = DslExpr::Binary {
                        left: Box::new(expr),
                        op: DslBinaryOp::Equal,
                        right: Box::new(rhs),
                    };
                }
                Token::Operator(ref op) if op == "!=" => {
                    self.bump()?;
                    let rhs = self.parse_comparison()?;
                    expr = DslExpr::Binary {
                        left: Box::new(expr),
                        op: DslBinaryOp::NotEqual,
                        right: Box::new(rhs),
                    };
                }
                _ => break,
            }
        }
        Ok(expr)
    }

    fn parse_comparison(&mut self) -> Result<DslExpr, DslParseError> {
        let mut expr = self.parse_additive()?;
        loop {
            match self.lookahead {
                Token::Operator(ref op) if op == "<" => {
                    self.bump()?;
                    let rhs = self.parse_additive()?;
                    expr = DslExpr::Binary {
                        left: Box::new(expr),
                        op: DslBinaryOp::Less,
                        right: Box::new(rhs),
                    };
                }
                Token::Operator(ref op) if op == "<=" => {
                    self.bump()?;
                    let rhs = self.parse_additive()?;
                    expr = DslExpr::Binary {
                        left: Box::new(expr),
                        op: DslBinaryOp::LessEqual,
                        right: Box::new(rhs),
                    };
                }
                Token::Operator(ref op) if op == ">" => {
                    self.bump()?;
                    let rhs = self.parse_additive()?;
                    expr = DslExpr::Binary {
                        left: Box::new(expr),
                        op: DslBinaryOp::Greater,
                        right: Box::new(rhs),
                    };
                }
                Token::Operator(ref op) if op == ">=" => {
                    self.bump()?;
                    let rhs = self.parse_additive()?;
                    expr = DslExpr::Binary {
                        left: Box::new(expr),
                        op: DslBinaryOp::GreaterEqual,
                        right: Box::new(rhs),
                    };
                }
                _ => break,
            }
        }
        Ok(expr)
    }

    fn parse_additive(&mut self) -> Result<DslExpr, DslParseError> {
        let mut expr = self.parse_multiplicative()?;
        loop {
            match self.lookahead {
                Token::Symbol('+') => {
                    self.bump()?;
                    let rhs = self.parse_multiplicative()?;
                    expr = DslExpr::Binary {
                        left: Box::new(expr),
                        op: DslBinaryOp::Add,
                        right: Box::new(rhs),
                    };
                }
                Token::Symbol('-') => {
                    self.bump()?;
                    let rhs = self.parse_multiplicative()?;
                    expr = DslExpr::Binary {
                        left: Box::new(expr),
                        op: DslBinaryOp::Sub,
                        right: Box::new(rhs),
                    };
                }
                _ => break,
            }
        }
        Ok(expr)
    }

    fn parse_multiplicative(&mut self) -> Result<DslExpr, DslParseError> {
        let mut expr = self.parse_unary()?;
        loop {
            match self.lookahead {
                Token::Symbol('*') => {
                    self.bump()?;
                    let rhs = self.parse_unary()?;
                    expr = DslExpr::Binary {
                        left: Box::new(expr),
                        op: DslBinaryOp::Mul,
                        right: Box::new(rhs),
                    };
                }
                Token::Symbol('/') => {
                    self.bump()?;
                    let rhs = self.parse_unary()?;
                    expr = DslExpr::Binary {
                        left: Box::new(expr),
                        op: DslBinaryOp::Div,
                        right: Box::new(rhs),
                    };
                }
                Token::Symbol('%') => {
                    self.bump()?;
                    let rhs = self.parse_unary()?;
                    expr = DslExpr::Binary {
                        left: Box::new(expr),
                        op: DslBinaryOp::Mod,
                        right: Box::new(rhs),
                    };
                }
                _ => break,
            }
        }
        Ok(expr)
    }

    fn parse_unary(&mut self) -> Result<DslExpr, DslParseError> {
        match self.lookahead {
            Token::Symbol('-') => {
                self.bump()?;
                let expr = self.parse_unary()?;
                Ok(DslExpr::Unary {
                    op: DslUnaryOp::Neg,
                    expr: Box::new(expr),
                })
            }
            Token::Operator(ref op) if op == "not" => {
                self.bump()?;
                let expr = self.parse_unary()?;
                Ok(DslExpr::Unary {
                    op: DslUnaryOp::Not,
                    expr: Box::new(expr),
                })
            }
            _ => self.parse_postfix(),
        }
    }

    fn parse_postfix(&mut self) -> Result<DslExpr, DslParseError> {
        let mut expr = self.parse_primary()?;
        loop {
            match self.lookahead {
                Token::Symbol('(') => {
                    self.bump()?;
                    let args = self.parse_call_args()?;
                    expr = DslExpr::Call {
                        callee: Box::new(expr),
                        args,
                    };
                }
                Token::Symbol('[') => {
                    self.bump()?;
                    let index = self.parse_or()?;
                    self.expect_symbol(']')?;
                    expr = DslExpr::Index {
                        target: Box::new(expr),
                        index: Box::new(index),
                    };
                }
                Token::Symbol('.') => {
                    self.bump()?;
                    match &self.lookahead {
                        Token::Ident(name) => {
                            let name = name.clone();
                            self.bump()?;
                            expr = DslExpr::Attribute {
                                target: Box::new(expr),
                                attribute: name,
                            };
                        }
                        _ => {
                            return Err(DslParseError::Expression(
                                "expected identifier after '.'".to_string(),
                            ))
                        }
                    }
                }
                _ => break,
            }
        }

        Ok(expr)
    }

    fn parse_primary(&mut self) -> Result<DslExpr, DslParseError> {
        match &self.lookahead {
            Token::Ident(name) => {
                let name = name.clone();
                self.bump()?;
                Ok(DslExpr::Identifier(name))
            }
            Token::True => {
                self.bump()?;
                Ok(DslExpr::Literal(DslLiteral::Bool(true)))
            }
            Token::False => {
                self.bump()?;
                Ok(DslExpr::Literal(DslLiteral::Bool(false)))
            }
            Token::None => {
                self.bump()?;
                Ok(DslExpr::Literal(DslLiteral::None))
            }
            Token::Number(number) => {
                let number = number.clone();
                self.bump()?;
                Ok(DslExpr::Literal(DslLiteral::Number(number)))
            }
            Token::String(value) => {
                let value = value.clone();
                self.bump()?;
                Ok(DslExpr::Literal(DslLiteral::String(value)))
            }
            Token::Bytes(bytes) => {
                let bytes = bytes.clone();
                self.bump()?;
                Ok(DslExpr::Literal(DslLiteral::Bytes(bytes)))
            }
            Token::Symbol('(') => {
                self.bump()?;
                let expr = self.parse_or()?;
                self.expect_symbol(')')?;
                Ok(expr)
            }
            Token::Symbol('[') => {
                self.bump()?;
                let mut items = Vec::new();
                if !matches!(self.lookahead, Token::Symbol(']')) {
                    loop {
                        let item = self.parse_or()?;
                        items.push(item);
                        if matches!(self.lookahead, Token::Symbol(',')) {
                            self.bump()?;
                            continue;
                        }
                        break;
                    }
                }
                self.expect_symbol(']')?;
                Ok(DslExpr::ListLiteral(items))
            }
            Token::Symbol('{') => {
                self.bump()?;
                let mut entries = Vec::new();
                if !matches!(self.lookahead, Token::Symbol('}')) {
                    loop {
                        let key = self.parse_or()?;
                        self.expect_symbol(':')?;
                        let value = self.parse_or()?;
                        entries.push((key, value));
                        if matches!(self.lookahead, Token::Symbol(',')) {
                            self.bump()?;
                            continue;
                        }
                        break;
                    }
                }
                self.expect_symbol('}')?;
                Ok(DslExpr::MapLiteral(entries))
            }
            _ => Err(DslParseError::Expression(format!(
                "unexpected token: {:?}",
                self.lookahead
            ))),
        }
    }

    fn parse_call_args(&mut self) -> Result<Vec<DslCallArg>, DslParseError> {
        let mut args = Vec::new();
        if !matches!(self.lookahead, Token::Symbol(')')) {
            loop {
                if self.peek_is_named_arg() {
                    args.push(self.parse_named_arg()?);
                } else {
                    args.push(DslCallArg::Positional(self.parse_or()?));
                }
                if matches!(self.lookahead, Token::Symbol(',')) {
                    self.bump()?;
                    continue;
                }
                break;
            }
        }
        self.expect_symbol(')')?;
        Ok(args)
    }

    fn peek_is_named_arg(&self) -> bool {
        if !matches!(self.lookahead, Token::Ident(_)) {
            return false;
        }
        let mut tokenizer = self.tokenizer.clone();
        matches!(tokenizer.next_token(), Ok(Token::Operator(ref op)) if op == "=")
    }

    fn parse_named_arg(&mut self) -> Result<DslCallArg, DslParseError> {
        let name = match &self.lookahead {
            Token::Ident(name) => name.clone(),
            _ => {
                return Err(DslParseError::Expression(
                    "expected identifier for named arg".to_string(),
                ))
            }
        };
        self.bump()?;
        match &self.lookahead {
            Token::Operator(op) if op == "=" => {
                self.bump()?;
            }
            _ => {
                return Err(DslParseError::Expression(
                    "expected '=' in named arg".to_string(),
                ))
            }
        }
        let value = self.parse_or()?;
        Ok(DslCallArg::Named { name, value })
    }

    fn expect_symbol(&mut self, symbol: char) -> Result<(), DslParseError> {
        if matches!(self.lookahead, Token::Symbol(c) if c == symbol) {
            self.bump()?;
            Ok(())
        } else {
            Err(DslParseError::Expression(format!(
                "expected '{}'",
                symbol
            )))
        }
    }

    fn bump(&mut self) -> Result<(), DslParseError> {
        self.lookahead = self.tokenizer.next_token()?;
        Ok(())
    }
}

fn parse_expr(input: &str) -> Result<DslExpr, DslParseError> {
    ExprParser::new(input)?.parse()
}

impl fmt::Display for DslType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DslType::Bool => write!(f, "bool"),
            DslType::Bytes => write!(f, "bytes"),
            DslType::String => write!(f, "string"),
            DslType::Address => write!(f, "address"),
            DslType::Uint { bits } => write!(f, "uint{}", bits),
            DslType::Int { bits } => write!(f, "int{}", bits),
            DslType::Optional(inner) => write!(f, "optional<{}>", inner),
            DslType::List(inner) => write!(f, "list<{}>", inner),
            DslType::Map { key, value } => write!(f, "map<{}, {}>", key, value),
            DslType::Any => write!(f, "any"),
            DslType::Custom(name) => write!(f, "{}", name),
        }
    }
}
