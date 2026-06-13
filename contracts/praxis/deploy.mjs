#!/usr/bin/env node
// deploy.mjs
// Deploys the praxis_permissions contract to a local or remote Omne network.
// Constructs a DeploymentExecutionPlan and sends it via omne_deployContract RPC.
//
// Usage: node deploy.mjs [RPC_URL]
//   Default RPC_URL: http://127.0.0.1:26657 (local Ignis seed validator)
//
// Bootstrap entry at deploy: get_status(burn_token_id) → returns 0 (read-only,
// no state mutation, smallest possible side-effect surface).
//
// The node must be started with OMNE_ALLOW_UNVERIFIED_PLANS=true for this
// script to deploy (no real signature). For production, regenerate using a
// signed metadata + signed plan.

import { readFileSync, writeFileSync } from 'fs';
import { createHash, randomBytes } from 'crypto';
import { fileURLToPath } from 'url';
import { dirname, join } from 'path';

const __dirname = dirname(fileURLToPath(import.meta.url));

// --- bech32m (BIP-350) encoder, no deps ---
// The chain is uniformly 32-byte (PQC). A contract address is the bech32m
// encoding of the FULL 32-byte SHA-256 deployment digest under HRP "om" +
// witness version 2 (the same scheme as demo/src/address.ts:encodeAddress).
const BECH32M_CHARSET = 'qpzry9x8gf2tvdw0s3jn54khce6mua7l';
const BECH32M_CONST = 0x2bc830a3;
function bech32mPolymod(values) {
  let chk = 1;
  const GEN = [0x3b6a57b2, 0x26508e6d, 0x1ea119fa, 0x3d4233dd, 0x2a1462b3];
  for (const v of values) {
    const top = chk >> 25;
    chk = ((chk & 0x1ffffff) << 5) ^ v;
    for (let i = 0; i < 5; i++) if ((top >> i) & 1) chk ^= GEN[i];
  }
  return chk;
}
function bech32mHrpExpand(hrp) {
  const r = [];
  for (let i = 0; i < hrp.length; i++) r.push(hrp.charCodeAt(i) >> 5);
  r.push(0);
  for (let i = 0; i < hrp.length; i++) r.push(hrp.charCodeAt(i) & 31);
  return r;
}
function bech32mChecksum(hrp, data) {
  const values = bech32mHrpExpand(hrp).concat(data).concat([0, 0, 0, 0, 0, 0]);
  const mod = bech32mPolymod(values) ^ BECH32M_CONST;
  const r = [];
  for (let i = 0; i < 6; i++) r.push((mod >> (5 * (5 - i))) & 31);
  return r;
}
function bech32mConvertBits(data, from, to, pad) {
  let acc = 0, bits = 0;
  const ret = [];
  const maxv = (1 << to) - 1;
  for (const b of data) {
    acc = (acc << from) | b;
    bits += from;
    while (bits >= to) { bits -= to; ret.push((acc >> bits) & maxv); }
  }
  if (pad && bits > 0) ret.push((acc << (to - bits)) & maxv);
  return ret;
}
function encodeOmneAddress(payload32) {
  const data = [2].concat(bech32mConvertBits(Array.from(payload32), 8, 5, true));
  const combined = data.concat(bech32mChecksum('om', data));
  let out = 'om1';
  for (const d of combined) out += BECH32M_CHARSET[d];
  return out;
}

// --- Config ---
const RPC_URL = process.argv[2] || 'http://127.0.0.1:26657';
const WASM_PATH = join(__dirname, 'praxis.wasm');
const METADATA_PATH = join(__dirname, 'praxis.json');
const ADDR_OUT_PATH = join(__dirname, 'deployed-address.json');

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

// --- ML-DSA-44 plan signature (OMNE_ALLOW_UNVERIFIED_PLANS=true bypasses
// signature VERIFICATION, but the node still validates key/sig LENGTHS).
// Use a real 1312-byte ML-DSA-44 public key from the demo principal wallet
// + a correct-length (2420-byte) zero signature. The chain is uniformly
// post-quantum; the legacy 32-byte ed25519 dummy is no longer accepted.
const ML_DSA_44_PUBKEY_BYTES = 1312;
const ML_DSA_44_SIG_BYTES = 2420;
const principalWallet = JSON.parse(
  readFileSync('/Users/gregbrown/github/praxis/demo/wallets.json', 'utf8'),
).principal;
const dummyPubKeyHex = principalWallet.publicKey;
if (dummyPubKeyHex.length !== ML_DSA_44_PUBKEY_BYTES * 2) {
  throw new Error(`expected ${ML_DSA_44_PUBKEY_BYTES}-byte ML-DSA-44 pubkey, got ${dummyPubKeyHex.length / 2}`);
}
const dummySignatureHex = '0'.repeat(ML_DSA_44_SIG_BYTES * 2);

// --- Bootstrap: get_status(burn_token_id) ---
// Pure read; returns 0 because the burn token_id has never had a permission minted.
// 32-byte PQC om1z burn address (witness version 2, 32-byte payload of 0xDE).
// The chain is uniformly 32-byte post-migration — address32 is the only
// accepted address arg type.
const BURN_TOKEN_ID = 'om1zmm0dahk7mm0dahk7mm0dahk7mm0dahk7mm0dahk7mm0dahk7mm0qdtuxap';
const typedArguments = [{ type: 'address32', value: BURN_TOKEN_ID }];

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
    path: 'praxis.wasm',
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

  console.log('\n✅ Deployment successful!');
  console.log(JSON.stringify(result.result, null, 2));

  // Derive the contract address locally. The node bech32m-encodes the FULL
  // 32-byte SHA-256 digest (witness v2, HRP "om") — NOT a truncated 20-byte
  // 'omne1'+hex string. This must match the node's derivation and config.ts.
  const addrHasher = createHash('sha256');
  addrHasher.update(wasmBytes);
  addrHasher.update(contract.name);
  addrHasher.update('get_status');
  addrHasher.update(`axiom_contract::${contract.name}::get_status`);
  addrHasher.update(Buffer.from(deploymentNonce, 'hex'));
  const addrDigest = addrHasher.digest(); // 32 bytes
  const contractAddress = encodeOmneAddress(addrDigest);

  console.log(`\nContract address (derived): ${contractAddress}`);
  if (result.result?.transactionHash) {
    console.log(`Transaction hash: ${result.result.transactionHash}`);
  }

  // Persist the deployment details so the Praxis SDK can pick them up.
  const out = {
    contract_name: contract.name,
    contract_address: contractAddress,
    rpc_url: RPC_URL,
    chain_id: 3,
    wasm_sha256: wasmSha256,
    deployment_nonce: deploymentNonce,
    transaction_hash: result.result?.transactionHash || null,
    deployed_at: new Date().toISOString(),
  };
  writeFileSync(ADDR_OUT_PATH, JSON.stringify(out, null, 2));
  console.log(`\nDeployment details written to ${ADDR_OUT_PATH}`);

  return result.result;
}

deploy().catch((err) => {
  console.error('Deploy error:', err);
  process.exit(1);
});
