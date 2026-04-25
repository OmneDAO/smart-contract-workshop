import { encodeContractCall, AbiEncode, Wallet } from '@omne/sdk';

const RPC_URL = 'https://rpc.ignis.omnechain.network';
const CONTRACT = 'om1ztd9tqdcj9539mtca0g4q52r92jegavmxclv7s0';
const TEST_LINK = 'om1z7ewdaayh4ffxwmgans4tna0v00qyez3dd86a8y';
const TREASURY = 'om1z0a886l5xfyx2egguyd256xdlegkgl3acajjx7x';
const TREASURY_KEY = '0ac74d47a70819839a880949bdbc2f2ef979779eac5cc959740dc4a9189c3a64';

const signer = Wallet.fromPrivateKey(TREASURY_KEY);
async function rpc(m, p) {
  const r = await fetch(RPC_URL, {method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({jsonrpc:'2.0',method:m,params:p,id:1})});
  return r.json();
}

// Get fresh nonce
const nonceRes = await rpc('omne_getNonce', [TREASURY]);
let nonce = Number(nonceRes.result ?? 0);
console.log('starting nonce:', nonce);

async function send(label, data) {
  const unsigned = {
    from: TREASURY, to: CONTRACT, value: '0', data, nonce: nonce++,
    chainId: 3, gasLimit: 200000, gasPrice: '1000',
  };
  const signed = signer.signTransaction(unsigned);
  const submitTx = {
    from: signed.from, to: signed.to, value: signed.value, data: signed.data,
    nonce: signed.nonce, chainId: signed.chainId, gasLimit: signed.gasLimit, gasPrice: signed.gasPrice,
    signature: { signature: signed.signature, publicKey: signed.publicKey },
  };
  const r = await rpc('omne_sendTransaction', [submitTx]);
  console.log(`\n[${label}] submit:`, r.result?.transactionHash || JSON.stringify(r.error));
  if (r.result?.transactionHash) {
    await new Promise(x=>setTimeout(x,5000));
    const rec = await rpc('omne_getTransactionReceipt', [r.result.transactionHash]);
    console.log(`[${label}] receipt:`, JSON.stringify(rec.result || rec.error));
  }
}

// Test 1: empty data → invokes default entry (get_status with no args)
await send('EMPTY-DATA', '');

// Test 2: ABI-encoded get_status(link_id)
const getStatusData = encodeContractCall('blox_pay::get_status', [AbiEncode.address(TEST_LINK)]);
await send('ABI get_status', getStatusData);

// Test 3: ABI-encoded with full export name
const getStatusFull = encodeContractCall('axiom_contract::blox_pay::get_status', [AbiEncode.address(TEST_LINK)]);
await send('ABI full export', getStatusFull);
