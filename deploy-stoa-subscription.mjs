#!/usr/bin/env node
// deploy-stoa-subscription.mjs
// Deploys the Stoa subscription contract to the Omne devnet.
// Constructs a DeploymentExecutionPlan and sends it via omne_deployContract RPC.
//
// Usage: node deploy-stoa-subscription.mjs [RPC_URL]
//   Default RPC_URL: https://rpc.ignis.omnechain.network
//
// The stoa_subscription contract supports per-author subscriptions:
//   - register_author(author, price_quar)
//   - subscribe(subscriber, author, payment_amount, current_time)
//   - is_active(subscriber, author, current_time)
//   - get_expiry(subscriber, author)
//   - get_price(author)
//   - is_registered(author)

import { readFileSync } from 'fs';
import { createHash, randomBytes } from 'crypto';
import { fileURLToPath } from 'url';
import { dirname, join } from 'path';

const __dirname = dirname(fileURLToPath(import.meta.url));

// --- Config ---
const RPC_URL = process.argv[2] || 'https://rpc.ignis.omnechain.network';
const WASM_PATH = join(__dirname, 'artifacts/stoa_subscription.wasm');
const METADATA_PATH = join(__dirname, 'artifacts/stoa_subscription.json');

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
const dummyPubKeyHex = '3b6a27bcceb6a42d62a3a8d02a6f0d73653215771de243a63ac048a18b59da29';
const dummySignatureHex = '0'.repeat(128);

// --- Bootstrap call: register_author with a burn address and 0 price ---
// This initialises the contract's state on deployment. The actual author
// registration will happen via RPC calls after deployment.
const nowSecs = Math.floor(Date.now() / 1000);
const typedArguments = [
  { type: 'address20', value: 'omne1deadbeefdeadbeefdeadbeefdeadbeefdeadbeef' },
  { type: 'i64', value: 0 },
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
    path: 'stoa_subscription.wasm',
    wasm_size_bytes: wasmBytes.length,
    wasm_sha256: wasmSha256,
    wasm_base64: wasmBase64,
    deployment_nonce: deploymentNonce,
    entry: {
      contract: contract.name,
      function: 'register_author',
      selector: `${contract.name}::register_author`,
      export: `axiom_contract::${contract.name}::register_author`,
      legacy_export: null,
    },
    metadata: {
      has_axiom_entry_main: true,
      has_legacy_entry_main: false,
      methods: planMethods,
      abi_sha256: abiSha256,
      // Omit compiler metadata so ABI validation is skipped on devnet
      // (OMNE_ALLOW_UNVERIFIED_PLANS=true).
      compiler: null,
    },
  },
  execution: {
    tier: 'standard',
    config: {
      function_name: `axiom_contract::${contract.name}::register_author`,
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
  addrHasher.update('register_author');
  addrHasher.update(`axiom_contract::${contract.name}::register_author`);
  addrHasher.update(Buffer.from(deploymentNonce, 'hex'));
  const addrDigest = addrHasher.digest();
  const contractAddress = 'omne1' + addrDigest.subarray(0, 20).toString('hex');

  console.log(`\nContract address (derived): ${contractAddress}`);
  console.log(`Transaction hash: ${result.result.transactionHash}`);
  console.log(`\nSet in your .env: NEXT_PUBLIC_SUBSCRIPTION_CONTRACT=${contractAddress}`);
  return result.result;
}

deploy().catch(err => {
  console.error('Deploy error:', err);
  process.exit(1);
});
