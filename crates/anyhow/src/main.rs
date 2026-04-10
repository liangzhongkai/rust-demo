use anyhow::{anyhow, bail, ensure, Context, Result};
use std::io;

// ============================================================
// 1. 自定义错误类型 —— 用 thiserror 定义结构化领域错误，
//    anyhow 负责在应用层统一擦除类型并附加上下文。
// ============================================================

#[derive(Debug, thiserror::Error)]
enum AppError {
    #[error("user `{username}` not found (id={id})")]
    UserNotFound { id: u64, username: String },

    #[error("permission denied: {action} requires role `{required_role}`")]
    PermissionDenied {
        action: String,
        required_role: String,
    },

    #[error("config key `{key}` is invalid: {reason}")]
    InvalidConfig { key: String, reason: String },
}

// ============================================================
// 2. anyhow::Result 作为应用层统一返回类型
//    —— 任何实现了 std::error::Error 的类型都可以用 ? 自动转换
// ============================================================

fn load_config(path: &str) -> Result<serde_json::Value> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read config from `{path}`"))?;

    let config: serde_json::Value = serde_json::from_str(&content)
        .with_context(|| format!("failed to parse `{path}` as JSON"))?;

    let version = config
        .get("version")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| anyhow!("missing or invalid `version` field in config"))?;

    ensure!(
        version >= 2,
        "config version {version} is too old, need >= 2"
    );

    Ok(config)
}

// ============================================================
// 3. bail! —— 提前中断并返回错误，比 return Err(anyhow!(...)) 更简洁
// ============================================================

fn validate_username(name: &str) -> Result<()> {
    if name.is_empty() {
        bail!("username must not be empty");
    }
    if name.len() > 32 {
        bail!(
            "username `{name}` exceeds 32-char limit (got {})",
            name.len()
        );
    }
    if !name.chars().all(|c| c.is_alphanumeric() || c == '_') {
        bail!("username `{name}` contains invalid characters");
    }
    Ok(())
}

// ============================================================
// 4. Context / with_context —— 为底层错误附加"调用侧语义"
//    形成错误链：高层描述 → 中层描述 → 底层 root cause
// ============================================================

fn find_user(id: u64) -> Result<String> {
    let db_path = format!("/tmp/users/{id}.json");
    let raw = std::fs::read_to_string(&db_path)
        .with_context(|| format!("failed to load user record for id={id}"))?;

    let record: serde_json::Value =
        serde_json::from_str(&raw).context("corrupted user record: invalid JSON")?;

    let name = record
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AppError::UserNotFound {
            id,
            username: "<unknown>".into(),
        })
        .context("user record missing `name` field")?;

    Ok(name.to_owned())
}

// ============================================================
// 5. 错误链的完整打印 —— 逐层展示 source chain
// ============================================================

fn print_error_chain(err: &anyhow::Error) {
    eprintln!("Error: {err}");
    for (i, cause) in err.chain().skip(1).enumerate() {
        eprintln!("  cause[{i}]: {cause}");
    }
}

// ============================================================
// 6. downcast —— 从擦除后的 anyhow::Error 中恢复具体类型，
//    实现"应用层擦除、处理层精确匹配"的模式
// ============================================================

fn handle_with_downcast(result: Result<String>) {
    match result {
        Ok(name) => println!("  found user: {name}"),
        Err(err) => {
            if let Some(app_err) = err.downcast_ref::<AppError>() {
                match app_err {
                    AppError::UserNotFound { id, username } => {
                        eprintln!("  [RECOVERABLE] user not found: id={id}, name={username}");
                    }
                    AppError::PermissionDenied {
                        action,
                        required_role,
                    } => {
                        eprintln!("  [AUTH] denied `{action}`, need role `{required_role}`");
                    }
                    AppError::InvalidConfig { key, reason } => {
                        eprintln!("  [CONFIG] bad key `{key}`: {reason}");
                    }
                }
            } else if err.downcast_ref::<io::Error>().is_some() {
                eprintln!("  [IO] {err:#}");
            } else {
                eprintln!("  [UNKNOWN] {err:#}");
            }
        }
    }
}

// ============================================================
// 7. 在闭包 / iterator 中使用 anyhow
// ============================================================

fn parse_id_list(input: &str) -> Result<Vec<u64>> {
    input
        .split(',')
        .enumerate()
        .map(|(idx, seg)| {
            let trimmed = seg.trim();
            trimmed
                .parse::<u64>()
                .with_context(|| format!("item[{idx}] `{trimmed}` is not a valid u64"))
        })
        .collect::<Result<Vec<_>>>()
        .context("failed to parse id list")
}

// ============================================================
// 8. 格式化输出：Display vs Debug vs Alternate ({:#})
// ============================================================

fn demonstrate_formatting() {
    let inner: Result<()> = Err(anyhow!("disk full"));
    let outer: Result<()> = inner
        .context("failed to write cache")
        .context("sync aborted");
    let err = outer.unwrap_err();

    println!("  Display   : {err}");
    println!("  Alternate : {err:#}");
    println!("  Debug     : {err:?}");
    println!("  Alt-Debug : {err:#?}");
}

// ============================================================
// 9. 与 main 返回 Result 结合 —— 进程退出码 + 人类可读输出
// ============================================================

fn run_app() -> Result<()> {
    println!("=== [1] load_config: Context + ensure ===");
    match load_config("/tmp/nonexistent.json") {
        Ok(cfg) => println!("  config loaded: {cfg}"),
        Err(e) => print_error_chain(&e),
    }

    println!("\n=== [2] validate_username: bail! ===");
    for name in ["alice", "", "a]b", "a]bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"] {
        match validate_username(name) {
            Ok(()) => println!("  `{name}` -> OK"),
            Err(e) => eprintln!("  `{name}` -> {e}"),
        }
    }

    println!("\n=== [3] find_user: chained Context ===");
    handle_with_downcast(find_user(42));

    println!("\n=== [4] parse_id_list: anyhow in iterators ===");
    for input in ["1, 2, 3", "10,abc,30"] {
        match parse_id_list(input) {
            Ok(ids) => println!("  \"{input}\" -> {ids:?}"),
            Err(e) => print_error_chain(&e),
        }
    }

    println!("\n=== [5] formatting styles ===");
    demonstrate_formatting();

    println!("\n=== [6] downcast: recover typed error ===");
    let typed_err: Result<String> = Err(AppError::PermissionDenied {
        action: "delete_user".into(),
        required_role: "admin".into(),
    }
    .into());
    handle_with_downcast(typed_err);

    let config_err: Result<String> = Err(AppError::InvalidConfig {
        key: "max_retries".into(),
        reason: "expected integer, got string".into(),
    }
    .into());
    handle_with_downcast(config_err);

    let untyped_err: Result<String> = Err(anyhow!("something unexpected"));
    handle_with_downcast(untyped_err);

    println!("\n=== [7] anyhow! with structured format args ===");
    let port = 8080;
    let err = anyhow!("port {port} already in use, try {}", port + 1);
    eprintln!("  {err}");

    Ok(())
}

fn main() -> Result<()> {
    run_app()
}
