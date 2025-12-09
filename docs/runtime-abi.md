# Runtime ABI for pysub Contracts

This document captures the first stable cut of the Application Binary Interface (ABI) that pysub-generated WebAssembly modules must satisfy in order to run under the Omne Axiom Runtime.

## Import Surface

The runtime exposes a deterministic subset of host functions. Modules may import from the following modules only:

| Module | Functions | Notes |
|--------|-----------|-------|
| `axiom_system` | `get_gas_remaining() -> i64`, `get_execution_time() -> i64`, `abort()` | Deterministic system signals. `abort` traps execution. Currently only the gas and execution time helpers are wired through the compiler. |
| `axiom_memory` | `deterministic_malloc(u32) -> u32`, `deterministic_free(u32)`, `deterministic_realloc(u32, u32) -> u32`, `memory_usage() -> u64` | Deterministic allocator bindings. The compiler currently wires `deterministic_malloc`, `deterministic_realloc`, and `memory_usage`; `deterministic_free` will land alongside statement support. |
| `axiom_float` | Deterministic `f64_*` helpers (`f64_add`, `f64_sub`, `f64_mul`, `f64_div`, `f64_sqrt`, `f64_abs`, `f64_neg`, `f64_min`, `f64_max`, `f64_floor`, `f64_ceil`, `f64_round`) | All float operations round identically across validators. |
| `env` | `abort()` | Compatibility alias for engines that emit `env::abort`. |

When pysub source code references one of the supported helpers, the compiler lowers the call directly to the corresponding import while preserving the exact module path. Additional helpers will come online as the language surface expands.

## Required Exports

Every pysub module must expose a deterministic entry point that the runtime can invoke. The compiler now guarantees two export conventions:

1. **Module entry** – A top-level function named `main` (no parameters, returns `u128`) is required. It is exported twice:
   - As `main` (legacy tooling compatibility)
   - As `axiom_entry_main` (stable runtime entry point)

2. **Contract methods** – Each contract function is exported twice:
   - As `<Contract>::<function>` (unchanged)
   - As `axiom_contract::<Contract>::<function>` (stable runtime namespace)

The runtime should exclusively invoke the `axiom_*` exports. Maintaining the legacy names allows existing tests and demonstrations to continue to work while the wider toolchain migrates.

## Value Mapping

| pysub type | WASM value | Notes |
|------------|------------|-------|
| `u128` | `i64` | Current lowering truncates to 64 bits until wide integer support lands. |
| `bool` / `address` / `bytes` | `i32` | Represented as raw handles/pointers by the runtime layer. |

Future ABI revisions can widen this table, but generated binaries must adhere to the mapping above.

## Example

```pysub
fn main() -> u128:
    return 0

contract Wallet(owner: address):
    fn balance() -> u128:
        return 0
```

The compiler emits a module with the following exported functions:

```
(main)                             ;; legacy
(axiom_entry_main)                 ;; runtime entry point
(Wallet::balance)                  ;; legacy contract export
(axiom_contract::Wallet::balance)  ;; runtime contract export
```

This ABI is the baseline that downstream components (runtime, CLI, deployment tooling) can target for end-to-end integration.
