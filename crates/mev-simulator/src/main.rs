//! MEV (Maximal Extractable Value) Bundle Simulator
//!
//! 展示 Rust 在 Web3 MEV 搜索中的经典特性组合：
//! - 并行交易执行 (rayon)
//! - 所有权系统 (状态转移)
//! - 借用检查器 (防止数据竞争)
//! - Result 类型 (错误处理)
//! - Clone trait (状态快照)
//!
//! 适用场景：以太坊 MEV 搜索、套利检测、三明治攻击模拟

use rayon::prelude::*;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

/// 账户余额 - 使用 Copy 语义的小对象
#[derive(Debug, Clone, Copy, PartialEq)]
struct Balance {
    eth: u64,
    token: u64,
}

/// 交易类型 - 枚举模式匹配
#[derive(Debug, Clone)]
enum Transaction {
    Transfer {
        from: u64,
        to: u64,
        amount: u64,
    },
    Swap {
        token_in: u64,
        #[allow(dead_code)]
        token_out: u64,
        amount_in: u64,
    },
    Arbitrage {
        path: Vec<u64>,
        min_profit: u64,
    },
}

/// 模拟的世界状态
#[derive(Debug)]
struct WorldState {
    /// 账户余额 - 使用 HashMap 支持动态账户
    balances: HashMap<u64, Balance>,
    /// 当前区块号 - AtomicU64 用于无锁并发
    block_number: AtomicU64,
    /// Gas 价格
    gas_price: u64,
}

impl Clone for WorldState {
    fn clone(&self) -> Self {
        Self {
            balances: self.balances.clone(),
            block_number: AtomicU64::new(self.block_number.load(Ordering::Relaxed)),
            gas_price: self.gas_price,
        }
    }
}

impl WorldState {
    /// 创建新状态 - 构造器模式
    fn new(gas_price: u64) -> Self {
        let mut balances = HashMap::new();
        // 初始化一些账户
        balances.insert(
            1,
            Balance {
                eth: 1000,
                token: 500,
            },
        );
        balances.insert(
            2,
            Balance {
                eth: 1000,
                token: 0,
            },
        );
        balances.insert(
            3,
            Balance {
                eth: 1000,
                token: 1000,
            },
        );

        Self {
            balances,
            block_number: AtomicU64::new(1),
            gas_price,
        }
    }

    /// 执行单个交易 - 返回 Result 进行错误处理
    fn execute_transaction(&mut self, tx: &Transaction) -> Result<u64, String> {
        match tx {
            Transaction::Transfer { from, to, amount } => {
                // 借用检查器确保我们不会同时使用可变和不可变引用
                let from_balance = self.balances.get(from).ok_or("Sender not found")?;
                if from_balance.eth < *amount {
                    return Err("Insufficient balance".to_string());
                }

                // 使用 get_mut 获取可变引用
                let from_bal = self.balances.get_mut(from).unwrap();
                from_bal.eth -= amount;

                self.balances
                    .entry(*to)
                    .or_insert(Balance { eth: 0, token: 0 })
                    .eth += amount;
                Ok(amount * self.gas_price / 100)
            }

            Transaction::Swap {
                token_in,
                token_out: _,
                amount_in,
            } => {
                // 简化的 swap 逻辑
                let account = self.balances.get_mut(token_in).ok_or("Account not found")?;
                if account.token < *amount_in {
                    return Err("Insufficient token balance".to_string());
                }

                account.token -= amount_in;
                account.eth += amount_in * 99 / 100; // 1% fee

                Ok(amount_in * self.gas_price / 50)
            }

            Transaction::Arbitrage { path, min_profit } => {
                // 模拟套利路径
                let mut current_account = *path.first().ok_or("Empty path")?;
                let mut profit = 0i64;

                for &next in path.iter().skip(1) {
                    // 这里简化了套利逻辑
                    let balance = self
                        .balances
                        .get(&current_account)
                        .copied()
                        .ok_or("Account not found")?;

                    profit += balance.eth as i64;
                    current_account = next;
                }

                if profit < 0 || (profit as u64) < *min_profit {
                    return Err("Arbitrage not profitable".to_string());
                }

                // 执行套利
                let first_account = self.balances.get_mut(&path[0]).unwrap();
                first_account.eth += profit as u64;

                Ok(path.len() as u64 * self.gas_price / 10)
            }
        }
    }
}

/// MEV Bundle - 一组必须原子执行的交易
#[derive(Debug, Clone)]
struct Bundle {
    transactions: Vec<Transaction>,
    revert_on_fail: bool,
    min_profit: u64,
}

/// Bundle 模拟结果
#[derive(Debug, Clone)]
struct SimulationResult {
    bundle_index: usize,
    success: bool,
    profit: i64,
    gas_used: u64,
    #[allow(dead_code)]
    final_state: WorldState,
}

/// MEV 搜索器 - 使用并行处理模拟多个 bundle
struct MEVSearcher {
    state: WorldState,
}

impl MEVSearcher {
    fn new(state: WorldState) -> Self {
        Self { state }
    }

    /// 并行模拟多个 bundles - 这是 Rust + rayon 的经典应用
    /// 每个 bundle 在独立的状态副本上执行，避免数据竞争
    fn simulate_bundles_parallel(&self, bundles: &[Bundle]) -> Vec<SimulationResult> {
        // par_iter 使用工作窃取算法并行处理
        bundles
            .par_iter()
            .enumerate()
            .map(|(i, bundle)| {
                // Clone 状态以获得独立的副本
                // Rust 的所有权系统确保没有数据竞争
                let mut local_state = self.state.clone();

                let mut total_profit = 0i64;
                let mut total_gas = 0u64;
                let mut success = true;

                for tx in &bundle.transactions {
                    match local_state.execute_transaction(tx) {
                        Ok(gas) => total_gas += gas,
                        Err(_e) => {
                            if bundle.revert_on_fail {
                                success = false;
                                break;
                            }
                        }
                    }
                }

                // 计算利润（简化）
                for balance in local_state.balances.values() {
                    total_profit += balance.eth as i64;
                }
                for balance in self.state.balances.values() {
                    total_profit -= balance.eth as i64;
                }

                SimulationResult {
                    bundle_index: i,
                    success,
                    profit: total_profit,
                    gas_used: total_gas,
                    final_state: local_state,
                }
            })
            .collect()
    }

    /// 找到最有利可图的 bundle
    fn find_best_bundle<'a>(&self, bundles: &'a [Bundle]) -> Option<&'a Bundle> {
        let results = self.simulate_bundles_parallel(bundles);

        results
            .into_iter()
            .filter(|r| r.success && r.profit > 0)
            .max_by_key(|r| r.profit)
            .map(|r| &bundles[r.bundle_index])
    }
}

fn main() {
    println!("=== MEV Bundle Simulator ===\n");

    // 初始化世界状态
    let state = WorldState::new(20); // gas_price = 20 gwei
    println!("Initial state: {:#?}\n", state);

    // 创建 MEV 搜索器
    let searcher = MEVSearcher::new(state);

    // 创建多个 bundles 进行模拟
    let bundles = vec![
        Bundle {
            transactions: vec![
                Transaction::Transfer {
                    from: 1,
                    to: 2,
                    amount: 100,
                },
                Transaction::Swap {
                    token_in: 2,
                    token_out: 1,
                    amount_in: 50,
                },
            ],
            revert_on_fail: true,
            min_profit: 10,
        },
        Bundle {
            transactions: vec![Transaction::Arbitrage {
                path: vec![1, 2, 3, 1],
                min_profit: 50,
            }],
            revert_on_fail: true,
            min_profit: 50,
        },
        Bundle {
            transactions: vec![
                Transaction::Transfer {
                    from: 2,
                    to: 3,
                    amount: 200,
                },
                Transaction::Swap {
                    token_in: 3,
                    token_out: 2,
                    amount_in: 100,
                },
            ],
            revert_on_fail: false,
            min_profit: 5,
        },
        Bundle {
            transactions: vec![
                Transaction::Transfer {
                    from: 3,
                    to: 1,
                    amount: 500,
                },
                Transaction::Swap {
                    token_in: 1,
                    token_out: 3,
                    amount_in: 250,
                },
            ],
            revert_on_fail: true,
            min_profit: 100,
        },
    ];

    println!("Simulating {} bundles in parallel...\n", bundles.len());

    // 并行模拟所有 bundles
    let results = searcher.simulate_bundles_parallel(&bundles);

    println!("=== Simulation Results ===");
    for result in &results {
        println!(
            "Bundle {}: {} | Profit: {} | Gas: {}",
            result.bundle_index,
            if result.success {
                "✓ SUCCESS"
            } else {
                "✗ FAILED"
            },
            result.profit,
            result.gas_used
        );
    }

    // 找到最佳 bundle
    println!("\n=== Best Bundle ===");
    if let Some(best) = searcher.find_best_bundle(&bundles) {
        println!(
            "Found profitable bundle with {} transactions",
            best.transactions.len()
        );
        println!("Min profit requirement: {}", best.min_profit);
    } else {
        println!("No profitable bundle found");
    }

    // 性能对比：串行 vs 并行
    println!("\n=== Performance Comparison ===");
    use std::time::Instant;

    let start = Instant::now();
    let _ = searcher.simulate_bundles_parallel(&bundles);
    let parallel_time = start.elapsed();

    let start = Instant::now();
    // 串行版本
    bundles
        .iter()
        .enumerate()
        .map(|(i, bundle)| {
            let mut local_state = searcher.state.clone();
            let mut success = true;
            for tx in &bundle.transactions {
                if local_state.execute_transaction(tx).is_err() {
                    success = false;
                    break;
                }
            }
            (i, success)
        })
        .count();
    let serial_time = start.elapsed();

    println!("Parallel time: {:?}", parallel_time);
    println!("Serial time:   {:?}", serial_time);
    println!(
        "Speedup: {:.2}x",
        serial_time.as_nanos() as f64 / parallel_time.as_nanos() as f64
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_transfer_success() {
        let mut state = WorldState::new(20);
        let tx = Transaction::Transfer {
            from: 1,
            to: 2,
            amount: 100,
        };

        assert!(state.execute_transaction(&tx).is_ok());
        assert_eq!(state.balances[&1].eth, 900);
        assert_eq!(state.balances[&2].eth, 1100);
    }

    #[test]
    fn test_transfer_insufficient() {
        let mut state = WorldState::new(20);
        let tx = Transaction::Transfer {
            from: 1,
            to: 2,
            amount: 10000,
        };

        assert!(state.execute_transaction(&tx).is_err());
    }

    #[test]
    fn test_parallel_simulation() {
        let state = WorldState::new(20);
        let searcher = MEVSearcher::new(state);
        let bundles = vec![Bundle {
            transactions: vec![Transaction::Transfer {
                from: 1,
                to: 2,
                amount: 100,
            }],
            revert_on_fail: true,
            min_profit: 0,
        }];

        let results = searcher.simulate_bundles_parallel(&bundles);
        assert_eq!(results.len(), 1);
        assert!(results[0].success);
    }
}
