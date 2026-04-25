//! Closures 深度实践：从 HFT / Web3 生产场景抽象通用策略。
//!
//! 运行：
//!   cargo run -p closures

use std::collections::{HashMap, HashSet};
use std::thread;
use std::time::Duration;

fn main() {
    println!("=== Closures 深度实践：生产问题 -> 代码案例 -> 通用策略 ===\n");

    closure_basics();
    hft_pre_trade_risk_gate();
    hft_venue_router();
    hft_micro_batch_aggregator();
    web3_event_filter();
    web3_nonce_allocator();
    web3_retry_policy();
    general_patterns();
}

fn section(title: &str) {
    println!("\n--- {title} ---");
}

fn closure_basics() {
    section("1. 闭包基础：Fn / FnMut / FnOnce");

    let fee_rate = 0.0002;
    let estimate_fee = |notional: f64| notional * fee_rate;
    println!("Fn: immutable capture, fee = {:.4}", estimate_fee(25_000.0));

    let mut accepted = 0;
    let mut count_accept = |passed: bool| {
        if passed {
            accepted += 1;
        }
        accepted
    };
    println!("FnMut: accepted orders = {}", count_accept(true));
    println!("FnMut: accepted orders = {}", count_accept(false));

    let incident_report = String::from("risk limit breached");
    let consume_report = move || format!("FnOnce: archived report '{incident_report}'");
    println!("{}", consume_report());
}

#[derive(Debug, Clone)]
struct Order {
    symbol: &'static str,
    side: Side,
    qty: u64,
    price: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    Buy,
    Sell,
}

impl Order {
    fn notional(&self) -> f64 {
        self.qty as f64 * self.price
    }
}

fn hft_pre_trade_risk_gate() {
    section("2. HFT 场景：pre-trade risk gate");

    let max_single_order_notional = 100_000.0;
    let max_session_notional = 250_000.0;
    let allowed_symbols = HashSet::from(["BTC-USD", "ETH-USD"]);

    let mut session_notional = 0.0;
    let mut risk_gate = |order: &Order| -> Result<(), String> {
        if !allowed_symbols.contains(order.symbol) {
            return Err(format!("reject {}: symbol not allowed", order.symbol));
        }

        let notional = order.notional();
        if notional > max_single_order_notional {
            return Err(format!(
                "reject {}: single order notional {:.2} > {:.2}",
                order.symbol, notional, max_single_order_notional
            ));
        }

        if session_notional + notional > max_session_notional {
            return Err(format!(
                "reject {}: session notional {:.2} would exceed {:.2}",
                order.symbol,
                session_notional + notional,
                max_session_notional
            ));
        }

        session_notional += notional;
        Ok(())
    };

    let orders = vec![
        Order {
            symbol: "BTC-USD",
            side: Side::Buy,
            qty: 2,
            price: 30_000.0,
        },
        Order {
            symbol: "DOGE-USD",
            side: Side::Buy,
            qty: 20_000,
            price: 0.12,
        },
        Order {
            symbol: "ETH-USD",
            side: Side::Sell,
            qty: 70,
            price: 2_100.0,
        },
    ];

    for order in &orders {
        match risk_gate(order) {
            Ok(()) => println!("accepted: {order:?}"),
            Err(reason) => println!("{reason}"),
        }
    }

    println!("production mapping: 闭包捕获限额配置和会话状态，形成低延迟 inline risk check");
}

#[derive(Debug, Clone)]
struct VenueQuote {
    venue: &'static str,
    bid: f64,
    ask: f64,
    latency_us: u64,
    fee_bps: f64,
}

fn hft_venue_router() {
    section("3. HFT 场景：venue router / smart order routing");

    let quotes = vec![
        VenueQuote {
            venue: "CEX-A",
            bid: 30_002.0,
            ask: 30_004.0,
            latency_us: 180,
            fee_bps: 0.8,
        },
        VenueQuote {
            venue: "CEX-B",
            bid: 30_000.0,
            ask: 30_003.0,
            latency_us: 70,
            fee_bps: 1.2,
        },
        VenueQuote {
            venue: "CEX-C",
            bid: 30_006.0,
            ask: 30_009.0,
            latency_us: 240,
            fee_bps: 0.6,
        },
    ];

    let latency_penalty_per_us = 0.003;
    let fee_penalty_multiplier = 0.25;
    let score = |quote: &VenueQuote, side: Side| -> f64 {
        let raw_price = match side {
            Side::Buy => -quote.ask,
            Side::Sell => quote.bid,
        };

        raw_price
            - quote.latency_us as f64 * latency_penalty_per_us
            - quote.fee_bps * fee_penalty_multiplier
    };

    let choose_venue = |side: Side| {
        quotes
            .iter()
            .max_by(|left, right| score(left, side).total_cmp(&score(right, side)))
            .expect("quotes should not be empty")
    };

    let buy_venue = choose_venue(Side::Buy);
    let sell_venue = choose_venue(Side::Sell);
    println!(
        "best buy route:  {} score {:.4}",
        buy_venue.venue,
        score(buy_venue, Side::Buy)
    );
    println!(
        "best sell route: {} score {:.4}",
        sell_venue.venue,
        score(sell_venue, Side::Sell)
    );
    println!("production mapping: 闭包把路由评分函数参数化，避免为每个策略复制排序逻辑");
}

fn hft_micro_batch_aggregator() {
    section("4. HFT 场景：micro-batch aggregation");

    let mut aggregate_by_symbol = {
        let mut exposure: HashMap<&'static str, i64> = HashMap::new();

        move |order: &Order| {
            let signed_qty = match order.side {
                Side::Buy => order.qty as i64,
                Side::Sell => -(order.qty as i64),
            };

            *exposure.entry(order.symbol).or_default() += signed_qty;
            exposure.clone()
        }
    };

    let stream = vec![
        Order {
            symbol: "BTC-USD",
            side: Side::Buy,
            qty: 3,
            price: 30_000.0,
        },
        Order {
            symbol: "ETH-USD",
            side: Side::Sell,
            qty: 8,
            price: 2_100.0,
        },
        Order {
            symbol: "BTC-USD",
            side: Side::Sell,
            qty: 1,
            price: 30_010.0,
        },
    ];

    for order in &stream {
        println!(
            "after {:?}: exposure = {:?}",
            order.side,
            aggregate_by_symbol(order)
        );
    }

    println!("production mapping: FnMut 闭包适合封装滚动窗口、仓位、节流计数等局部状态");
}

#[derive(Debug)]
struct ChainEvent {
    chain_id: u64,
    contract: &'static str,
    topic: &'static str,
    block_number: u64,
    confirmations: u64,
}

fn web3_event_filter() {
    section("5. Web3 场景：indexer event filter");

    let supported_chains = HashSet::from([1, 10, 42161]);
    let watched_contracts = HashSet::from(["0xRouter", "0xPool"]);
    let min_confirmations = 12;
    let min_block_number = 1;

    let is_finalized_swap = |event: &&ChainEvent| {
        supported_chains.contains(&event.chain_id)
            && watched_contracts.contains(event.contract)
            && event.topic == "Swap"
            && event.block_number >= min_block_number
            && event.confirmations >= min_confirmations
    };

    let events = vec![
        ChainEvent {
            chain_id: 1,
            contract: "0xRouter",
            topic: "Swap",
            block_number: 19_000_001,
            confirmations: 13,
        },
        ChainEvent {
            chain_id: 1,
            contract: "0xRouter",
            topic: "Transfer",
            block_number: 19_000_002,
            confirmations: 20,
        },
        ChainEvent {
            chain_id: 42161,
            contract: "0xPool",
            topic: "Swap",
            block_number: 140_000_001,
            confirmations: 9,
        },
    ];

    let finalized_swaps: Vec<_> = events.iter().filter(is_finalized_swap).collect();
    println!("finalized swaps: {finalized_swaps:?}");
    println!(
        "production mapping: filter/map 闭包把链、合约、确认数等索引规则组合成声明式 pipeline"
    );
}

fn web3_nonce_allocator() {
    section("6. Web3 场景：nonce allocator");

    let mut allocate_nonce = {
        let mut next_nonce = 42_u64;

        move || {
            let nonce = next_nonce;
            next_nonce += 1;
            nonce
        }
    };

    let build_tx = |to: &str, value_wei: u128, nonce: u64| {
        format!("tx(to={to}, value={value_wei}, nonce={nonce})")
    };

    let tx1 = build_tx("0xMarketMaker", 1_000_000_000_000_000, allocate_nonce());
    let tx2 = build_tx("0xSettlement", 2_500_000_000_000_000, allocate_nonce());
    println!("{tx1}");
    println!("{tx2}");
    println!(
        "production mapping: move + FnMut 闭包可把外部 nonce 状态收进局部 allocator，减少重复传参"
    );
}

fn web3_retry_policy() {
    section("7. Web3 场景：RPC retry / backoff policy");

    let retryable = |error: &&str| error.contains("timeout") || error.contains("rate limited");
    let backoff = |attempt: u32| Duration::from_millis(40 * 2_u64.pow(attempt));

    let mut failures = vec!["timeout", "rate limited"];
    let result = retry(
        4,
        || {
            if let Some(error) = failures.pop() {
                Err(error)
            } else {
                Ok("block 19000003")
            }
        },
        retryable,
        backoff,
    );

    println!("rpc result: {result:?}");
    println!("production mapping: 高阶函数接收闭包，让重试、超时、熔断策略独立于具体 RPC 调用");
}

fn retry<T, E, Op, ShouldRetry, Backoff>(
    max_attempts: u32,
    mut operation: Op,
    should_retry: ShouldRetry,
    backoff: Backoff,
) -> Result<T, E>
where
    Op: FnMut() -> Result<T, E>,
    ShouldRetry: Fn(&E) -> bool,
    Backoff: Fn(u32) -> Duration,
{
    for attempt in 0..max_attempts {
        match operation() {
            Ok(value) => return Ok(value),
            Err(error) if attempt + 1 < max_attempts && should_retry(&error) => {
                let delay = backoff(attempt);
                println!("retry attempt {}, sleeping {:?}", attempt + 1, delay);
                thread::sleep(delay);
            }
            Err(error) => return Err(error),
        }
    }

    unreachable!("loop always returns because max_attempts is checked on each attempt")
}

fn general_patterns() {
    section("8. 泛化：一般性问题和应对策略");

    let patterns = [
        (
            "策略参数化",
            "把价格评分、风控阈值、过滤规则写成 Fn 闭包；调用方只关心何时执行。",
        ),
        (
            "局部状态封装",
            "用 FnMut 捕获滚动计数、nonce、窗口聚合状态；避免把临时状态扩散到全局对象。",
        ),
        (
            "资源所有权转移",
            "需要跨线程、延迟执行或任务队列时使用 move；明确闭包持有什么、生命周期到哪里。",
        ),
        (
            "热路径性能",
            "优先泛型参数 F: Fn/FnMut，让编译器单态化和内联；只有异构回调集合才使用 Box<dyn Fn>。",
        ),
        (
            "错误和重试策略",
            "把 should_retry/backoff/on_error 抽成闭包，使业务调用和稳定性策略解耦。",
        ),
        (
            "可测试性",
            "生产代码接收闭包或 trait；测试时注入确定性的时钟、RPC、价格源、随机数。",
        ),
    ];

    for (problem, strategy) in patterns {
        println!("{problem}: {strategy}");
    }
}
