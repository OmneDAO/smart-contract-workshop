# Omne TypeScript SDK

Official TypeScript/JavaScript SDK for Omne Blockchain - the commerce-first blockchain with dual-layer consensus and microscopic fees.

## Features

- 🔗 **Full Omne Integration**: Complete support for dual-layer PoVERA consensus
- 💰 **Microscopic Fees**: Quar-precision arithmetic (18-decimal precision)
- 🔐 **BIP39 HD Wallets**: Secure wallet generation and management
- 🪙 **ORC-20 Tokens**: Deploy and interact with Omne token standard
- 🤖 **Computational Jobs**: Submit work to Omne Orchestration Network (OON)
- ⚡ **Async/Await**: Modern JavaScript patterns throughout
- 🔒 **Type Safety**: Full TypeScript support with comprehensive type definitions
- 🌐 **Cross-Platform**: Works in Node.js and modern browsers

## Installation

```bash
npm install @omne/sdk
```

> **Node fetch requirement**
>
> The SDK relies on the global `fetch` API. Modern browsers and Node.js 18+ provide it natively. If you target Node.js 16 or 17, install a polyfill and the SDK will auto-detect it at runtime:
>
> ```bash
> npm install node-fetch@^3.3.2
> ```
>
> No extra configuration is required—`@omne/sdk` will import the polyfill when `globalThis.fetch` is unavailable.

Or with yarn:

```bash
yarn add @omne/sdk
```

## Quick Start

### Basic Usage

```typescript
import { OmneClient, Wallet, toQuar, createClient } from '@omne/sdk';

// Connect to Omne network
const client = createClient('testum'); // or 'primum' for local dev
await client.connect();

// Create or import wallet
const wallet = Wallet.generate();
console.log('Address:', wallet.address);
console.log('Mnemonic:', wallet.getMnemonic());

// Get network information
const networkInfo = await client.getNetworkInfo();
console.log('Chain ID:', networkInfo.chainId);
console.log('Latest block:', networkInfo.latestBlock);
```

### Deployment Metadata APIs

The SDK can query the hardened deployment metadata service that Phase 3 introduced on the node. These helpers use the same base URL as hardened deployments by default (`/v1/plans`, `/v1/provenance`, etc.) and automatically include bearer tokens when configured.

```typescript
import { OmneClient } from '@omne/sdk';

const client = new OmneClient({
  url: 'https://testnet-rpc.omne.network',
  // Optional: override when metadata is hosted separately
  // metadataBaseUrl: 'https://metadata.omne.network/v1/',
  authToken: process.env.OMNE_AUTH_TOKEN,
});

// List recorded deployment plans
const plans = await client.listDeploymentPlans({ pageSize: 10, network: 'testnet' });
plans.plans.forEach((plan) => {
  console.log(plan.planId, plan.services);
});

// Fetch details by plan identifier or digest
const plan = await client.getDeploymentPlan('pln_abcd1234');
const digestCopy = await client.getDeploymentPlanByDigest(plan?.plan.digest ?? '');

// Look up nonce provenance (SHA-256 hash of the deployment nonce)
const provenance = await client.getNonceProvenance('f29c...');
```

### Send Transaction

```typescript
// Send OMC with microscopic fees
const receipt = await client.transfer({
  from: wallet.address,
  to: '0x742d35cc4bf688aee6f7c3c3a6b1c98aaee5e84e',
  valueOMC: '0.001', // 0.001 OMC
  priority: 'commerce' // Use 3-second commerce layer
});

console.log('Transaction confirmed:', receipt.transactionHash);
console.log('Gas used:', receipt.gasUsed);
console.log('Confirmation time:', receipt.confirmationTime, 'ms');
```

### Deploy ORC-20 Token

```typescript
// Deploy token with microscopic fee inheritance
const { token, receipt } = await client.deployORC20Token({
  name: 'My App Token',
  symbol: 'MAT',
  totalSupply: '1000000',
  from: wallet.address,
  config: {
    inheritsMicroscopicFees: true, // 50% fee discount
    mintable: true,
    burnable: false
  }
});

console.log('Token deployed at:', token.address);
console.log('Transaction cost:', formatBalance(receipt.gasUsed * 1000), 'cents');
```

### Quar Precision Arithmetic

```typescript
import { toQuar, fromQuar, formatBalance } from '@omne/sdk';

// Convert between OMC and quar (18-decimal precision)
const omcAmount = '1.5';
const quarAmount = toQuar(omcAmount);
console.log(quarAmount); // '1500000000000000000'

const backToOMC = fromQuar(quarAmount);
console.log(backToOMC.toString()); // '1.5'

// Format for display
console.log(formatBalance(quarAmount)); // '1.500000 OMC'

// Commerce calculations
const coffeePrice = toQuar('0.003');
const gasCost = BigInt(21000) * BigInt('1000'); // 21k gas * 1000 quar/gas
const totalCost = BigInt(coffeePrice) + gasCost;
console.log('Coffee + gas:', formatBalance(totalCost.toString()));
```

### Wallet Management

```typescript
// Generate new wallet
const wallet = Wallet.generate();

// Import from mnemonic
const imported = Wallet.fromMnemonic(
  'abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about'
);

// Derive multiple accounts
const accounts = wallet.getAccounts(5); // Get first 5 accounts
accounts.forEach((account, i) => {
  console.log(`Account ${i}: ${account.address}`);
});

// Sign transactions
const account = wallet.getAccount(0);
const signedTx = account.signTransaction({
  from: account.address,
  to: '0x...',
  value: toQuar('0.1'),
  gasLimit: 21000,
  gasPrice: '1000',
  nonce: 0
});
```

### Submit Computational Job

```typescript
// Submit AI/ML job to Omne Orchestration Network
const job = await client.submitComputationalJob({
  jobType: 'ml_training',
  dataSource: 'https://example.com/dataset.csv',
  parameters: {
    model: 'neural_network',
    epochs: 100,
    learningRate: 0.001
  },
  maxCostOMC: '10.0',
  timeoutMinutes: 60,
  priority: 5
});

console.log('Job submitted:', job.jobId);

// Monitor job progress
const status = await client.getJobStatus(job.jobId);
console.log('Progress:', status.progress, '%');
```

### Event Subscriptions (WebSocket)

```typescript
// Subscribe to new blocks
const subscription = await client.subscribe('newBlock', (block) => {
  console.log('New block:', block.number);
  console.log('Layer:', block.layer); // 'commerce' or 'security'
  console.log('Transactions:', block.transactionCount);
});

// Subscribe to token transfers
const tokenSub = await client.subscribe('tokenTransfer', (transfer) => {
  console.log('Token transfer:', transfer.from, '->', transfer.to);
  console.log('Amount:', formatBalance(transfer.value));
});

// Unsubscribe when done
await subscription.unsubscribe();
```

## Network Configuration

### Supported Networks

| Network | Chain ID | Description | URL |
|---------|----------|-------------|-----|
| Primum | 0 | Local development | `ws://localhost:8545` |
| Testum | 1 | Public testnet | `wss://testnet.omne.org` |
| Principalis | 42 | Mainnet | `wss://mainnet.omne.org` |

## Runtime Snapshot Verification

The SDK ships with the golden runtime manifest generated from the `runtime-golden`
test harness. Use the bundled CLI to confirm that a compiled WASM module matches the
recorded snapshot:

```bash
# Inspect the available fixture slugs
npx omne-sdk-verify-runtime --list

# Verify a module against the canonical hash
npx omne-sdk-verify-runtime --artifact path/to/module.wasm --slug entry_constant_main
```

Pass `--json` to emit machine-readable output or `--manifest` to point at a custom
manifest file generated from a different commit.

### Custom Network

```typescript
const client = new OmneClient({
  url: 'wss://custom.omne.network',
  timeout: 30000,
  retries: 3,
  headers: {
    'Authorization': 'Bearer your-api-key'
  }
});
```

## Error Handling

```typescript
import { 
  NetworkError, 
  TransactionError, 
  ValidationError, 
  WalletError 
} from '@omne/sdk';

try {
  await client.sendTransaction(tx);
} catch (error) {
  if (error instanceof NetworkError) {
    console.log('Network issue:', error.message);
    console.log('Status code:', error.statusCode);
  } else if (error instanceof TransactionError) {
    console.log('Transaction failed:', error.message);
    console.log('TX hash:', error.txHash);
  } else if (error instanceof ValidationError) {
    console.log('Invalid input:', error.field, error.value);
  }
}
```

## TypeScript Support

The SDK is written in TypeScript and provides comprehensive type definitions:

```typescript
import type { 
  NetworkInfo,
  Transaction,
  TransactionReceipt,
  ORC20Token,
  ComputationalJob
} from '@omne/sdk';

// Fully typed API responses
const networkInfo: NetworkInfo = await client.getNetworkInfo();
const receipt: TransactionReceipt = await client.sendTransaction(tx);
const token: ORC20Token = await client.getTokenInfo(tokenAddress);
```

## Browser Usage

The SDK works in modern browsers with WebSocket support:

```html
<script type="module">
import { OmneClient, Wallet } from 'https://cdn.skypack.dev/@omne/sdk';

const client = new OmneClient('wss://testnet.omne.org');
const wallet = Wallet.generate();
// ... use SDK features
</script>
```

## Custom Platform Providers (React Native, Workers, etc.)

`@omne/sdk` automatically installs Node-friendly or browser-friendly providers depending on which bundle you import (`main/module` for Node, `browser` field for web bundlers). If you run inside a different runtime (React Native, Cloudflare Workers, Electron preload, etc.) you can provide your own implementations via `setPlatformProviders`.

```ts
import { setPlatformProviders, type PlatformProviders } from '@omne/sdk';
import fetch from 'cross-fetch';
import WebSocket from 'isomorphic-ws';
import { aesCtr } from './native-aes-ctr'; // your AES-CTR bridge

const providers: PlatformProviders = {
  fetch: {
    async getFetch() {
      return fetch as typeof globalThis.fetch;
    }
  },
  webSocket: {
    connect(url, handlers) {
      const socket = new WebSocket(url);
      socket.onopen = () => handlers.onOpen();
      socket.onerror = (event: any) => handlers.onError(event);
      socket.onclose = () => handlers.onClose();
      socket.onmessage = (event: any) => handlers.onMessage(event.data?.toString?.() ?? '');
      return {
        send(data) {
          socket.send(data);
        },
        close() {
          socket.close();
        }
      };
    }
  },
  crypto: {
    async aesCtrEncrypt(keyBytes, ivBytes, plaintext) {
      return aesCtr('encrypt', keyBytes, ivBytes, plaintext);
    },
    async aesCtrDecrypt(keyBytes, ivBytes, ciphertext) {
      return aesCtr('decrypt', keyBytes, ivBytes, ciphertext);
    }
  },
  env: {
    isBrowser() {
      return false; // React Native runtime
    },
    isNode() {
      return false;
    }
  }
};

setPlatformProviders(providers);
```

Call `setPlatformProviders()` once, as early as possible (before creating `OmneClient`). You can mix and match—e.g., reuse the built-in browser fetch provider but override just the WebSocket layer when embedding inside Electron.

## Examples

Check the `examples/` directory for complete usage examples:

- `basic-usage.ts` - Wallet creation, transactions, quar calculations
- `orc20-tokens.ts` - Token deployment and management
- `computational-jobs.ts` - Submit work to OON
- `event-monitoring.ts` - Real-time blockchain events

## API Reference

### Classes

- **OmneClient** - Main blockchain client
- **Wallet** - BIP39 HD wallet implementation
- **WalletAccount** - Individual account with signing
- **WalletManager** - Multi-wallet management

### Functions

- **createClient(network)** - Create client with network defaults
- **toQuar(omc)** - Convert OMC to quar precision
- **fromQuar(quar)** - Convert quar to OMC
- **formatBalance(quar, decimals?)** - Format for display
- **isValidAddress(address)** - Validate address format

## Development

### Building

```bash
npm run build
```

### Testing

```bash
npm test
```

### Type Checking

```bash
npm run type-check
```

## License

MIT - see LICENSE file for details

## Support

- Documentation: https://docs.omne.org/sdk/typescript
- Discord: https://discord.gg/omne
- GitHub Issues: https://github.com/OmneDAO/omne-blockchain/issues
