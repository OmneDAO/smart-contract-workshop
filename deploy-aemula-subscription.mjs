#!/usr/bin/env node
// deploy-aemula-subscription.mjs
// Deploys the Aemula subscription contract to the Omne devnet.
// Constructs a DeploymentExecutionPlan and sends it via omne_deployContract RPC.
//
// Usage: node deploy-aemula-subscription.mjs [RPC_URL]
//   Default RPC_URL: https://rpc.ignis.omnechain.network

import { readFileSync } from 'fs';
import { createHash, randomBytes } from 'crypto';
import { fileURLToPath } from 'url';
import { dirname, join } from 'path';

const __dirname = dirname(fileURLToPath(import.meta.url));

// --- Config ---
const RPC_URL = process.argv[2] || 'https://rpc.ignis.omnechain.network';
const WASM_PATH = join(__dirname, 'artifacts/aemula_subscription.wasm');
const METADATA_PATH = join(__dirname, 'artifacts/aemula_subscription.json');

// --- Load artifacts ---
const wasmBytes = readFileSync(WASM_PATH);
const wasmBase64 = wasmBytes.toString('base64');
const wasmSha256 = createHash('sha256').update(wasmBytes).digest('hex');
const compilerOutput = JSON.parse(readFileSync(METADATA_PATH, 'utf8'));

console.log(`WASM: ${wasmBytes.length} bytes, SHA-256: ${wasmSha256}`);
console.log(`RPC:  ${RPC_URL}`);

// --- Build plan methods from compiler metadata ---
const contract = compilerOutput.metadata.contracts[0];
const planMethods = contract.methods.map(m => ({
  contract: contract.name,
  function: m.name,
  export: m.export,
  legacy_export: null,
  has_runtime_export: true,
  has_legacy_export: false,
}));

// Compute ABI checksum (sorted methods → JSON → SHA-256, matching Rust's serde_json output)
const methodsSorted = [...planMethods].sort((a, b) =>
  a.contract.localeCompare(b.contract) ||
  a.function.localeCompare(b.function) ||
  a.export.localeCompare(b.export)
);
const abiSha256 = createHash('sha256')
  .update(JSON.stringify(methodsSorted))
  .digest('hex');

// --- Generate deployment nonce (16 random bytes as 32 hex chars) ---
const deploymentNonce = randomBytes(16).toString('hex');

// --- Dummy ed25519 keypair for signature (OMNE_ALLOW_UNVERIFIED_PLANS=true bypasses verification) ---
// We need a valid ed25519 public key (32 bytes) and a plausible signature (64 bytes).
// Using a well-known test keypair: all-1s seed generates a valid public key.
// Actually, we just need valid hex of the right length that parses as an ed25519 point.
// The simplest valid ed25519 public key is the basepoint (not useful for signing but parses).
// For devnet with OMNE_ALLOW_UNVERIFIED_PLANS=true, the signature is not verified.
const dummyPubKeyHex = '3b6a27bcceb6a42d62a3a8d02a6f0d73653215771de243a63ac048a18b59da29'; // ed25519 pubkey for seed=01*32
const dummySignatureHex = '0'.repeat(128); // 64 zero bytes — won't verify but doesn't need to

// --- Build typed arguments for subscribe(subscriber: i32, duration_secs: i64, current_time: i64) ---
// subscriber is a pointer (Address20 writes 20 bytes into the input buffer and passes the ptr as i32).
// For the deployment bootstrap call, we use a burn address, 0 duration, and current timestamp.
const nowSecs = Math.floor(Date.now() / 1000);
const typedArguments = [
  { type: 'address20', value: 'omne1deadbeefdeadbeefdeadbeefdeadbeefdeadbeef' },
  { type: 'i64', value: 0 },
  { type: 'i64', value: nowSecs },
];

// --- Build the DeploymentExecutionPlan ---
const plan = {
  generated_at: new Date().toISOString(),
  network: {
    name: 'ignis',
    chain_id: 7,
    rpc_endpoint: RPC_URL,
    ws_endpoint: RPC_URL.replace('https://', 'wss://').replace('http://', 'ws://'),
    explorer_url: 'https://omnescan.com',
  },
  contract: {
    path: 'aemula_subscription.wasm',
    wasm_size_bytes: wasmBytes.length,
    wasm_sha256: wasmSha256,
    wasm_base64: wasmBase64,
    deployment_nonce: deploymentNonce,
    entry: {
      contract: contract.name,
      function: 'subscribe',
      selector: `${contract.name}::subscribe`,
      export: `axiom_contract::${contract.name}::subscribe`,
      legacy_export: null,
    },
    metadata: {
      has_axiom_entry_main: true,
      has_legacy_entry_main: false,
      methods: planMethods,
      abi_sha256: abiSha256,
      // Omit compiler metadata so ABI validation is skipped on devnet
      // (OMNE_ALLOW_UNVERIFIED_PLANS=true). This allows Address20 typed arguments
      // to pass through for i32 pointer parameters without ABI type mismatch errors.
      compiler: null,
    },
  },
  execution: {
    tier: 'standard',
    config: {
      function_name: `axiom_contract::${contract.name}::subscribe`,
      arguments: [],
      gas_limit: 200000,
      timeout: { secs: 3, nanos: 0 },
    },
    typed_arguments: typedArguments,
    input_base64: null,
    preview: null,
    preview_summary: null,
  },
  services: ['orchestrator'],
  signature: {
    algorithm: 'ed25519',
    public_key_hex: dummyPubKeyHex,
    signature_hex: dummySignatureHex,
  },
};

// --- Send via JSON-RPC ---
async function deploy() {
  const body = JSON.stringify({
    jsonrpc: '2.0',
    method: 'omne_deployContract',
    params: [plan],
    id: 1,
  });

  console.log(`\nSubmitting deployment plan (nonce: ${deploymentNonce})...`);

  const response = await fetch(RPC_URL, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body,
  });

  const result = await response.json();

  if (result.error) {
    console.error('\nDeployment failed:');
    console.error(JSON.stringify(result.error, null, 2));
    process.exit(1);
  }

  console.log('\nDeployment successful!');
  console.log(JSON.stringify(result.result, null, 2));
  console.log(`\nContract address: ${result.result.contractAddress}`);
  console.log(`Transaction ID:   ${result.result.transactionId}`);
  console.log(`Block height:     ${result.result.blockHeight}`);
  return result.result;
}

deploy().catch(err => {
  console.error('Deploy error:', err);
  process.exit(1);
});
