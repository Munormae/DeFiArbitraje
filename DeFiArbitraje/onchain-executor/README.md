# On-chain Executor (Solidity 0.8.x)

- Поддержка: UniswapV2, UniswapV3, Solidly (Velo/Aerodrome).
- Permit2: `permitTransferFrom` для подтягивания входных токенов от EOA.
- `minOut`/`deadline` на каждый шаг, финальная проверка профита по токену последнего хопа.
- `simulate()` через V2 Router.getAmountsOut / V3 QuoterV2 / Solidly Pair.getAmountOut.
- Безопасность: SafeTransferLib, ReentrancyGuard, аварийный `sweep()`.

## Foundry
```bash
forge build
PRIVATE_KEY=0xabc... PERMIT2_ADDRESS=0x000000000022D473030F116dDEE9F6B43aC78BA3 \
forge script script/Deploy.s.sol --broadcast --rpc-url <RPC>
```

## Hardhat
```bash
cd hardhat
npm i
npx hardhat compile
PERMIT2_ADDRESS=0x000000000022D473030F116dDEE9F6B43aC78BA3 \
npx hardhat run scripts/deploy.ts --network <yournet>
```
Компиляция положит полный ABI+bytecode в `artifacts/contracts/Executor.sol/Executor.json`.
