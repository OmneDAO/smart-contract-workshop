use std::{env, fs, path::PathBuf, process};

use chrono::Utc;
use ed25519_dalek::{Signer, SigningKey};
use pysub_compiler::{
    compile_file_with_artifacts, compile_manifest_with_artifacts,
    metadata::{canonical_metadata_digest, CompilationMetadata},
};
use rand::rngs::OsRng;
use serde::Serialize;

fn main() {
    if let Err(err) = run() {
        eprintln!("{err}");
        process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let options = parse_args()?;
    let artifacts = if let Some(manifest_path) = options.manifest.as_deref() {
        compile_manifest_with_artifacts(manifest_path).map_err(|err| err.to_string())?
    } else {
        let contents = fs::read_to_string(&options.source)
            .map_err(|err| format!("failed to read source {}: {err}", options.source))?;
        if pysub_compiler::manifest::looks_like_manifest(&contents) {
            compile_manifest_with_artifacts(&options.source).map_err(|err| err.to_string())?
        } else {
            compile_file_with_artifacts(&options.source).map_err(|err| err.to_string())?
        }
    };
    let pysub_compiler::CompilationArtifacts { wasm, metadata } = artifacts;

    if let Some(wasm_path) = options.emit_wasm.as_deref() {
        write_wasm(wasm_path, &wasm)?;
    }

    if let Some(metadata_path) = options.emit_metadata.as_deref() {
        write_metadata(metadata_path, metadata, &options)?;
    } else {
        let _ = metadata;
    }

    println!("Compiled {}", options.source);
    Ok(())
}

#[derive(Debug, Default)]
struct CliOptions {
    source: String,
    manifest: Option<String>,
    emit_metadata: Option<String>,
    emit_wasm: Option<String>,
    signing_key: Option<String>,
    no_sign: bool,
}

fn parse_args() -> Result<CliOptions, String> {
    let mut args = env::args().skip(1);
    if env::args().len() == 1 {
        print_usage();
        return Err("source file is required".to_string());
    }

    let mut options = CliOptions::default();

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--manifest" => {
                let Some(path) = args.next() else {
                    return Err("--manifest requires a path".to_string());
                };
                options.manifest = Some(path);
            }
            "--emit-metadata" => {
                let Some(path) = args.next() else {
                    return Err("--emit-metadata requires a path".to_string());
                };
                options.emit_metadata = Some(path);
            }
            "--emit-wasm" => {
                let Some(path) = args.next() else {
                    return Err("--emit-wasm requires a path".to_string());
                };
                options.emit_wasm = Some(path);
            }
            "--signing-key" => {
                let Some(path) = args.next() else {
                    return Err("--signing-key requires a path".to_string());
                };
                options.signing_key = Some(path);
            }
            "--no-sign" => {
                options.no_sign = true;
            }
            "-h" | "--help" => {
                print_usage();
                process::exit(0);
            }
            _ if arg.starts_with('-') => {
                return Err(format!("unknown option: {arg}"));
            }
            _ => {
                if options.source.is_empty() {
                    options.source = arg;
                } else {
                    return Err("multiple source files provided".to_string());
                }
            }
        }
    }

    if options.source.is_empty() {
        if options.manifest.is_none() {
            return Err("source file is required".to_string());
        }
    }

    if options.signing_key.is_some() && options.no_sign {
        return Err("--no-sign cannot be combined with --signing-key".to_string());
    }

    if options.signing_key.is_some() && options.emit_metadata.is_none() {
        return Err("--signing-key is only valid when --emit-metadata is used".to_string());
    }

    if options.no_sign && options.emit_metadata.is_none() {
        return Err("--no-sign is only valid when --emit-metadata is used".to_string());
    }

    Ok(options)
}

fn write_wasm(path: &str, wasm: &[u8]) -> Result<(), String> {
    fs::write(path, wasm)
        .map_err(|err| format!("failed to write wasm to {path}: {err}"))?;
    println!("WASM written to {path}");
    Ok(())
}

fn write_metadata(
    path: &str,
    metadata: CompilationMetadata,
    options: &CliOptions,
) -> Result<(), String> {
    let signature = if options.no_sign {
        None
    } else {
        Some(sign_metadata(&metadata, options, path)?)
    };

    let envelope = MetadataEnvelope { metadata, signature };
    let json = serde_json::to_vec_pretty(&envelope)
        .map_err(|err| format!("failed to serialise metadata: {err}"))?;
    fs::write(path, json).map_err(|err| format!("failed to write metadata to {path}: {err}"))?;
    println!("Metadata written to {path}");
    Ok(())
}

fn sign_metadata(
    metadata: &CompilationMetadata,
    options: &CliOptions,
    metadata_path: &str,
) -> Result<MetadataSignature, String> {
    let signing_key = if let Some(path) = options.signing_key.as_deref() {
        load_signing_key(path)?
    } else {
        let (key, written_path) = generate_ephemeral_signing_key(metadata_path)?;
        println!("Ephemeral signing key written to {written_path}");
        key
    };

    let digest = canonical_metadata_digest(metadata)
        .map_err(|err| format!("failed to compute metadata digest: {err}"))?;

    let signature = signing_key.sign(digest.as_ref());
    let verifying_key = signing_key.verifying_key();

    Ok(MetadataSignature {
        algorithm: "ed25519".to_string(),
        public_key_hex: hex::encode(verifying_key.to_bytes()),
        signature_hex: hex::encode(signature.to_bytes()),
        digest_hex: hex::encode(digest),
        signed_at: Utc::now().to_rfc3339(),
    })
}

fn load_signing_key(path: &str) -> Result<SigningKey, String> {
    let raw = fs::read(path)
        .map_err(|err| format!("failed to read signing key from {path}: {err}"))?;

    let secret_bytes = if raw.len() == 32 {
        raw
    } else {
        let content = String::from_utf8(raw)
            .map_err(|_| "signing key must be raw 32 bytes or 64 hex characters".to_string())?;
        let cleaned = content.trim();
        if cleaned.len() == 64 && cleaned.chars().all(|c| c.is_ascii_hexdigit()) {
            hex::decode(cleaned)
                .map_err(|err| format!("failed to decode signing key hex: {err}"))?
        } else {
            return Err("signing key must be provided as 32 raw bytes or 64 hex characters".to_string());
        }
    };

    let secret_array: [u8; 32] = secret_bytes
        .try_into()
        .map_err(|_| "signing key must decode to exactly 32 bytes".to_string())?;

    Ok(SigningKey::from_bytes(&secret_array))
}

fn generate_ephemeral_signing_key(metadata_path: &str) -> Result<(SigningKey, String), String> {
    let mut rng = OsRng;
    let signing_key = SigningKey::generate(&mut rng);
    let mut key_path = PathBuf::from(metadata_path);
    key_path.set_extension("signing-key");
    fs::write(&key_path, format!("{}\n", hex::encode(signing_key.to_bytes())))
        .map_err(|err| format!("failed to write ephemeral signing key to {}: {err}", key_path.display()))?;
    Ok((signing_key, key_path.display().to_string()))
}

#[derive(Serialize)]
struct MetadataEnvelope {
    metadata: CompilationMetadata,
    #[serde(skip_serializing_if = "Option::is_none")]
    signature: Option<MetadataSignature>,
}

#[derive(Serialize)]
struct MetadataSignature {
    algorithm: String,
    public_key_hex: String,
    signature_hex: String,
    digest_hex: String,
    signed_at: String,
}

fn print_usage() {
    eprintln!("Usage: pysub-compiler [OPTIONS] <source-file>");
    eprintln!("\nOptions:");
    eprintln!("  --manifest <path>        Compile a YAML manifest instead of a pysub source file");
    eprintln!("  --emit-metadata <path>   Write compilation metadata JSON to the given path");
    eprintln!("  --emit-wasm <path>       Write emitted WASM bytes to the given path");
    eprintln!("  --signing-key <path>     Sign metadata with the Ed25519 key at the given path");
    eprintln!("  --no-sign                Skip metadata signing (unsafe)");
    eprintln!("  -h, --help               Show this help message");
}
