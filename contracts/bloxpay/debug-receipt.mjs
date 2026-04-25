import {} from 'fs';
const RPC = 'https://rpc.ignis.omnechain.network';
const txs = [
  'txn_aebed72a298a5d20177df8c034f1751684a1ad2dfd9d6115eee1c55124609fc3',
  'txn_6db395a247281204ee18d39df83c36299040048fe172f47268c0e3f8ea7d2cb0',
  'txn_868c1910869dc03004839e2e6a30fa875aa5cbc004eb45ff0a7557af8d6f5600',
];
for (const h of txs) {
  const res = await fetch(RPC, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ jsonrpc: '2.0', method: 'omne_getTransactionReceipt', params: [h], id: 1 }),
  });
  const j = await res.json();
  console.log(`\n${h}:`);
  console.log(JSON.stringify(j.result || j.error, null, 2));
}
