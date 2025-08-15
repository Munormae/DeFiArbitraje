# Pool Discovery CLI (Rust + ethers-rs)

Сканер пулов для сетей/DEX из `defi_config.json`. Находит:
- v2/solidly пары: адреса пар, `getReserves`, `token0/token1`;
- v3 пулы: адреса, `slot0` (sqrtPriceX96, tick) и `liquidity` для заданных fee tiers.

Выводит `pools.generated.json` с резервыми/ликвидностью и эвристически рассчитанным `suggested_amount_*` (20 бп от min(reserve)).
Для v3 резервов нет — оставляем `note` и публикуем `liquidity/slot0`.

## Сборка и запуск
```bash
cd pool-discovery-cli
cargo build
RUST_LOG=info ./target/debug/pool-discovery-cli --config /mnt/data/defi_config.json --out /mnt/data/pools.generated.json
```

Флаги:
- `--concurrency` — уровень параллелизма RPC (по умолчанию 32).
