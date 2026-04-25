#!/usr/bin/env node
// deploy.mjs
// Deploys the blox_pay payment-intent registry contract to the Omne devnet.
// Constructs a DeploymentExecutionPlan and sends it via omne_deployContract RPC.
//
// Usage: node deploy.mjs [RPC_URL]
//   Default RPC_URL: https://rpc.ignis.omnechain.network
//
// The blox_pay contract supports payment intents:
//   - create_intent(link_id, merchant, amount_quar, current_time)
//   - mark_paid(link_id, payer, paid_amount, current_time)
//   - void_intent(link_id, current_time)
//   - get_status / get_merchant / get_amount / get_payer / get_paid_amount /
//     get_created_at / get_paid_at
//
// Bootstrap entry at deploy: get_status(burn_link_id) → returns 0 (read-only,
// no state mutation, smallest possible side-effect surface).

import { readFileSync } from 'fs';
import { createHash, randomBytes } from 'crypto';
import { fileURLToPath } from 'url';
import { dirname, join } from 'path';

const __dirname = dirname(fileURLToPath(import.meta.url));

// --- Config ---
const RPC_URL = process.argv[2] || 'https://rpc.ignis.omnechain.network';
const WASM_PATH = join(__dirname, 'blox_pay.wasm');
const METADATA_PATH = join(__dirname, 'blox_pay.json');

// --- Load artifacts ---
const wasmBytes = readFileSync(WASM_PATH);
const wasmBase64 = wasmBytes.toString('base64');
const wasmSha256 = createHash('sha256').update(wasmBytes).digest('hex');
const compilerOutput = JSON.parse(readFileSync(METADATA_PATH, 'utf8'));

console.log(`WASM: ${wasmBytes.length} bytes, SHA-256: ${wasmSha256}`);
console.log(`RPC:  ${RPC_URL}`);

// --- Build plan methods from compiler metadata ---
const contract = compilerOutput.metadata.contracts[0];
const planMethods = contract.methods.map((m) => ({
  contract: contract.name,
  function: m.name,
  export: m.export,
  legacy_export: null,
  has_runtime_export: true,
  has_legacy_export: false,
}));

// Compute ABI checksum (sorted methods → JSON → SHA-256, matching Rust's serde_json output)
const methodsSorted = [...planMethods].sort(
  (a, b) =>
    a.contract.localeCompare(b.contract) ||
    a.function.localeCompare(b.function) ||
    a.export.localeCompare(b.export),
);
const abiSha256 = createHash('sha256')
  .update(JSON.stringify(methodsSorted))
  .digest('hex');

// --- Generate deployment nonce (16 random bytes as 32 hex chars) ---
const deploymentNonce = randomBytes(16).toString('hex');

// --- Dummy ed25519 keypair for signature (OMNE_ALLOW_UNVERIFIED_PLANS=true bypasses verification) ---
const dummyPubKeyHex =
  '3b6a27bcceb6a42d62a3a8d02a6f0d73653215771de243a63ac048a18b59da29';
const dummySignatureHex = '0'.repeat(128);

// --- Bootstrap: get_status(burn_link_id) ---
// Pure read; returns 0 because the burn link_id has never had an intent created.
// No state mutation, no side effects. Burn address is 20 bytes of 0xde 0xad
// alternating, bech32m-encoded with HRP "om" + witness version 2.
const BURN_LINK_ID = 'om1zm6kaatw74h02mh4dm6kaatw74h02mh4dxy2f3j';
const typedArguments = [{ type: 'address20', value: BURN_LINK_ID }];

// --- Build the DeploymentExecutionPlan ---
const plan = {
  generated_at: new Date().toISOString(),
  network: {
    name: 'ignis',
    chain_id: 3,
    rpc_endpoint: RPC_URL,
    ws_endpoint: RPC_URL.replace('https://', 'wss://').replace('http://', 'ws://'),
    explorer_url: 'https://omnescan.com',
  },
  contract: {
    path: 'blox_pay.wasm',
    wasm_size_bytes: wasmBytes.length,
    wasm_sha256: wasmSha256,
    wasm_base64: wasmBase64,
    deployment_nonce: deploymentNonce,
    entry: {
      contract: contract.name,
      function: 'get_status',
      selector: `${contract.name}::get_status`,
      export: `axiom_contract::${contract.name}::get_status`,
      legacy_export: null,
    },
    metadata: {
      has_axiom_entry_main: true,
      has_legacy_entry_main: false,
      methods: planMethods,
      abi_sha256: abiSha256,
      compiler: null,
    },
  },
  execution: {
    tier: 'standard',
    config: {
      function_name: `axiom_contract::${contract.name}::get_status`,
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

  // Derive the contract address locally (same SHA-256 algorithm as the node).
  // address = SHA-256(wasm || contractName || entryFunction || export || nonce)[0:20]
  const addrHasher = createHash('sha256');
  addrHasher.update(wasmBytes);
  addrHasher.update(contract.name);
  addrHasher.update('get_status');
  addrHasher.update(`axiom_contract::${contract.name}::get_status`);
  addrHasher.update(Buffer.from(deploymentNonce, 'hex'));
  const addrDigest = addrHasher.digest();
  const contractAddress = 'omne1' + addrDigest.subarray(0, 20).toString('hex');

  console.log(`\nContract address (derived): ${contractAddress}`);
  if (result.result?.transactionHash) {
    console.log(`Transaction hash: ${result.result.transactionHash}`);
  }
  console.log(`\nSet in Blox backend .env: BLOXPAY_CONTRACT_ADDRESS=${contractAddress}`);
  return result.result;
}

deploy().catch((err) => {
  console.error('Deploy error:', err);
  process.exit(1);
});
