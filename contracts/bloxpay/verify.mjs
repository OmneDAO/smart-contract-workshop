#!/usr/bin/env node
// verify.mjs
// End-to-end verification of the deployed BloxPay contract on Ignis devnet.
//
// Walks the full payment lifecycle:
//   1. create_intent  → status = 1 (pending)
//   2. mark_escrowed  → status = 2 (escrowed); release_after = 0 for instant release
//   3. release_to_merchant → status = 3 (settled)
//
// All calls are signed by the Blox treasury wallet.
//
// Note: amounts are capped at ~9 OMC for v1 because pysub uint128 compiles to
// WASM i64. $25 demo will require cents-denominated amount tracking on-chain.
//
// Usage:  node verify.mjs

import { encodeContractCall, AbiEncode, OmneClient, Wallet } from '@omne/sdk';

const RPC_URL = 'https://rpc.ignis.omnechain.network';

// Live deployed contract (from deploy.mjs)
const CONTRACT_ADDR = 'om1zfue6neu4ym5gc5e07qtd5z9r33f5r8xf4rmxt5';

// Test data
const TEST_LINK_ID = 'om1z7ewdaayh4ffxwmgans4tna0v00qyez3dd86a8y';
const TEST_MERCHANT = 'om1zxa28hvfnpw0k8676x4pfgxeky7p9hrf5auf6cw'; // Greg's Test Coffee Co.
const ESCROW_ADDR = 'om1z0a886l5xfyx2egguyd256xdlegkgl3acajjx7x';   // Blox treasury (escrow for v1)
const TEST_CUSTOMER = 'om1zgy7vrrphvwc8r7pr8fn9c3a9dhvvnkda7uz0np'; // any valid om1z

// 5 OMC = 5 * 10^18 quar (fits in i64; v1 limit, see file header)
const AMOUNT_QUAR = 5_000_000_000_000_000_000n;

// release_after = 0 → release_to_merchant succeeds at any current_time
const RELEASE_AFTER = 0n;

// Caller — Blox treasury (signs every contract call)
const TREASURY_ADDR = 'om1z0a886l5xfyx2egguyd256xdlegkgl3acajjx7x';
const TREASURY_PRIVKEY = '0ac74d47a70819839a880949bdbc2f2ef979779eac5cc959740dc4a9189c3a64';

// ── Helpers ────────────────────────────────────────────────────────────

const nowSec = () => BigInt(Math.floor(Date.now() / 1000));

async function rawRpc(method, params = []) {
  const res = await fetch(RPC_URL, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ jsonrpc: '2.0', method, params, id: 1 }),
  });
  return res.json();
}

async function readStatus(label) {
  const data = encodeContractCall('blox_pay::get_status', [AbiEncode.address(TEST_LINK_ID)]);
  const res = await rawRpc('omne_call', [{ to: CONTRACT_ADDR, data }]);
  console.log(`  ${label}: ${JSON.stringify(res.result || res.error)}`);
  return res;
}

// Cached nonce, incremented locally per-call to avoid re-fetching after
// each tx (txs in the same block share the same on-chain nonce until
// the block lands).
let _treasuryNonce = null;
async function nextNonce() {
  if (_treasuryNonce === null) {
    const res = await rawRpc('omne_getNonce', [TREASURY_ADDR]);
    _treasuryNonce = Number(res.result ?? 0);
    console.log(`  → treasury nonce starting at ${_treasuryNonce}`);
  }
  return _treasuryNonce++;
}

const treasurySigner = Wallet.fromPrivateKey(TREASURY_PRIVKEY);

async function sendCall(method, args, label) {
  const qualifiedMethod = `blox_pay::${method}`;
  const data = encodeContractCall(qualifiedMethod, args);
  const nonce = await nextNonce();

  const unsignedTx = {
    from: TREASURY_ADDR,
    to: CONTRACT_ADDR,
    value: '0',
    data,
    nonce,
    chainId: 3,
    gasLimit: 200_000,
    gasPrice: '1000',
  };

  // SDK signs flat (signature + publicKey at top level); chain expects nested
  // (signature: { signature, publicKey }). Rewrap before submit.
  const signed = treasurySigner.signTransaction(unsignedTx);
  const submitTx = {
    from: signed.from,
    to: signed.to,
    value: signed.value,
    data: signed.data,
    nonce: signed.nonce,
    chainId: signed.chainId,
    gasLimit: signed.gasLimit,
    gasPrice: signed.gasPrice,
    signature: {
      signature: signed.signature,
      publicKey: signed.publicKey,
    },
  };

  console.log(`\n── ${label} ──`);
  console.log(`  method: ${method}`);
  const res = await rawRpc('omne_sendTransaction', [submitTx]);
  if (res.error) {
    console.log(`  ✗ ERROR: ${JSON.stringify(res.error)}`);
    return res;
  }
  console.log(`  → tx: ${res.result?.transactionHash || JSON.stringify(res.result)}`);
  return res;
}

async function waitForBlock(seconds = 6) {
  console.log(`  ⏳ waiting ${seconds}s for block inclusion...`);
  await new Promise((r) => setTimeout(r, seconds * 1000));
}

// ── Main lifecycle ────────────────────────────────────────────────────

async function main() {
  console.log('BloxPay verification cycle');
  console.log(`  RPC:          ${RPC_URL}`);
  console.log(`  Contract:     ${CONTRACT_ADDR}`);
  console.log(`  Test link_id: ${TEST_LINK_ID}`);
  console.log(`  Merchant:     ${TEST_MERCHANT}`);
  console.log(`  Escrow:       ${ESCROW_ADDR}`);
  console.log(`  Amount:       ${AMOUNT_QUAR} quar (5 OMC)`);
  console.log(`  Release:      after timestamp ${RELEASE_AFTER} (immediate)`);

  // 0. Pre-flight: confirm contract is callable + intent slot is empty
  console.log('\n=== 0. Pre-flight: get_status (expect 0 = not_created) ===');
  await readStatus('initial status');

  // 1. create_intent
  await sendCall(
    'create_intent',
    [
      AbiEncode.address(TEST_LINK_ID),
      AbiEncode.address(TEST_MERCHANT),
      AbiEncode.u128(AMOUNT_QUAR),
      AbiEncode.address(ESCROW_ADDR),
      AbiEncode.u128(nowSec()),
    ],
    '1. create_intent',
  );
  await waitForBlock();
  console.log('\n=== status after create_intent (expect 1 = pending) ===');
  await readStatus('post-create');

  // 2. mark_escrowed (simulating customer paid into escrow)
  await sendCall(
    'mark_escrowed',
    [
      AbiEncode.address(TEST_LINK_ID),
      AbiEncode.address(TEST_CUSTOMER),
      AbiEncode.u128(AMOUNT_QUAR),
      AbiEncode.u128(RELEASE_AFTER),
      AbiEncode.u128(nowSec()),
    ],
    '2. mark_escrowed',
  );
  await waitForBlock();
  console.log('\n=== status after mark_escrowed (expect 2 = escrowed) ===');
  await readStatus('post-escrow');

  // 3. release_to_merchant (release_after=0 so always passes)
  await sendCall(
    'release_to_merchant',
    [AbiEncode.address(TEST_LINK_ID), AbiEncode.u128(nowSec())],
    '3. release_to_merchant',
  );
  await waitForBlock();
  console.log('\n=== status after release_to_merchant (expect 3 = settled) ===');
  await readStatus('post-release');

  // 4. Final state dump — read all the getters
  console.log('\n=== Final on-chain state for this intent ===');
  const getters = [
    ['get_status', 'u128'],
    ['get_merchant', 'address'],
    ['get_amount', 'u128'],
    ['get_escrow_addr', 'address'],
    ['get_payer', 'address'],
    ['get_paid_amount', 'u128'],
    ['get_paid_at', 'u128'],
    ['get_settled_at', 'u128'],
    ['get_release_after', 'u128'],
  ];
  for (const [method, _kind] of getters) {
    const data = encodeContractCall(`blox_pay::${method}`, [AbiEncode.address(TEST_LINK_ID)]);
    const res = await rawRpc('omne_call', [{ to: CONTRACT_ADDR, data }]);
    console.log(`  ${method}: ${JSON.stringify(res.result ?? res.error)}`);
  }

  console.log('\n✅ Verification cycle complete.');
}

main().catch((err) => {
  console.error('Verify failed:', err);
  process.exit(1);
});
