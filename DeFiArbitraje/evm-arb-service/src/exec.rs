use anyhow::{anyhow, Context, Result};
use ethers::abi::Abi;
use ethers::prelude::*;
use std::sync::Arc;
use itertools::Itertools;
use tracing::{info, warn};

// джиттер/MEV утилиты
use crate::mev::{jitter_u64_bps, jitter_value_bps, GasJitterCfg, PrivateRelay};

/// Экзекьютор маршрутов (контракт с методами simulate/execute)
pub struct Executor<P, S>
where
    P: Middleware + 'static,
    S: Signer + 'static,
{
    pub client: Arc<SignerMiddleware<P, S>>,
    pub address: Address,
    pub abi: Abi,
}

impl<P, S> Executor<P, S>
where
    P: Middleware + 'static,
    S: Signer + 'static,
{
    /// address берём из ENV: EXECUTOR_<chainId>
    pub async fn new(client: Arc<SignerMiddleware<P, S>>) -> Result<Self> {
        let chain_id = client.provider().get_chainid().await?.as_u64();
        let key = format!("EXECUTOR_{}", chain_id);
        let addr_s = std::env::var(&key)
            .with_context(|| format!("укажите адрес экзекутора в ENV: {key}"))?;
        let address: Address = addr_s.parse().context("invalid executor address")?;

        // грузим ABI один раз
        let abi: Abi = serde_json::from_str(include_str!("../abis/Executor.json"))
            .context("bad Executor ABI json")?;

        // sanity: метод execute(bytes,uint256) должен существовать
        if abi.function("execute").is_err() {
            return Err(anyhow!("Executor ABI: method 'execute' not found"));
        }

        Ok(Self { client, address, abi })
    }

    /// Статическая симуляция: simulate(bytes) -> uint256 (profit)
    pub async fn simulate(&self, route_calldata: Bytes) -> Result<U256> {
        let c = Contract::new(self.address, self.abi.clone(), self.client.clone());

        // небольшой лимит газа, чтобы eth_call не падал
        let out: U256 = c
            .method::<_, U256>("simulate", route_calldata)?
            .gas(200_000u64)
            .call()
            .await
            .context("simulate() call failed")?;

        Ok(out)
    }

    /// Быстрый путь (без специальных опций)
    pub async fn execute(&self, route_calldata: Bytes, min_profit: U256) -> Result<TxHash> {
        let opts = TxOpts::default();
        self.execute_with_opts(route_calldata, min_profit, opts).await
    }
}

/// Опции исполнения
#[derive(Clone, Debug, Default)]
pub struct TxOpts {
    /// Отправлять приватно (используется вместе с `private_relay`)
    pub private: bool,

    /// Параметры джиттера
    pub gas_jitter: Option<GasJitterCfg>,

    /// Желаемый лимит газа (если None — дефолт 1_500_000)
    pub gas_limit: Option<u64>,

    /// EIP-1559 поля: мы сконвертим в эквивалентную legacy gasPrice
    pub max_fee_per_gas: Option<U256>,
    pub max_priority_fee_per_gas: Option<U256>,

    /// Legacy режим: фиксированная gas_price (если EIP-1559 не заданы)
    pub legacy_gas_price: Option<U256>,

    /// Опциональный приватный релей (Flashbots/bloxroute/…)
    pub private_relay: Option<PrivateRelay>,
}

impl<P, S> Executor<P, S>
where
    P: Middleware + 'static,
    S: Signer + 'static,
{
    /// Гибкое исполнение с джиттером и pseudo-EIP1559 (через legacy gasPrice)
    pub async fn execute_with_opts(
        &self,
        route_calldata: Bytes,
        min_profit: U256,
        opts: TxOpts,
    ) -> Result<TxHash> {
        // --- префлайт: сеть/nonce/basefee (диагностика)
        let chain_id = self.client.provider().get_chainid().await?.as_u64();
        let me = self.client.address();
        let nonce = self.client.get_transaction_count(me, Some(BlockId::Number(BlockNumber::Latest))).await?;
        let basefee = self
            .client
            .get_block(BlockNumber::Latest)
            .await?
            .and_then(|b| b.base_fee_per_gas)
            .unwrap_or_default();

        info!("execute: chain_id={chain_id} addr={me:?} nonce={nonce} basefee={basefee}");

        // --- конструктор контракта
        let c = Contract::new(self.address, self.abi.clone(), self.client.clone());
        let mut call = c
            // NB: если в контракте execute возвращает int256, здесь I256; если uint256 — поменяй на U256
            .method::<_, I256>("execute", (route_calldata, min_profit))
            .context("encode execute(route,min_profit)")?;

        // --- газ лимит + джиттер
        let base_gas = opts.gas_limit.unwrap_or(1_500_000u64);
        let gas_limit = if let Some(cfg) = &opts.gas_jitter {
            jitter_u64_bps(base_gas, cfg.jitter_bps)
        } else {
            base_gas
        };
        call = call.gas(gas_limit);

        // --- вычисляем эффективный legacy gasPrice
        let mut effective_gas_price: Option<U256> = None;

        if let (Some(max_fee), Some(tip)) = (opts.max_fee_per_gas, opts.max_priority_fee_per_gas)
        {
            let sum = basefee.saturating_add(tip);
            let legacy_eq = if sum > max_fee { max_fee } else { sum };
            effective_gas_price = Some(legacy_eq);
        }

        if effective_gas_price.is_none() {
            if let Some(gp) = opts.legacy_gas_price {
                effective_gas_price = Some(gp);
            }
        }

        if let Some(mut gp) = effective_gas_price {
            if let Some(cfg) = &opts.gas_jitter {
                gp = jitter_value_bps(gp, cfg.jitter_bps);
            }
            call = call.gas_price(gp); // legacy price
        } else {
            warn!("execute: using provider's default gas pricing (no EIP1559/legacy overrides)");
        }

        // --- приватная отправка (заглушка; для реального — нужен raw tx)
        if opts.private {
            if let Some(relay) = &opts.private_relay {
                let _ = relay.send_raw_tx("0x").await; // no-op
            }
        }

        // --- отправляем
        let pending = call.send().await.context("execute() send failed")?;
        let tx = pending.tx_hash();
        info!("execute sent: tx={:?} gas_limit={}", tx, gas_limit);
        Ok(tx)
    }
}
