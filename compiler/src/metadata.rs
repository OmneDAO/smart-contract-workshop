use std::path::Path;

use axiom_runtime::abi;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::ir::{self, HostFunction, ValueType};
use crate::CompilerError;

const METADATA_VERSION: &str = "1.0";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompilationMetadata {
    pub metadata_version: String,
    pub compiler_version: String,
    pub generated_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_path: Option<String>,
    pub wasm_sha256: String,
    pub wasm_size_bytes: usize,
    pub contracts: Vec<ContractMetadata>,
    pub free_functions: Vec<FreeFunctionMetadata>,
    pub host_functions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContractMetadata {
    pub name: String,
    pub params: Vec<FunctionParamMetadata>,
    pub storage: Vec<StorageFieldMetadata>,
    pub methods: Vec<ContractMethodMetadata>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContractMethodMetadata {
    pub name: String,
    pub selector: String,
    pub export: String,
    pub params: Vec<FunctionParamMetadata>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub return_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FreeFunctionMetadata {
    pub name: String,
    pub export: String,
    pub params: Vec<FunctionParamMetadata>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub return_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionParamMetadata {
    pub name: String,
    pub ty: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageFieldMetadata {
    pub name: String,
    pub ty: String,
}

impl CompilationMetadata {
    pub fn from_ir(module: &ir::Module, source_path: Option<&Path>, wasm: &[u8]) -> Self {
        let compiler_version = env!("CARGO_PKG_VERSION").to_string();
        let generated_at = Utc::now().to_rfc3339();
        let wasm_sha256 = format!("{:x}", Sha256::digest(wasm));
        let wasm_size_bytes = wasm.len();
        let host_functions = collect_host_functions(module);

        let contracts = module
            .contracts
            .iter()
            .map(|contract| ContractMetadata {
                name: contract.name.clone(),
                params: contract
                    .params
                    .iter()
                    .map(FunctionParamMetadata::from_param)
                    .collect(),
                storage: contract
                    .storage
                    .iter()
                    .map(StorageFieldMetadata::from_field)
                    .collect(),
                methods: contract
                    .functions
                    .iter()
                    .map(|function| {
                        ContractMethodMetadata {
                            name: function.name.clone(),
                            selector: format!("{}::{}", contract.name, function.name),
                            export: abi::contract_export(&contract.name, &function.name),
                            params: function
                                .params
                                .iter()
                                .map(FunctionParamMetadata::from_param)
                                .collect(),
                            return_type: function.return_type.map(value_type_to_string),
                        }
                    })
                    .collect(),
            })
            .collect();

        let free_functions = module
            .functions
            .iter()
            .map(|function| FreeFunctionMetadata {
                name: function.name.clone(),
                export: function.name.clone(),
                params: function
                    .params
                    .iter()
                    .map(FunctionParamMetadata::from_param)
                    .collect(),
                return_type: function.return_type.map(value_type_to_string),
            })
            .collect();

        Self {
            metadata_version: METADATA_VERSION.to_string(),
            compiler_version,
            generated_at,
            source_path: source_path.map(|path| path.to_string_lossy().to_string()),
            wasm_sha256,
            wasm_size_bytes,
            contracts,
            free_functions,
            host_functions,
        }
    }
}

impl FunctionParamMetadata {
    fn from_param(param: &ir::Param) -> Self {
        Self {
            name: param.name.clone(),
            ty: value_type_to_string(param.ty),
        }
    }
}

impl StorageFieldMetadata {
    fn from_field(field: &ir::Field) -> Self {
        Self {
            name: field.name.clone(),
            ty: value_type_to_string(field.ty),
        }
    }
}

fn collect_host_functions(module: &ir::Module) -> Vec<String> {
    module
        .used_host_functions()
        .into_iter()
        .map(|function| format_host_function(&function))
        .collect()
}

fn format_host_function(function: &HostFunction) -> String {
    format!("{}::{}", function.module(), function.field())
}

fn value_type_to_string(value_type: ValueType) -> String {
    value_type.to_string()
}

pub fn canonical_metadata_digest(metadata: &CompilationMetadata) -> Result<[u8; 32], CompilerError> {
    let serialized = bincode::serialize(metadata)
        .map_err(|err| CompilerError::Serialization(err.to_string()))?;
    let digest = Sha256::digest(serialized);
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    Ok(out)
}
