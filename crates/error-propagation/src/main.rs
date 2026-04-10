//! Error Propagation 深度实践
//!
//! 考察点：
//! 1. `?` 如何做早返回，以及何时不该直接 `?`
//! 2. `From` / `map_err` 在多层调用中的职责划分
//! 3. 什么时候应该继续向上传播，什么时候应该降级兜底
//! 4. 业务错误、配置错误、基础设施错误是否要分层建模
//! 5. 批处理场景下，fail-fast 和 collect-all 的取舍
//! 6. `Result<Option<T>, E>` 在生产场景中的实际含义

use std::collections::HashMap;
use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::fs;
use std::io;
use std::num::ParseIntError;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

// =============================================================================
// 第一部分：启动阶段配置加载
// =============================================================================
//
// 题目：
// 一个 HTTP 服务启动时，需要读取监听端口。优先从环境变量 `APP_PORT`
// 读取；如果未设置，再从配置文件读取 `port=8080` 这一行。
//
// 希望候选人说明：
// 1. 为什么启动配置错误通常应该 fail-fast，而不是静默使用默认值？
// 2. 为什么需要在错误里补充 path / 原始值等上下文？
// 3. 这一层适合返回 String，还是结构化错误类型？

#[derive(Debug)]
enum BootstrapError {
    MissingPortConfig,
    ReadConfigFile { path: PathBuf, source: io::Error },
    InvalidPort { raw: String, source: ParseIntError },
}

impl Display for BootstrapError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingPortConfig => {
                write!(f, "missing `APP_PORT` and no `port=` entry in config file")
            }
            Self::ReadConfigFile { path, .. } => {
                write!(f, "failed to read config file `{}`", path.display())
            }
            Self::InvalidPort { raw, .. } => {
                write!(f, "invalid port `{raw}`")
            }
        }
    }
}

impl Error for BootstrapError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::ReadConfigFile { source, .. } => Some(source),
            Self::InvalidPort { source, .. } => Some(source),
            Self::MissingPortConfig => None,
        }
    }
}

fn parse_port(raw: &str) -> Result<u16, BootstrapError> {
    raw.trim()
        .parse::<u16>()
        .map_err(|source| BootstrapError::InvalidPort {
            raw: raw.trim().to_string(),
            source,
        })
}

fn read_port_from_file(path: &Path) -> Result<u16, BootstrapError> {
    let content = fs::read_to_string(path).map_err(|source| BootstrapError::ReadConfigFile {
        path: path.to_path_buf(),
        source,
    })?;

    let raw = content
        .lines()
        .find_map(|line| line.strip_prefix("port="))
        .ok_or(BootstrapError::MissingPortConfig)?;

    parse_port(raw)
}

fn load_http_port(
    env: &HashMap<String, String>,
    config_path: &Path,
) -> Result<u16, BootstrapError> {
    if let Some(raw) = env.get("APP_PORT") {
        return parse_port(raw);
    }

    read_port_from_file(config_path)
}

// =============================================================================
// 第二部分：服务内错误分层 + `From` 自动传播
// =============================================================================
//
// 题目：
// 电商服务实现退款接口：
// 1. 先校验请求参数
// 2. 再读取支付网关 API Key
// 3. 调用第三方网关发起退款
// 4. 最后写审计日志
//
// 希望候选人说明：
// 1. 哪些错误属于“调用方输错参数”，哪些属于“系统配置问题”，哪些属于“上游依赖异常”？
// 2. 审计日志失败时，是直接返回错误，还是记录告警后继续成功？
// 3. `?` 背后为什么要求错误类型可转换？

#[derive(Debug)]
struct RefundRequest {
    order_id: String,
    cents: u64,
}

#[derive(Debug)]
enum PaymentGatewayError {
    Timeout,
    Unauthorized,
    UpstreamRejected(String),
}

impl Display for PaymentGatewayError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Timeout => write!(f, "payment gateway timeout"),
            Self::Unauthorized => write!(f, "payment gateway rejected api key"),
            Self::UpstreamRejected(reason) => {
                write!(f, "payment gateway rejected request: {reason}")
            }
        }
    }
}

impl Error for PaymentGatewayError {}

#[derive(Debug)]
enum RefundError {
    Validation(String),
    MissingApiKey,
    Payment(PaymentGatewayError),
}

impl Display for RefundError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Validation(msg) => write!(f, "invalid refund request: {msg}"),
            Self::MissingApiKey => write!(f, "missing env `PAYMENT_API_KEY`"),
            Self::Payment(_) => write!(f, "refund failed because upstream payment failed"),
        }
    }
}

impl Error for RefundError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Payment(err) => Some(err),
            Self::Validation(_) | Self::MissingApiKey => None,
        }
    }
}

impl From<PaymentGatewayError> for RefundError {
    fn from(value: PaymentGatewayError) -> Self {
        Self::Payment(value)
    }
}

fn validate_refund_request(req: &RefundRequest) -> Result<(), RefundError> {
    if req.order_id.trim().is_empty() {
        return Err(RefundError::Validation("order_id must not be empty".into()));
    }
    if req.cents == 0 {
        return Err(RefundError::Validation(
            "refund amount must be greater than 0".into(),
        ));
    }
    Ok(())
}

fn call_payment_gateway(api_key: &str, req: &RefundRequest) -> Result<String, PaymentGatewayError> {
    if api_key != "live-secret-key" {
        return Err(PaymentGatewayError::Unauthorized);
    }
    if req.order_id == "timeout-order" {
        return Err(PaymentGatewayError::Timeout);
    }
    if req.order_id == "already-refunded" {
        return Err(PaymentGatewayError::UpstreamRejected(
            "refund already submitted".into(),
        ));
    }

    Ok(format!("refund-{}", req.order_id))
}

fn write_audit_log(refund_id: &str, should_fail: bool) -> io::Result<()> {
    if should_fail {
        return Err(io::Error::other(format!(
            "failed to write audit log for `{refund_id}`"
        )));
    }
    Ok(())
}

fn issue_refund(
    env: &HashMap<String, String>,
    req: &RefundRequest,
    audit_should_fail: bool,
) -> Result<String, RefundError> {
    validate_refund_request(req)?;

    let api_key = env
        .get("PAYMENT_API_KEY")
        .map(String::as_str)
        .ok_or(RefundError::MissingApiKey)?;

    let refund_id = call_payment_gateway(api_key, req)?;

    // 审计是合规要求里很常见的一环，但是否阻塞主流程取决于业务要求。
    // 这里模拟“主交易成功优先，审计失败告警后异步补偿”。
    if let Err(err) = write_audit_log(&refund_id, audit_should_fail) {
        println!("  [warn] audit log degraded: {err}");
    }

    Ok(refund_id)
}

// =============================================================================
// 第三部分：该不该传播？关键链路 vs 非关键链路
// =============================================================================
//
// 题目：
// 构建用户首页时，用户资料服务失败应该直接报错，因为页面无法渲染；
// 推荐服务失败则可以降级成空列表。
//
// 希望候选人说明：
// 1. 什么叫关键依赖 / 非关键依赖？
// 2. 降级之后是否应该吞错？还是要记录日志 / 指标？
// 3. 什么时候降级会掩盖线上故障？

#[derive(Debug)]
struct UserProfile {
    user_id: u64,
    display_name: String,
}

#[derive(Debug)]
struct HomePage {
    profile: UserProfile,
    recommended_items: Vec<String>,
}

#[derive(Debug)]
enum ProfileError {
    NotFound(u64),
    DependencyDown(&'static str),
}

impl Display for ProfileError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound(user_id) => write!(f, "user `{user_id}` not found"),
            Self::DependencyDown(name) => write!(f, "dependency `{name}` is unavailable"),
        }
    }
}

impl Error for ProfileError {}

fn fetch_profile(user_id: u64) -> Result<UserProfile, ProfileError> {
    if user_id == 404 {
        return Err(ProfileError::NotFound(user_id));
    }
    if user_id == 503 {
        return Err(ProfileError::DependencyDown("profile-service"));
    }

    Ok(UserProfile {
        user_id,
        display_name: format!("user-{user_id}"),
    })
}

fn fetch_recommendations(user_id: u64) -> Result<Vec<String>, ProfileError> {
    if user_id == 42 {
        return Err(ProfileError::DependencyDown("recommendation-service"));
    }

    Ok(vec!["keyboard".into(), "monitor".into()])
}

fn build_home_page(user_id: u64) -> Result<HomePage, ProfileError> {
    let profile = fetch_profile(user_id)?;

    let recommended_items = match fetch_recommendations(user_id) {
        Ok(items) => items,
        Err(err) => {
            println!("  [warn] recommendations degraded for user {user_id}: {err}");
            Vec::new()
        }
    };

    Ok(HomePage {
        profile,
        recommended_items,
    })
}

// =============================================================================
// 第四部分：`Result<Option<T>, E>` 的真实含义
// =============================================================================
//
// 题目：
// 营销服务查找优惠券时：
// - 用户没有可用优惠券：是正常业务结果，不是错误
// - Redis 连接超时：是系统错误
//
// 希望候选人说明：
// 1. 为什么这里不能只返回 Option？
// 2. 为什么这里也不应该把“查无数据”建模成 Err？

#[derive(Debug)]
struct Coupon {
    code: String,
    discount_percent: u8,
}

#[derive(Debug)]
enum CouponLookupError {
    CacheUnavailable,
    CorruptedRecord(String),
}

impl Display for CouponLookupError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::CacheUnavailable => write!(f, "coupon cache is unavailable"),
            Self::CorruptedRecord(raw) => write!(f, "coupon record `{raw}` is corrupted"),
        }
    }
}

impl Error for CouponLookupError {}

fn lookup_coupon(user_id: u64) -> Result<Option<Coupon>, CouponLookupError> {
    match user_id {
        7 => Ok(Some(Coupon {
            code: "SPRING-20".into(),
            discount_percent: 20,
        })),
        8 => Ok(None),
        9 => Err(CouponLookupError::CacheUnavailable),
        _ => Err(CouponLookupError::CorruptedRecord("bad-payload".into())),
    }
}

// =============================================================================
// 第五部分：批处理的 trade-off
// =============================================================================
//
// 题目：
// 财务导入每日订单文件时，两种策略都可能合理：
// 1. fail-fast：任何一行非法都立即中止，保证“全有或全无”
// 2. collect-all：继续处理，最后返回所有坏行，方便运营一次性修复
//
// 希望候选人说明：
// 1. 哪些业务更适合 fail-fast？
// 2. 哪些后台运营工具更适合 collect-all？
// 3. 返回第一条错误还是所有错误，对调用方体验有什么影响？

#[derive(Debug)]
struct ImportedOrder {
    order_id: String,
    cents: u64,
}

#[derive(Debug)]
enum ImportError {
    InvalidFormat {
        line: usize,
        raw: String,
    },
    InvalidAmount {
        line: usize,
        raw: String,
        source: ParseIntError,
    },
}

impl Display for ImportError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidFormat { line, raw } => {
                write!(f, "line {line} has invalid format: `{raw}`")
            }
            Self::InvalidAmount { line, raw, .. } => {
                write!(f, "line {line} has invalid amount: `{raw}`")
            }
        }
    }
}

impl Error for ImportError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InvalidAmount { source, .. } => Some(source),
            Self::InvalidFormat { .. } => None,
        }
    }
}

fn parse_order_line(line_no: usize, raw: &str) -> Result<ImportedOrder, ImportError> {
    let (order_id, cents) = raw
        .split_once(',')
        .ok_or_else(|| ImportError::InvalidFormat {
            line: line_no,
            raw: raw.to_string(),
        })?;

    let cents = cents
        .parse::<u64>()
        .map_err(|source| ImportError::InvalidAmount {
            line: line_no,
            raw: cents.to_string(),
            source,
        })?;

    Ok(ImportedOrder {
        order_id: order_id.to_string(),
        cents,
    })
}

fn import_orders_fail_fast(lines: &[&str]) -> Result<Vec<ImportedOrder>, ImportError> {
    lines
        .iter()
        .enumerate()
        .map(|(idx, raw)| parse_order_line(idx + 1, raw))
        .collect()
}

fn import_orders_collect_all(lines: &[&str]) -> (Vec<ImportedOrder>, Vec<ImportError>) {
    let mut ok_orders = Vec::new();
    let mut errors = Vec::new();

    for (idx, raw) in lines.iter().enumerate() {
        match parse_order_line(idx + 1, raw) {
            Ok(order) => ok_orders.push(order),
            Err(err) => errors.push(err),
        }
    }

    (ok_orders, errors)
}

// =============================================================================
// 第六部分：打印错误链，模拟线上排障
// =============================================================================

fn print_error_chain(err: &dyn Error) {
    println!("  error: {err}");
    let mut current = err.source();
    while let Some(source) = current {
        println!("    caused by: {source}");
        current = source.source();
    }
}

fn create_demo_config_file(content: &str) -> io::Result<PathBuf> {
    let path = std::env::temp_dir().join(format!(
        "error-propagation-demo-{}.conf",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_nanos()
    ));
    fs::write(&path, content)?;
    Ok(path)
}

// =============================================================================
// 主入口：运行所有场景
// =============================================================================

fn run_examples() -> Result<(), Box<dyn Error>> {
    println!("=== [1] 启动配置：该不该继续向上传播？ ===\n");

    let config_path = create_demo_config_file("port=8080\nmode=prod")?;
    let env = HashMap::new();
    println!(
        "  load_http_port(file fallback): {:?}",
        load_http_port(&env, &config_path)
    );

    let bad_env = HashMap::from([("APP_PORT".into(), "abc".into())]);
    match load_http_port(&bad_env, &config_path) {
        Ok(port) => println!("  loaded port: {port}"),
        Err(err) => print_error_chain(&err),
    }

    println!("\n=== [2] 退款链路：`?` + `From` + 分层错误 ===\n");

    let payment_env = HashMap::from([("PAYMENT_API_KEY".into(), "live-secret-key".into())]);
    let ok_request = RefundRequest {
        order_id: "order-1001".into(),
        cents: 2999,
    };
    println!(
        "  issue_refund(success): {:?}",
        issue_refund(&payment_env, &ok_request, false)
    );
    println!(
        "  issue_refund(audit degraded): {:?}",
        issue_refund(&payment_env, &ok_request, true)
    );

    let timeout_request = RefundRequest {
        order_id: "timeout-order".into(),
        cents: 2999,
    };
    match issue_refund(&payment_env, &timeout_request, false) {
        Ok(refund_id) => println!("  timeout request refund id: {refund_id}"),
        Err(err) => print_error_chain(&err),
    }

    println!("\n=== [3] 首页聚合：关键依赖传播，非关键依赖降级 ===\n");

    let normal_page = build_home_page(1)?;
    println!(
        "  build_home_page(1): profile={}, rec_count={}",
        normal_page.profile.display_name,
        normal_page.recommended_items.len()
    );

    let degraded_page = build_home_page(42)?;
    println!(
        "  build_home_page(42): profile={}, rec_count={}",
        degraded_page.profile.user_id,
        degraded_page.recommended_items.len()
    );

    match build_home_page(503) {
        Ok(page) => println!("  unexpected success: {:?}", page),
        Err(err) => print_error_chain(&err),
    }

    println!("\n=== [4] Result<Option<T>, E>：业务无数据 vs 系统错误 ===\n");

    for user_id in [7_u64, 8, 9, 10] {
        match lookup_coupon(user_id) {
            Ok(Some(coupon)) => println!(
                "  lookup_coupon({user_id}): code={}, discount={}%",
                coupon.code, coupon.discount_percent
            ),
            Ok(None) => println!("  lookup_coupon({user_id}): no coupon"),
            Err(err) => println!("  lookup_coupon({user_id}): err={err}"),
        }
    }

    println!("\n=== [5] 批处理：fail-fast vs collect-all ===\n");

    let lines = ["order-1,1999", "broken-line", "order-2,abc", "order-3,3999"];
    println!(
        "  import_orders_fail_fast: {:?}",
        import_orders_fail_fast(&lines)
    );

    let (orders, errors) = import_orders_collect_all(&lines);
    let order_summaries: Vec<String> = orders
        .iter()
        .map(|order| format!("{}:{}c", order.order_id, order.cents))
        .collect();
    println!("  import_orders_collect_all orders: {:?}", order_summaries);
    println!("  import_orders_collect_all errors: {:?}", errors);

    Ok(())
}

fn main() {
    if let Err(err) = run_examples() {
        eprintln!("fatal demo failure:");
        print_error_chain(err.as_ref());
    }
}
