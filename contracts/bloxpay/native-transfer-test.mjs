import { OmneClient, Wallet } from '@omne/sdk';

// Configuration via environment variables. Required:
//   OMNE_DEMO_PRIVKEY  — private key of the funded demo customer wallet
// Optional:
//   OMNE_RPC_URL       — defaults to the public Ignis RPC
//   OMNE_FROM_ADDR     — defaults to the demo customer address below
//   OMNE_TO_ADDR       — defaults to the demo merchant address below
//   OMNE_AMOUNT_QUAR   — defaults to 2.5 OMC in quar
//   OMNE_NONCE         — manual nonce override (see comment in main())
//
// Run example:
//   OMNE_DEMO_PRIVKEY=<hex> node contracts/bloxpay/native-transfer-test.mjs
//
// To generate a demo wallet:
//   node -e "import('@omne/sdk').then(m => { const w = m.Wallet.create(); console.log('addr:', w.address); console.log('privkey:', w.privateKey); })"
// Fund the resulting address from the Ignis testnet faucet, then export
// OMNE_DEMO_PRIVKEY=<that key> before running this test.

const RPC_URL = process.env.OMNE_RPC_URL || 'http://144.202.60.67:26657';

const FROM_PRIVKEY = process.env.OMNE_DEMO_PRIVKEY;
if (!FROM_PRIVKEY) {
  console.error('ERROR: OMNE_DEMO_PRIVKEY env var is required.');
  console.error('See header comment for setup instructions.');
  process.exit(1);
}

// Demo customer wallet (default: known-funded testnet address)
const FROM_ADDR = process.env.OMNE_FROM_ADDR || 'om1zanywrcddx404mhzkrd6rys6477q8hjuddwqu0h';

// Demo merchant — Greg's Test Coffee Co. (starts at 0 OMC)
const TO_ADDR = process.env.OMNE_TO_ADDR || 'om1zxa28hvfnpw0k8676x4pfgxeky7p9hrf5auf6cw';

// 2.5 OMC default (bumped from prior 1 OMC to dodge cached tx hash on retry)
const AMOUNT_QUAR = process.env.OMNE_AMOUNT_QUAR
  ? BigInt(process.env.OMNE_AMOUNT_QUAR)
  : 2_500_000_000_000_000_000n;

const signer = Wallet.fromPrivateKey(FROM_PRIVKEY);

async function rpc(method, params) {
  const r = await fetch(RPC_URL, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ jsonrpc: '2.0', id: 1, method, params }),
  });
  return r.json();
}

async function main() {
  console.log('Native-transfer test: 1 OMC customer → merchant');
  console.log(`  RPC:  ${RPC_URL}`);
  console.log(`  from: ${FROM_ADDR}`);
  console.log(`  to:   ${TO_ADDR}`);

  const fromBefore = await rpc('omne_getBalance', [FROM_ADDR]);
  const toBefore = await rpc('omne_getBalance', [TO_ADDR]);
  console.log(`\nBalances BEFORE:`);
  console.log(`  customer: ${JSON.stringify(fromBefore.result)}`);
  console.log(`  merchant: ${JSON.stringify(toBefore.result)}`);

  const nonceRes = await rpc('omne_getNonce', [FROM_ADDR]);
  // Native transfers don't bump the on-chain nonce counter exposed via RPC
  // yet (Phase 7c-1 follow-up), so we increment locally each run to dodge
  // mempool dedupe. Override via env if you know better.
  const overrideNonce = process.env.OMNE_NONCE;
  const nonce = overrideNonce ? Number(overrideNonce) : Number(nonceRes.result ?? 0);
  console.log(`\n  nonce: ${nonce} (rpc reported ${nonceRes.result})`);

  const unsigned = {
    from: FROM_ADDR,
    to: TO_ADDR,
    value: AMOUNT_QUAR.toString(),
    data: '',
    nonce,
    chainId: 3,
    gasLimit: 21000,
    gasPrice: '1000',
  };

  const signed = signer.signTransaction(unsigned);
  const submitTx = {
    from: signed.from,
    to: signed.to,
    value: signed.value,
    data: signed.data,
    nonce: signed.nonce,
    chainId: signed.chainId,
    gasLimit: signed.gasLimit,
    gasPrice: signed.gasPrice,
    signature: { signature: signed.signature, publicKey: signed.publicKey },
  };

  console.log('\nSubmitting native transfer...');
  const sendRes = await rpc('omne_sendTransaction', [submitTx]);
  if (sendRes.error) {
    console.log(`  ✗ ERROR: ${JSON.stringify(sendRes.error)}`);
    return;
  }
  const txHash = sendRes.result?.transactionHash || sendRes.result;
  console.log(`  → tx: ${txHash}`);

  console.log('\nWaiting 10s for inclusion...');
  await new Promise(r => setTimeout(r, 10000));

  const receipt = await rpc('omne_getTransactionReceipt', [txHash]);
  console.log(`\nReceipt: ${JSON.stringify(receipt.result || receipt.error, null, 2)}`);

  const fromAfter = await rpc('omne_getBalance', [FROM_ADDR]);
  const toAfter = await rpc('omne_getBalance', [TO_ADDR]);
  console.log(`\nBalances AFTER:`);
  console.log(`  customer: ${JSON.stringify(fromAfter.result)}`);
  console.log(`  merchant: ${JSON.stringify(toAfter.result)}`);
}

main().catch(e => { console.error(e); process.exit(1); });
