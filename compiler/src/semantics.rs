//! Semantic analysis for pysub programs.

use std::collections::HashSet;

use thiserror::Error;

use crate::ast::{Contract, Function, PrimitiveType, Program, Type};

#[derive(Debug, Error, PartialEq, Eq)]
pub enum SemanticError {
    #[error("duplicate contract definition: {0}")]
    DuplicateContract(String),

    #[error("duplicate storage field `{field}` in contract `{contract}`")]
    DuplicateStorage { contract: String, field: String },

    #[error("duplicate function `{function}` in contract `{contract}`")]
    DuplicateFunction { contract: String, function: String },

    #[error("duplicate parameter `{param}` in function `{function}`")]
    DuplicateParameter { function: String, param: String },

    #[error("unknown type `{ty}` in {context}")]
    UnknownType { ty: String, context: String },

    #[error("invalid map key type `{ty}` in {context}")]
    InvalidMapKeyType { ty: String, context: String },

    #[error("missing required entry function `{0}`")]
    MissingEntry(String),

    #[error("invalid signature for entry `{function}`: expected `{expected}`, found `{found}`")]
    InvalidEntrySignature {
        function: String,
        expected: String,
        found: String,
    },
}

pub struct SemanticAnalyzer;

impl SemanticAnalyzer {
    pub fn validate(program: &Program) -> Result<(), SemanticError> {
        let mut contracts = HashSet::new();
        for contract in &program.contracts {
            let name = contract.name.as_str().to_owned();
            if !contracts.insert(name.clone()) {
                return Err(SemanticError::DuplicateContract(name));
            }
            Self::validate_contract(contract)?;
        }

        let mut functions = HashSet::new();
        let mut has_main = false;
        for function in &program.functions {
            let name = function.name.as_str().to_owned();
            if !functions.insert(name.clone()) {
                return Err(SemanticError::DuplicateFunction {
                    contract: "<module>".to_string(),
                    function: name,
                });
            }
            Self::validate_function(function)?;

            if function.name.as_str() == "main" {
                has_main = true;
                if !function.params.is_empty() {
                    return Err(SemanticError::InvalidEntrySignature {
                        function: "main".to_string(),
                        expected: "fn main() -> u128".to_string(),
                        found: format_function_signature(function),
                    });
                }

                match &function.return_type {
                    Some(Type::Primitive(PrimitiveType::U128)) => {}
                    Some(other) => {
                        return Err(SemanticError::InvalidEntrySignature {
                            function: "main".to_string(),
                            expected: "fn main() -> u128".to_string(),
                            found: format!("fn main() -> {}", other),
                        })
                    }
                    None => {
                        return Err(SemanticError::InvalidEntrySignature {
                            function: "main".to_string(),
                            expected: "fn main() -> u128".to_string(),
                            found: format_function_signature(function),
                        })
                    }
                }
            }
        }

        if !has_main {
            return Err(SemanticError::MissingEntry("main".to_string()));
        }

        Ok(())
    }

    fn validate_contract(contract: &Contract) -> Result<(), SemanticError> {
        let mut fields = HashSet::new();
        for field in &contract.storage {
            let field_name = field.name.as_str().to_owned();
            if !fields.insert(field_name.clone()) {
                return Err(SemanticError::DuplicateStorage {
                    contract: contract.name.as_str().to_owned(),
                    field: field_name,
                });
            }
            Self::validate_type(
                &field.ty,
                &format!("contract `{}` storage `{}`", contract.name, field.name),
            )?;
        }

        let mut functions = HashSet::new();
        for function in &contract.functions {
            let fname = function.name.as_str().to_owned();
            if !functions.insert(fname.clone()) {
                return Err(SemanticError::DuplicateFunction {
                    contract: contract.name.as_str().to_owned(),
                    function: fname,
                });
            }
            Self::validate_function(function)?;
        }

        Ok(())
    }

    fn validate_function(function: &Function) -> Result<(), SemanticError> {
        let mut params = HashSet::new();
        for param in &function.params {
            Self::validate_type(
                &param.ty,
                &format!("function `{}` parameter `{}`", function.name, param.name),
            )?;
            if !params.insert(param.name.as_str().to_owned()) {
                return Err(SemanticError::DuplicateParameter {
                    function: function.name.as_str().to_owned(),
                    param: param.name.as_str().to_owned(),
                });
            }
        }

        if let Some(ret) = &function.return_type {
            Self::validate_type(ret, &format!("function `{}` return type", function.name))?;
        }

        Ok(())
    }

    fn validate_type(ty: &Type, context: &str) -> Result<(), SemanticError> {
        match ty {
            Type::Primitive(_) => Ok(()),
            Type::Map { key, value } => {
                if !matches!(
                    key.as_ref(),
                    Type::Primitive(PrimitiveType::Address) | Type::Primitive(PrimitiveType::Bytes)
                ) {
                    return Err(SemanticError::InvalidMapKeyType {
                        ty: key.to_string(),
                        context: context.to_owned(),
                    });
                }
                Self::validate_type(value, context)
            }
        }
    }
}

fn format_function_signature(function: &Function) -> String {
    let params = function
        .params
        .iter()
        .map(|param| param.ty.to_string())
        .collect::<Vec<_>>()
        .join(", ");

    if let Some(ret) = &function.return_type {
        if params.is_empty() {
            format!("fn {}() -> {}", function.name, ret)
        } else {
            format!("fn {}({}) -> {}", function.name, params, ret)
        }
    } else if params.is_empty() {
        format!("fn {}()", function.name)
    } else {
        format!("fn {}({})", function.name, params)
    }
}

pub fn validate_program(program: &Program) -> Result<(), SemanticError> {
    SemanticAnalyzer::validate(program)
}
