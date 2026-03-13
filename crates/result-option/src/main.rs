//! Result 与 Option 深度实践
//!
//! 考察点：
//! 1. 基础语义与组合子
//! 2. Option vs Result 的 trade-off
//! 3. unwrap / expect / ? 的适用场景
//! 4. 生产场景还原

use std::collections::HashMap;
use std::str::FromStr;

// =============================================================================
// 第一部分：基础理解 —— Option 与 Result 的语义边界
// =============================================================================

/// 场景：用户 ID 查找 —— "不存在" 是业务常态，用 Option
fn find_user_by_id(users: &HashMap<u64, String>, id: u64) -> Option<&String> {
    users.get(&id)
}

/// 场景：解析配置 —— "解析失败" 是异常，需要错误信息，用 Result
fn parse_port(config: &str) -> Result<u16, String> {
    u16::from_str(config).map_err(|_| format!("invalid port: `{config}`"))
}

/// 场景：可选配置 —— 键不存在是合法状态，用 Option<Result<T, E>>
/// Trade-off: 键存在但值非法时，需要区分 "未配置" vs "配置错误"
fn get_optional_port(config: &HashMap<&str, &str>, key: &str) -> Option<Result<u16, String>> {
    config.get(key).map(|v| parse_port(v))
}

// =============================================================================
// 第二部分：组合子 —— map, and_then, or_else 的链式表达
// =============================================================================

/// 生产场景：从环境变量读取端口，带默认值
fn port_from_env_or_default(env: &HashMap<String, String>, default: u16) -> u16 {
    env.get("PORT")
        .and_then(|s| s.parse().ok())
        .unwrap_or(default)
}

/// 生产场景：级联查找 —— 内存缓存 -> 本地文件 -> 默认值
fn get_config_value(
    cache: &HashMap<String, String>,
    file_content: Option<&str>,
    key: &str,
    default: &str,
) -> String {
    cache
        .get(key)
        .map(String::clone)
        .or_else(|| {
            file_content.and_then(|c| {
                c.lines()
                    .find(|l| l.starts_with(&format!("{key}=")))
                    .and_then(|l| l.split('=').nth(1).map(String::from))
            })
        })
        .unwrap_or_else(|| default.to_string())
}

/// 生产场景：Result 链式传播 —— 多步校验，任一步失败即返回
fn validate_and_parse_user_input(raw_id: &str, raw_age: &str) -> Result<(u64, u8), String> {
    let id = raw_id
        .parse::<u64>()
        .map_err(|_| format!("invalid user id: `{raw_id}`"))?;
    let age = raw_age
        .parse::<u8>()
        .map_err(|_| format!("invalid age: `{raw_age}`"))?;
    if age > 150 {
        return Err(format!("age {age} out of range"));
    }
    Ok((id, age))
}

// =============================================================================
// 第三部分：Trade-off —— unwrap vs expect vs ? 的取舍
// =============================================================================

/// unwrap：仅用于原型/测试，或逻辑上不可能为 None/Err 的断言
/// 生产代码慎用：panic 会终止整个进程
#[allow(dead_code)]
fn demo_unwrap_only_for_prototype() {
    let _: u16 = "8080".parse().unwrap();
}

/// expect：比 unwrap 好 —— 失败时给出上下文，便于排查
/// 适用："不可能失败" 的断言，但希望 panic 信息有意义
#[allow(dead_code)]
fn demo_expect_for_assertion() {
    let config = std::env::var("CONFIG_PATH").expect("CONFIG_PATH must be set in production");
    let _ = config;
}

/// ? 操作符：生产首选 —— 错误向上传播，由调用方统一处理
#[allow(dead_code)]
fn load_user_preferences(path: &str) -> Result<String, std::io::Error> {
    let content = std::fs::read_to_string(path)?;
    Ok(content)
}

// =============================================================================
// 第四部分：Option 与 Result 的相互转换
// =============================================================================

/// ok_or / ok_or_else：Option -> Result，在 "None 即错误" 时使用
fn require_env(key: &str, env: &HashMap<String, String>) -> Result<String, String> {
    env.get(key)
        .cloned()
        .ok_or_else(|| format!("required env `{key}` is not set"))
}

/// 反向：Result -> Option，当不关心错误细节、只关心成功与否时
fn try_parse_port(s: &str) -> Option<u16> {
    s.parse().ok()
}

// =============================================================================
// 第五部分：生产场景 —— 配置加载、缓存、权限
// =============================================================================

/// 模拟：从多源加载配置，优先顺序 + 校验
struct AppConfig {
    port: u16,
    debug: bool,
}

fn load_app_config(
    env: &HashMap<String, String>,
    defaults: &HashMap<&str, &str>,
) -> Result<AppConfig, String> {
    let port = env
        .get("PORT")
        .map(|s| s.as_str())
        .or(defaults.get("port").copied())
        .ok_or("port not configured")?
        .parse()
        .map_err(|_| "port must be a valid u16")?;

    let debug = env
        .get("DEBUG")
        .map(|s| s.as_str())
        .or(defaults.get("debug").copied())
        .unwrap_or("false");
    let debug = matches!(debug.to_lowercase().as_str(), "1" | "true" | "yes");

    Ok(AppConfig { port, debug })
}

/// 模拟：带缓存的用户查询 —— 缓存 miss 返回 None，DB 错误返回 Err
#[derive(Debug, Clone)]
#[allow(dead_code)]
struct User {
    id: u64,
    name: String,
}

#[derive(Debug)]
enum UserLookupError {
    NotFound,
    #[allow(dead_code)]
    DbError(String), // 预留：DB 连接失败等系统错误
}

fn get_user_with_cache(
    cache: &HashMap<u64, User>,
    db: &HashMap<u64, User>,
    id: u64,
) -> Result<Option<User>, UserLookupError> {
    if let Some(u) = cache.get(&id) {
        return Ok(Some(u.clone()));
    }
    db.get(&id)
        .cloned()
        .map(Some)
        .ok_or(UserLookupError::NotFound)
}

/// 模拟：权限校验 —— 无权限是业务结果(Option)，系统错误是 Err(Result)
fn check_permission(user_role: &str, required_role: &str) -> Result<bool, String> {
    if user_role.is_empty() {
        return Err("user role not initialized".into());
    }
    Ok(user_role == required_role || user_role == "admin")
}

// =============================================================================
// 第六部分：迭代器中的 Result/Option —— collect 与 ?
// =============================================================================

/// 批量解析：任一失败则整体失败
fn parse_ids(ids: &[&str]) -> Result<Vec<u64>, String> {
    ids.iter()
        .enumerate()
        .map(|(i, s)| s.parse().map_err(|_| format!("ids[{i}] `{s}` is not u64")))
        .collect()
}

/// 批量解析：忽略失败项，只保留成功的
fn parse_ids_ignore_failures(ids: &[&str]) -> Vec<u64> {
    ids.iter().filter_map(|s| s.parse().ok()).collect()
}

// =============================================================================
// 第七部分：if let / while let —— 简洁的模式匹配
// =============================================================================

fn demo_if_let(config: &HashMap<&str, &str>) {
    if let Some(port_str) = config.get("port") {
        if let Ok(port) = port_str.parse::<u16>() {
            println!("  config port: {port}");
        }
    }
}

/// 模拟：从队列中持续取任务直到空
fn process_queue_until_empty(queue: &mut Vec<String>) {
    while let Some(task) = queue.pop() {
        println!("  processing: {task}");
    }
}

// =============================================================================
// 第八部分：常见陷阱 —— unwrap 滥用、Option/Result 混用
// =============================================================================

/// 陷阱 1：对可能为 None 的值 unwrap -> panic
#[allow(dead_code)]
fn trap_unwrap_on_none() {
    let _map: HashMap<i32, i32> = HashMap::new();
    // let _ = map.get(&1).unwrap(); // panic!
}

/// 陷阱 2：Option 和 Result 混用时，注意类型对齐
/// 正确做法：先统一成 Result，再 ? 传播
fn chain_option_and_result(
    maybe_id: Option<&str>,
    db: &HashMap<u64, String>,
) -> Result<String, String> {
    let id_str = maybe_id.ok_or("id not provided")?;
    let id = id_str
        .parse()
        .map_err(|_| format!("invalid id: {id_str}"))?;
    db.get(&id)
        .cloned()
        .ok_or_else(|| format!("user {id} not found"))
}

// =============================================================================
// 主入口：运行所有示例
// =============================================================================

fn run_examples() {
    println!("=== [1] Option vs Result 语义边界 ===\n");

    let users = HashMap::from([(1u64, "alice".into()), (2, "bob".into())]);
    println!("  find_user(1): {:?}", find_user_by_id(&users, 1));
    println!("  find_user(99): {:?}", find_user_by_id(&users, 99));

    println!("\n  parse_port(\"8080\"): {:?}", parse_port("8080"));
    println!("  parse_port(\"abc\"): {:?}", parse_port("abc"));

    let config = HashMap::from([("port", "8080"), ("bad_port", "xyz")]);
    println!(
        "\n  get_optional_port(port): {:?}",
        get_optional_port(&config, "port")
    );
    println!(
        "  get_optional_port(bad_port): {:?}",
        get_optional_port(&config, "bad_port")
    );
    println!(
        "  get_optional_port(missing): {:?}",
        get_optional_port(&config, "missing")
    );

    println!("\n=== [2] 组合子链式表达 ===\n");

    let env = HashMap::from([("PORT".into(), "3000".into()), ("OTHER".into(), "x".into())]);
    println!(
        "  port_from_env_or_default (PORT=3000): {}",
        port_from_env_or_default(&env, 8080)
    );

    let env_empty: HashMap<String, String> = HashMap::new();
    println!(
        "  port_from_env_or_default (no PORT): {}",
        port_from_env_or_default(&env_empty, 8080)
    );

    let cache = HashMap::from([("host".into(), "localhost".into())]);
    let file = Some("port=9090\nhost=remote");
    println!(
        "  get_config_value(host): {}",
        get_config_value(&cache, file, "host", "0.0.0.0")
    );
    println!(
        "  get_config_value(port from file): {}",
        get_config_value(&cache, file, "port", "8080")
    );

    println!(
        "\n  validate_and_parse_user_input(\"1\", \"25\"): {:?}",
        validate_and_parse_user_input("1", "25")
    );
    println!(
        "  validate_and_parse_user_input(\"x\", \"25\"): {:?}",
        validate_and_parse_user_input("x", "25")
    );

    println!("\n=== [3] Option <-> Result 转换 ===\n");

    println!("  require_env(PORT): {:?}", require_env("PORT", &env));
    println!("  require_env(MISSING): {:?}", require_env("MISSING", &env));
    println!("  try_parse_port(\"80\"): {:?}", try_parse_port("80"));
    println!("  try_parse_port(\"x\"): {:?}", try_parse_port("x"));

    println!("\n=== [4] 生产场景：配置加载 ===\n");

    let defaults = HashMap::from([("port", "8080"), ("debug", "false")]);
    match load_app_config(&env, &defaults) {
        Ok(cfg) => println!("  AppConfig {{ port: {}, debug: {} }}", cfg.port, cfg.debug),
        Err(e) => println!("  load_app_config error: {e}"),
    }

    println!("\n=== [5] 生产场景：带缓存的用户查询 ===\n");

    let cache_users = HashMap::from([(
        1,
        User {
            id: 1,
            name: "alice".into(),
        },
    )]);
    let db_users = HashMap::from([
        (
            1,
            User {
                id: 1,
                name: "alice".into(),
            },
        ),
        (
            2,
            User {
                id: 2,
                name: "bob".into(),
            },
        ),
    ]);
    println!(
        "  get_user(1, cache hit): {:?}",
        get_user_with_cache(&cache_users, &db_users, 1)
    );
    println!(
        "  get_user(2, cache miss): {:?}",
        get_user_with_cache(&cache_users, &db_users, 2)
    );
    println!(
        "  get_user(99): {:?}",
        get_user_with_cache(&cache_users, &db_users, 99)
    );

    println!("\n=== [6] 权限校验 ===\n");

    println!(
        "  check_permission(admin, user): {:?}",
        check_permission("admin", "user")
    );
    println!(
        "  check_permission(user, user): {:?}",
        check_permission("user", "user")
    );
    println!(
        "  check_permission(guest, admin): {:?}",
        check_permission("guest", "admin")
    );
    println!(
        "  check_permission(\"\", admin): {:?}",
        check_permission("", "admin")
    );

    println!("\n=== [7] 迭代器中的 Result/Option ===\n");

    println!(
        "  parse_ids([\"1\", \"2\", \"3\"]): {:?}",
        parse_ids(&["1", "2", "3"])
    );
    println!(
        "  parse_ids([\"1\", \"x\", \"3\"]): {:?}",
        parse_ids(&["1", "x", "3"])
    );
    println!(
        "  parse_ids_ignore_failures([\"1\", \"x\", \"3\"]): {:?}",
        parse_ids_ignore_failures(&["1", "x", "3"])
    );

    println!("\n=== [8] if let / while let ===\n");

    demo_if_let(&config);
    let mut queue = vec!["task_a".into(), "task_b".into()];
    print!("  process_queue_until_empty: ");
    process_queue_until_empty(&mut queue);

    println!("\n=== [9] Option + Result 链式混用 ===\n");

    let db = HashMap::from([(1u64, "alice".into())]);
    println!(
        "  chain_option_and_result(Some(\"1\")): {:?}",
        chain_option_and_result(Some("1"), &db)
    );
    println!(
        "  chain_option_and_result(Some(\"99\")): {:?}",
        chain_option_and_result(Some("99"), &db)
    );
    println!(
        "  chain_option_and_result(None): {:?}",
        chain_option_and_result(None, &db)
    );
}

fn main() {
    run_examples();
}
