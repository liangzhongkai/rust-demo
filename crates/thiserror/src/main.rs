//! thiserror 深度实践
//!
//! 考察点：
//! 1. `#[derive(Error)]` 与手写 impl Error 的取舍：何时值得引入依赖？
//! 2. `#[from]` vs `#[source]`：有额外上下文时为什么不能用 #[from]？
//! 3. `#[error(transparent)]`：库的公开 API 如何做 opaque error，避免泄露实现细节？
//! 4. Display 消息的 `{var}` / `{0}` / `{var:?}` 插值，以及何时需要自定义表达式？
//! 5. 错误链设计：库边界用 thiserror 结构化，应用层用 anyhow 的职责划分
//! 6. 生产场景：配置加载、RPC 调用、数据库、批处理中的错误建模 trade-off
//! 7. 库 vs 应用边界：库用 thiserror 返回结构化错误，应用用 anyhow 聚合

use std::error::Error;
use std::fs;
use std::io;
use std::num::ParseIntError;
use std::path::{Path, PathBuf};

use thiserror::Error;

// =============================================================================
// 第一部分：基础 derive 与 Display 插值
// =============================================================================
//
// 题目：
// 数据存储服务有多种错误：断开连接、键不存在、头部格式错误。
// 用 thiserror 派生，并展示 {var}、{0}、{var:?} 的用法。
//
// 希望候选人说明：
// 1. 为什么 `#[error("...")]` 能替代手写 Display + Error？
// 2. `{expected:?}` 和 `{expected}` 在日志里有什么区别？
// 3. thiserror 是否出现在你的 public API 里？（答案：不出现，可随时替换）

#[derive(Error, Debug)]
pub enum DataStoreError {
    #[error("data store disconnected")]
    Disconnect(#[from] io::Error),

    #[error("the data for key `{0}` is not available")]
    Redaction(String),

    #[error("invalid header (expected {expected:?}, found {found:?})")]
    InvalidHeader {
        expected: String,
        found: String,
    },

    #[error("unknown data store error")]
    Unknown,
}

// =============================================================================
// 第二部分：#[from] vs #[source] —— 有额外上下文时为什么不能用 #[from]
// =============================================================================
//
// 题目：
// 配置加载：读取文件失败时，需要附带 path；解析端口失败时，需要附带 raw 字符串。
// 这些 variant 有「source + 额外字段」，不能只用 #[from]。
//
// 希望候选人说明：
// 1. #[from] 要求 variant 只包含 source（可选 backtrace），为什么？
// 2. 有 path、raw 等上下文时，应该用 #[source] 还是 #[from]？
// 3. 手写 From 和 impl source() 时，如何保证错误链正确？

#[derive(Error, Debug)]
pub enum ConfigError {
    #[error("missing `APP_PORT` and no `port=` entry in config file")]
    MissingPortConfig,

    #[error("failed to read config file `{path}`")]
    ReadConfigFile {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("invalid port `{raw}`")]
    InvalidPort {
        raw: String,
        #[source]
        source: ParseIntError,
    },
}

impl ConfigError {
    fn read_config_file(path: PathBuf, source: io::Error) -> Self {
        Self::ReadConfigFile { path, source }
    }

    fn invalid_port(raw: String, source: ParseIntError) -> Self {
        Self::InvalidPort { raw, source }
    }
}

fn parse_port(raw: &str) -> Result<u16, ConfigError> {
    raw.trim().parse::<u16>().map_err(|source| {
        ConfigError::invalid_port(raw.trim().to_string(), source)
    })
}

fn read_port_from_file(path: &Path) -> Result<u16, ConfigError> {
    let content = fs::read_to_string(path).map_err(|source| {
        ConfigError::read_config_file(path.to_path_buf(), source)
    })?;

    let raw = content
        .lines()
        .find_map(|line| line.strip_prefix("port="))
        .ok_or(ConfigError::MissingPortConfig)?;

    parse_port(raw)
}

// =============================================================================
// 第三部分：#[error(transparent)] —— 库的 opaque error
// =============================================================================
//
// 题目：
// 你维护一个 HTTP 客户端库。内部实现可能用 reqwest、hyper 等，错误类型会变。
// 对外只暴露一个 PublicError，调用方不应依赖内部 Error 类型。
//
// 希望候选人说明：
// 1. 为什么库的公开错误类型要「不透明」？
// 2. #[error(transparent)] 做了什么？Display 和 source 如何转发？
// 3. 如果内部实现从 reqwest 换成 hyper，PublicError 的 API 是否兼容？

#[derive(Error, Debug)]
#[error(transparent)]
pub struct PublicError(#[from] ErrorRepr);

impl PublicError {
    pub fn is_timeout(&self) -> bool {
        matches!(self.0, ErrorRepr::Timeout)
    }

    pub fn is_connection_refused(&self) -> bool {
        matches!(self.0, ErrorRepr::ConnectionRefused)
    }
}

#[derive(Error, Debug)]
enum ErrorRepr {
    #[error("request timeout")]
    Timeout,

    #[error("connection refused")]
    ConnectionRefused,

    #[error("invalid header value")]
    InvalidHeader(#[from] std::io::Error),
}

// =============================================================================
// 第四部分：RPC 调用链 —— #[from] 自动传播 + 分层错误
// =============================================================================
//
// 题目：
// 退款服务：校验 → 网关调用 → 审计。支付网关返回 Timeout、Unauthorized 等。
// 用 #[from] 简化 PaymentGatewayError -> RefundError 的转换。
//
// 希望候选人说明：
// 1. 为什么 PaymentGatewayError 适合用 #[from] 自动转？
// 2. Validation、MissingApiKey 为什么不能用 #[from]？
// 3. 错误链打印时，上层和下层如何区分？

#[derive(Error, Debug)]
pub enum PaymentGatewayError {
    #[error("payment gateway timeout")]
    Timeout,

    #[error("payment gateway rejected api key")]
    Unauthorized,

    #[error("payment gateway rejected request: {0}")]
    UpstreamRejected(String),
}

#[derive(Error, Debug)]
pub enum RefundError {
    #[error("invalid refund request: {0}")]
    Validation(String),

    #[error("missing env `PAYMENT_API_KEY`")]
    MissingApiKey,

    #[error("refund failed because upstream payment failed")]
    Payment(#[from] PaymentGatewayError),
}

// =============================================================================
// 第五部分：批处理错误 —— 多字段 + 行号上下文
// =============================================================================
//
// 题目：
// 财务导入每日订单文件，每行格式：order_id,cents。解析失败需记录行号和原始内容。
//
// 希望候选人说明：
// 1. InvalidFormat 和 InvalidAmount 的 source 设计差异？
// 2. 为什么 InvalidAmount 需要 #[source]？
// 3. 手写 vs thiserror 时，Display 的维护成本？

#[derive(Error, Debug)]
pub enum ImportError {
    #[error("line {line} has invalid format: `{raw}`")]
    InvalidFormat { line: usize, raw: String },

    #[error("line {line} has invalid amount: `{raw}`")]
    InvalidAmount {
        line: usize,
        raw: String,
        #[source]
        source: ParseIntError,
    },
}

fn parse_order_line(line_no: usize, raw: &str) -> Result<(String, u64), ImportError> {
    let (order_id, cents) = raw
        .split_once(',')
        .ok_or_else(|| ImportError::InvalidFormat {
            line: line_no,
            raw: raw.to_string(),
        })?;

    let cents = cents.parse::<u64>().map_err(|source| ImportError::InvalidAmount {
        line: line_no,
        raw: cents.to_string(),
        source,
    })?;

    Ok((order_id.to_string(), cents))
}

// =============================================================================
// 第六部分：Display 表达式 —— 复杂格式
// =============================================================================
//
// 题目：
// 有时需要 #[error("...", expr)] 做额外计算，例如 first_char(.0)。
//
// 希望候选人说明：
// 1. 为什么不能直接用 {.0} 取 first char？
// 2. 表达式参数和字段插值可以混用吗？

#[derive(Error, Debug)]
pub enum ValidationError {
    #[error("first letter must be lowercase but was {:?}", .0.chars().next())]
    WrongCase(String),

    #[error("invalid index {idx}, expected 0..{max}", max = 100)]
    OutOfBounds { idx: usize },
}

// =============================================================================
// 第七部分：错误链打印 —— 与 error-propagation 对比
// =============================================================================

fn print_error_chain(err: &dyn Error) {
    println!("  error: {err}");
    let mut current = err.source();
    while let Some(source) = current {
        println!("    caused by: {source}");
        current = source.source();
    }
}

// =============================================================================
// 主入口：运行所有场景
// =============================================================================

fn run_examples() -> Result<(), Box<dyn Error>> {
    println!("=== [1] 基础 derive：Display 插值 ===\n");

    let err: DataStoreError = io::Error::from(io::ErrorKind::ConnectionRefused).into();
    println!("  Disconnect: {err}");

    let err = DataStoreError::Redaction("user:123".into());
    println!("  Redaction: {err}");

    let err = DataStoreError::InvalidHeader {
        expected: "application/json".into(),
        found: "text/plain".into(),
    };
    println!("  InvalidHeader: {err}");
    println!("  (Debug: {:?})", err);

    println!("\n=== [2] #[from] vs #[source]：有额外上下文 ===\n");

    let path = Path::new("/nonexistent/config.conf");
    match fs::read_to_string(path) {
        Ok(_) => println!("  unexpected success"),
        Err(e) => {
            let err = ConfigError::read_config_file(path.to_path_buf(), e);
            println!("  ReadConfigFile: {err}");
            print_error_chain(&err);
        }
    }

    match parse_port("not-a-number") {
        Ok(_) => println!("  unexpected success"),
        Err(err) => {
            println!("  InvalidPort: {err}");
            print_error_chain(&err);
        }
    }

    // 成功场景：从临时文件加载
    let temp_config = std::env::temp_dir().join("thiserror-demo-port.conf");
    fs::write(&temp_config, "port=9090\n").unwrap();
    match read_port_from_file(&temp_config) {
        Ok(port) => println!("  read_port_from_file(success): {port}"),
        Err(err) => println!("  read_port_from_file: {err}"),
    }

    println!("\n=== [3] #[error(transparent)]：Opaque 库错误 ===\n");

    let err: PublicError = ErrorRepr::Timeout.into();
    println!("  PublicError(Timeout): {err}");
    println!("  is_timeout: {}", err.is_timeout());

    let err: PublicError = ErrorRepr::ConnectionRefused.into();
    println!("  PublicError(ConnectionRefused): {err}");
    println!("  is_connection_refused: {}", err.is_connection_refused());

    println!("\n=== [4] RPC 退款链：#[from] 自动传播 ===\n");

    let err: RefundError = PaymentGatewayError::Timeout.into();
    println!("  RefundError(Payment): {err}");
    print_error_chain(&err);

    println!("\n=== [5] 批处理：多字段 + source ===\n");

    let lines = ["order-1,1999", "broken-line", "order-2,abc"];
    for (i, line) in lines.iter().enumerate() {
        match parse_order_line(i + 1, line) {
            Ok((id, cents)) => println!("  line {}: {:?} -> {}:{}c", i + 1, line, id, cents),
            Err(err) => {
                println!("  line {}: {:?} -> err: {}", i + 1, line, err);
                if let Some(source) = err.source() {
                    println!("    caused by: {source}");
                }
            }
        }
    }

    println!("\n=== [6] Display 表达式 ===\n");

    let err = ValidationError::WrongCase("Hello".into());
    println!("  WrongCase: {err}");

    let err = ValidationError::OutOfBounds { idx: 999 };
    println!("  OutOfBounds: {err}");

    println!("\n=== [7] 库 vs 应用边界（说明）===\n");
    println!("  库层：用 thiserror 定义 RefundError、ConfigError 等，返回 Result<T, E>");
    println!("  应用层：用 anyhow::Result<T> 或 .context() 聚合，? 自动 From 转换");
    println!("  Trade-off：库需要稳定、可匹配的错误类型；应用需要快速开发、丰富上下文");

    Ok(())
}

fn main() {
    if let Err(err) = run_examples() {
        eprintln!("fatal demo failure:");
        print_error_chain(err.as_ref());
    }
}
