use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex, Weak,
};
use std::thread;
use std::time::Duration;

// ---------- 工具：用于检测 Drop 是否被调用 ----------
struct DropFlag {
    dropped: Arc<AtomicBool>,
    name: &'static str,
}

impl DropFlag {
    fn new(name: &'static str) -> (Self, Arc<AtomicBool>) {
        let flag = Arc::new(AtomicBool::new(false));
        (
            DropFlag {
                dropped: Arc::clone(&flag),
                name,
            },
            flag,
        )
    }
}

impl Drop for DropFlag {
    fn drop(&mut self) {
        self.dropped.store(true, Ordering::SeqCst);
        println!("✅ Dropped: {}", self.name);
    }
}

// ========== 场景 1: HFT 策略与行情源循环 ==========
mod hft {
    use super::*;

    // --- 问题代码：相互强引用 ---
    pub struct MarketDataFeed {
        pub strategies: Vec<Arc<Mutex<TradingStrategy>>>,
        _flag: DropFlag,
    }

    pub struct TradingStrategy {
        pub id: u64,
        pub feed: Arc<Mutex<MarketDataFeed>>,
        _flag: DropFlag,
    }

    impl MarketDataFeed {
        pub fn subscribe(&mut self, strategy: Arc<Mutex<TradingStrategy>>) {
            self.strategies.push(strategy);
        }
    }

    /// 返回外部引用和它们的 Drop 标志
    pub fn create_leak_scenario() -> (
        Arc<AtomicBool>, // feed dropped?
        Arc<AtomicBool>, // strategy dropped?
    ) {
        let (feed_flag, feed_dropped) = DropFlag::new("HFT Feed (leak)");
        let (strat_flag, strat_dropped) = DropFlag::new("HFT Strategy (leak)");

        let feed = Arc::new(Mutex::new(MarketDataFeed {
            strategies: Vec::new(),
            _flag: feed_flag,
        }));
        let strategy = Arc::new(Mutex::new(TradingStrategy {
            id: 1,
            feed: Arc::clone(&feed),
            _flag: strat_flag,
        }));

        feed.lock().unwrap().subscribe(Arc::clone(&strategy));

        // 丢弃外部引用，模拟离开作用域
        drop(feed);
        drop(strategy);

        (feed_dropped, strat_dropped)
    }

    // --- 修复代码：使用 Weak 打破循环 ---
    pub struct MarketDataFeedFixed {
        pub strategies: Vec<Weak<Mutex<TradingStrategyFixed>>>,
        _flag: DropFlag,
    }

    pub struct TradingStrategyFixed {
        pub id: u64,
        pub feed: Weak<Mutex<MarketDataFeedFixed>>,
        _flag: DropFlag,
    }

    impl MarketDataFeedFixed {
        pub fn subscribe(&mut self, strategy: &Arc<Mutex<TradingStrategyFixed>>) {
            self.strategies.push(Arc::downgrade(strategy));
        }

        pub fn broadcast(&mut self, data: &str) {
            self.strategies.retain(|weak| {
                if let Some(strategy) = weak.upgrade() {
                    println!(
                        "Pushing '{}' to strategy {}",
                        data,
                        strategy.lock().unwrap().id
                    );
                    true
                } else {
                    false // 自动清理已释放的策略
                }
            });
        }
    }

    pub fn create_fixed_scenario() -> (
        Arc<AtomicBool>, // feed dropped?
        Arc<AtomicBool>, // strategy dropped?
    ) {
        let (feed_flag, feed_dropped) = DropFlag::new("HFT Feed (fixed)");
        let (strat_flag, strat_dropped) = DropFlag::new("HFT Strategy (fixed)");

        let feed = Arc::new(Mutex::new(MarketDataFeedFixed {
            strategies: Vec::new(),
            _flag: feed_flag,
        }));
        let strategy = Arc::new(Mutex::new(TradingStrategyFixed {
            id: 2,
            feed: Arc::downgrade(&feed),
            _flag: strat_flag,
        }));

        feed.lock().unwrap().subscribe(&strategy);

        drop(feed);
        drop(strategy);

        (feed_dropped, strat_dropped)
    }
}

// ========== 场景 2: Web3 代币与流动性池循环 ==========
mod web3 {
    use super::*;

    // --- 问题代码：Token 与池子双向强引用 ---
    pub struct Token {
        pub symbol: String,
        pub pools: Vec<Arc<Mutex<LiquidityPool>>>,
        _flag: DropFlag,
    }

    pub struct LiquidityPool {
        pub token0: Arc<Mutex<Token>>,
        pub token1: Arc<Mutex<Token>>,
        _flag: DropFlag,
    }

    impl Token {
        pub fn add_pool(&mut self, pool: Arc<Mutex<LiquidityPool>>) {
            self.pools.push(pool);
        }
    }

    pub fn create_leak_scenario() -> (
        Arc<AtomicBool>, // token_a dropped?
        Arc<AtomicBool>, // token_b dropped?
        Arc<AtomicBool>, // pool dropped?
    ) {
        let (ta_flag, ta_dropped) = DropFlag::new("Token A (leak)");
        let (tb_flag, tb_dropped) = DropFlag::new("Token B (leak)");
        let (pool_flag, pool_dropped) = DropFlag::new("Pool (leak)");

        let token_a = Arc::new(Mutex::new(Token {
            symbol: "A".into(),
            pools: vec![],
            _flag: ta_flag,
        }));
        let token_b = Arc::new(Mutex::new(Token {
            symbol: "B".into(),
            pools: vec![],
            _flag: tb_flag,
        }));

        let pool = Arc::new(Mutex::new(LiquidityPool {
            token0: Arc::clone(&token_a),
            token1: Arc::clone(&token_b),
            _flag: pool_flag,
        }));

        token_a.lock().unwrap().add_pool(Arc::clone(&pool));
        token_b.lock().unwrap().add_pool(Arc::clone(&pool));

        drop(token_a);
        drop(token_b);
        drop(pool);

        (ta_dropped, tb_dropped, pool_dropped)
    }

    // --- 修复方案一：Token 使用 Weak 持有池子 ---
    pub struct TokenWeak {
        pub symbol: String,
        pub pools: Vec<Weak<Mutex<LiquidityPoolWeak>>>,
        _flag: DropFlag,
    }

    pub struct LiquidityPoolWeak {
        pub token0: Arc<Mutex<TokenWeak>>,
        pub token1: Arc<Mutex<TokenWeak>>,
        _flag: DropFlag,
    }

    impl TokenWeak {
        pub fn add_pool(&mut self, pool: &Arc<Mutex<LiquidityPoolWeak>>) {
            self.pools.push(Arc::downgrade(pool));
        }
    }

    pub fn create_weak_fix_scenario() -> (Arc<AtomicBool>, Arc<AtomicBool>, Arc<AtomicBool>) {
        let (ta_flag, ta_dropped) = DropFlag::new("Token A (weak)");
        let (tb_flag, tb_dropped) = DropFlag::new("Token B (weak)");
        let (pool_flag, pool_dropped) = DropFlag::new("Pool (weak)");

        let token_a = Arc::new(Mutex::new(TokenWeak {
            symbol: "A".into(),
            pools: vec![],
            _flag: ta_flag,
        }));
        let token_b = Arc::new(Mutex::new(TokenWeak {
            symbol: "B".into(),
            pools: vec![],
            _flag: tb_flag,
        }));

        let pool = Arc::new(Mutex::new(LiquidityPoolWeak {
            token0: Arc::clone(&token_a),
            token1: Arc::clone(&token_b),
            _flag: pool_flag,
        }));

        token_a.lock().unwrap().add_pool(&pool);
        token_b.lock().unwrap().add_pool(&pool);

        drop(token_a);
        drop(token_b);
        drop(pool);

        (ta_dropped, tb_dropped, pool_dropped)
    }

    // --- 修复方案二：用 ID 替代引用（零循环） ---
    pub type TokenId = String;

    pub struct TokenIdOnly {
        pub id: TokenId,
        _flag: DropFlag,
    }

    pub struct LiquidityPoolIdOnly {
        pub token0_id: TokenId,
        pub token1_id: TokenId,
        _flag: DropFlag,
    }

    pub struct Registry {
        pub tokens: std::collections::HashMap<TokenId, Arc<Mutex<TokenIdOnly>>>,
        pub pools: Vec<Arc<Mutex<LiquidityPoolIdOnly>>>,
    }

    pub fn create_id_fix_scenario() -> (
        Arc<AtomicBool>, // token_a dropped?
        Arc<AtomicBool>, // token_b dropped?
        Arc<AtomicBool>, // pool dropped?
    ) {
        let (ta_flag, ta_dropped) = DropFlag::new("Token A (id)");
        let (tb_flag, tb_dropped) = DropFlag::new("Token B (id)");
        let (pool_flag, pool_dropped) = DropFlag::new("Pool (id)");

        let mut registry = Registry {
            tokens: std::collections::HashMap::new(),
            pools: Vec::new(),
        };

        let token_a = Arc::new(Mutex::new(TokenIdOnly {
            id: "A".into(),
            _flag: ta_flag,
        }));
        let token_b = Arc::new(Mutex::new(TokenIdOnly {
            id: "B".into(),
            _flag: tb_flag,
        }));

        registry
            .tokens
            .insert(token_a.lock().unwrap().id.clone(), Arc::clone(&token_a));
        registry
            .tokens
            .insert(token_b.lock().unwrap().id.clone(), Arc::clone(&token_b));

        let pool = Arc::new(Mutex::new(LiquidityPoolIdOnly {
            token0_id: "A".into(),
            token1_id: "B".into(),
            _flag: pool_flag,
        }));
        registry.pools.push(Arc::clone(&pool));

        // 所有结构体之间没有相互引用，只有 Registry 拥有
        drop(registry);
        drop(token_a);
        drop(token_b);
        drop(pool);

        (ta_dropped, tb_dropped, pool_dropped)
    }
}

// ========== 场景 3: 观察者模式循环（额外补充场景） ==========
mod observer {
    use super::*;

    pub struct Subject {
        pub observers: Vec<Arc<Mutex<Observer>>>,
        _flag: DropFlag,
    }

    pub struct Observer {
        pub subject: Arc<Mutex<Subject>>,
        _flag: DropFlag,
    }

    impl Subject {
        pub fn attach(&mut self, observer: Arc<Mutex<Observer>>) {
            self.observers.push(observer);
        }
    }

    pub fn create_leak_scenario() -> (
        Arc<AtomicBool>, // subject dropped?
        Arc<AtomicBool>, // observer dropped?
    ) {
        let (subj_flag, subj_dropped) = DropFlag::new("Subject (leak)");
        let (obs_flag, obs_dropped) = DropFlag::new("Observer (leak)");

        let subject = Arc::new(Mutex::new(Subject {
            observers: vec![],
            _flag: subj_flag,
        }));
        let observer = Arc::new(Mutex::new(Observer {
            subject: Arc::clone(&subject),
            _flag: obs_flag,
        }));

        subject.lock().unwrap().attach(Arc::clone(&observer));

        drop(subject);
        drop(observer);

        (subj_dropped, obs_dropped)
    }

    // 修复：Subject 持有 Weak<Observer>
    pub struct SubjectFixed {
        pub observers: Vec<Weak<Mutex<ObserverFixed>>>,
        _flag: DropFlag,
    }

    pub struct ObserverFixed {
        pub subject: Weak<Mutex<SubjectFixed>>,
        _flag: DropFlag,
    }

    impl SubjectFixed {
        pub fn attach(&mut self, observer: &Arc<Mutex<ObserverFixed>>) {
            self.observers.push(Arc::downgrade(observer));
        }
    }

    pub fn create_fixed_scenario() -> (
        Arc<AtomicBool>, // subject dropped?
        Arc<AtomicBool>, // observer dropped?
    ) {
        let (subj_flag, subj_dropped) = DropFlag::new("Subject (fixed)");
        let (obs_flag, obs_dropped) = DropFlag::new("Observer (fixed)");

        let subject = Arc::new(Mutex::new(SubjectFixed {
            observers: vec![],
            _flag: subj_flag,
        }));
        let observer = Arc::new(Mutex::new(ObserverFixed {
            subject: Arc::downgrade(&subject),
            _flag: obs_flag,
        }));

        subject.lock().unwrap().attach(&observer);

        drop(subject);
        drop(observer);

        (subj_dropped, obs_dropped)
    }
}

// ========== 测试与演示 ==========
fn main() {
    println!("=== Rust Reference Cycle Demonstration ===\n");

    // 注意：泄漏场景中 Drop 不会发生，因此不会有打印
    println!("--- HFT Leak Scenario ---");
    {
        let (feed_dropped, strat_dropped) = hft::create_leak_scenario();
        // 由于泄漏，两个 drop 标志仍然为 false
        assert!(!feed_dropped.load(Ordering::SeqCst));
        assert!(!strat_dropped.load(Ordering::SeqCst));
        println!(
            "⚠️  Feed dropped: {}, Strategy dropped: {} (expected false)",
            feed_dropped.load(Ordering::SeqCst),
            strat_dropped.load(Ordering::SeqCst)
        );
    }

    println!("\n--- HFT Fixed Scenario ---");
    {
        let (feed_dropped, strat_dropped) = hft::create_fixed_scenario();
        assert!(feed_dropped.load(Ordering::SeqCst));
        assert!(strat_dropped.load(Ordering::SeqCst));
        println!(
            "   Feed dropped: {}, Strategy dropped: {} (expected true)",
            feed_dropped.load(Ordering::SeqCst),
            strat_dropped.load(Ordering::SeqCst)
        );
    }

    println!("\n--- Web3 Leak Scenario ---");
    {
        let (ta, tb, pool) = web3::create_leak_scenario();
        assert!(!ta.load(Ordering::SeqCst));
        assert!(!tb.load(Ordering::SeqCst));
        assert!(!pool.load(Ordering::SeqCst));
        println!(
            "⚠️  Token A: {}, Token B: {}, Pool: {} (expected false)",
            ta.load(Ordering::SeqCst),
            tb.load(Ordering::SeqCst),
            pool.load(Ordering::SeqCst)
        );
    }

    println!("\n--- Web3 Weak Fix ---");
    {
        let (ta, tb, pool) = web3::create_weak_fix_scenario();
        assert!(ta.load(Ordering::SeqCst));
        assert!(tb.load(Ordering::SeqCst));
        assert!(pool.load(Ordering::SeqCst));
        println!(
            "   Token A: {}, Token B: {}, Pool: {} (expected true)",
            ta.load(Ordering::SeqCst),
            tb.load(Ordering::SeqCst),
            pool.load(Ordering::SeqCst)
        );
    }

    println!("\n--- Web3 ID Fix ---");
    {
        let (ta, tb, pool) = web3::create_id_fix_scenario();
        assert!(ta.load(Ordering::SeqCst));
        assert!(tb.load(Ordering::SeqCst));
        assert!(pool.load(Ordering::SeqCst));
        println!(
            "   Token A: {}, Token B: {}, Pool: {} (expected true)",
            ta.load(Ordering::SeqCst),
            tb.load(Ordering::SeqCst),
            pool.load(Ordering::SeqCst)
        );
    }

    println!("\n--- Observer Leak Scenario ---");
    {
        let (subj, obs) = observer::create_leak_scenario();
        assert!(!subj.load(Ordering::SeqCst));
        assert!(!obs.load(Ordering::SeqCst));
        println!(
            "⚠️  Subject: {}, Observer: {} (expected false)",
            subj.load(Ordering::SeqCst),
            obs.load(Ordering::SeqCst)
        );
    }

    println!("\n--- Observer Fixed Scenario ---");
    {
        let (subj, obs) = observer::create_fixed_scenario();
        assert!(subj.load(Ordering::SeqCst));
        assert!(obs.load(Ordering::SeqCst));
        println!(
            "   Subject: {}, Observer: {} (expected true)",
            subj.load(Ordering::SeqCst),
            obs.load(Ordering::SeqCst)
        );
    }

    println!("\n🎉 All demos completed. Assertions passed (leaks confirmed, fixes verified).");
}

// 如果使用 cargo test，则运行以下测试
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hft_leak_does_not_drop() {
        let (feed, strat) = hft::create_leak_scenario();
        assert!(!feed.load(Ordering::SeqCst));
        assert!(!strat.load(Ordering::SeqCst));
    }

    #[test]
    fn hft_fixed_drops() {
        let (feed, strat) = hft::create_fixed_scenario();
        assert!(feed.load(Ordering::SeqCst));
        assert!(strat.load(Ordering::SeqCst));
    }

    #[test]
    fn web3_leak_does_not_drop() {
        let (ta, tb, pool) = web3::create_leak_scenario();
        assert!(!ta.load(Ordering::SeqCst));
        assert!(!tb.load(Ordering::SeqCst));
        assert!(!pool.load(Ordering::SeqCst));
    }

    #[test]
    fn web3_weak_fix_drops() {
        let (ta, tb, pool) = web3::create_weak_fix_scenario();
        assert!(ta.load(Ordering::SeqCst));
        assert!(tb.load(Ordering::SeqCst));
        assert!(pool.load(Ordering::SeqCst));
    }

    #[test]
    fn web3_id_fix_drops() {
        let (ta, tb, pool) = web3::create_id_fix_scenario();
        assert!(ta.load(Ordering::SeqCst));
        assert!(tb.load(Ordering::SeqCst));
        assert!(pool.load(Ordering::SeqCst));
    }

    #[test]
    fn observer_leak_does_not_drop() {
        let (subj, obs) = observer::create_leak_scenario();
        assert!(!subj.load(Ordering::SeqCst));
        assert!(!obs.load(Ordering::SeqCst));
    }

    #[test]
    fn observer_fixed_drops() {
        let (subj, obs) = observer::create_fixed_scenario();
        assert!(subj.load(Ordering::SeqCst));
        assert!(obs.load(Ordering::SeqCst));
    }
}
