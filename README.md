# Rust 深度实践

> 系统化深入理解 Rust 本质和应用场景

## 开发进度

### 已完成

| 主题 | 状态 | 说明 |
|------|------|------|
| **ownership** | ✅ 完成 | 包含基础、进阶、陷阱、实战四大模块，共 30+ 示例 |
| **iterators** | ✅ 完成 | basics + HFT 7 场景 + Web3 6 场景 + 8 陷阱 + 8 应对策略 |
| 项目结构 | ✅ 完成 | Workspace 配置，36 个主题占位符 |

### 待完成

34 个主题 crate 待实现，按优先级排序：
- 第一阶段（核心基础）: borrowing, lifetimes, trait-system, generics
- 其他 30 个主题

## 项目结构

本项目采用 Cargo Workspace 管理多个独立的示例 crate，每个主题都是一个独立的可执行项目，便于单独运行、测试和理解。

```
rust-demo/
├── Cargo.toml              # Workspace 配置
├── README.md               # 本文件
└── crates/                 # 所有示例 crate
    ├── ownership/          # 所有权系统
    ├── borrowing/          # 借用检查
    ├── lifetimes/          # 生命周期标注
    ├── trait-system/       # Trait 系统
    ├── generics/           # 泛型
    ├── smart-pointers/     # 智能指针
    ├── interior-mutability/# 内部可变性
    ├── type-coercion/      # 类型转换
    ├── copy-vs-clone/      # Copy vs Clone
    ├── threads/            # 线程
    ├── channels/           # 通道通信
    ├── mutex-locks/        # 互斥锁
    ├── async-runtime/      # 异步运行时
    ├── cancellation/       # 取消机制
    ├── result-option/      # Result 和 Option
    ├── error-propagation/  # 错误传播
    ├── thiserror/          # thiserror 库
    ├── anyhow/             # anyhow 库
    ├── closures/           # 闭包
    ├── iterators/          # 迭代器
    ├── macros/             # 宏
    ├── unsafe-rust/        # Unsafe Rust
    ├── ffi/                # FFI 外部函数接口
    ├── stack-vs-heap/      # 栈 vs 堆
    ├── arena-allocators/   # Arena 分配器
    ├── custom-allocators/  # 自定义分配器
    ├── reference-cycles/   # 引用循环
    ├── pattern-matching/   # 模式匹配
    ├── guards-ranges/      # 守卫和范围
    ├── data-structures/    # 数据结构
    ├── parsing/            # 解析
    ├── networking/         # 网络编程
    ├── file-io/            # 文件 I/O
    ├── zero-cost/          # 零成本抽象
    ├── inline-caching/     # 内联缓存
    ├── simd/               # SIMD 向量化
    └── profile-optimization/# 性能分析和优化
```

## 运行示例

```bash
# 运行特定 crate
cargo run -p ownership

# 运行特定示例（如果 crate 有多个 bin）
cargo run -p ownership --bin example_name

# 测试
cargo test -p ownership

# 检查（不构建）
cargo check -p ownership

# 构建 release 版本
cargo build --release -p ownership
```

## 学习路径

### 第一阶段：核心基础

| 主题 | 说明 | 关键概念 |
|------|------|----------|
| **ownership** | 所有权是 Rust 的核心 | Move 语义、Copy、Drop |
| **borrowing** | 借用规则 | 可变借用、不可变借用、借用检查器 |
| **lifetimes** | 生命周期标注 | 显式生命周期、生命周期省略规则 |
| **trait-system** | Trait 系统 | Trait bound、关联类型、Trait 对象 |
| **generics** | 泛型 | 类型参数、约束、单态化 |

### 第二阶段：类型系统

| 主题 | 说明 | 关键概念 |
|------|------|----------|
| **smart-pointers** | 智能指针 | Box, Rc, Arc, Weak, Cow |
| **interior-mutability** | 内部可变性 | Cell, RefCell, UnsafeCell |
| **type-coercion** | 类型转换 | Coercion, Cast, Transmute |
| **copy-vs-clone** | 复制语义 | Copy trait, Clone trait |

### 第三阶段：并发与异步

| 主题 | 说明 | 关键概念 |
|------|------|----------|
| **threads** | 线程 | std::thread, Scoped 线程 |
| **channels** | 通道 | mpsc, oneshot, watch |
| **mutex-locks** | 同步原语 | Mutex, RwLock, Atomic |
| **async-runtime** | 异步运行时 | Future, Async/Await, Executor |
| **cancellation** | 取消机制 | Drop guard, Cooperative cancellation |

### 第四阶段：错误处理

| 主题 | 说明 | 关键概念 |
|------|------|----------|
| **result-option** | 类型安全错误处理 | Result, Option, 组合子 |
| **error-propagation** | 错误传播 | ?, From, chain error |
| **thiserror** | 结构化错误 | 派生宏, 错误上下文 |
| **anyhow** | 便捷错误处理 | Context, anyhow::Error |

### 第五阶段：高级特性

| 主题 | 说明 | 关键概念 |
|------|------|----------|
| **closures** | 闭包 | 捕获、Fn traits、去糖化 |
| **iterators** | 迭代器 | 惰性求值、适配器、消费者 |
| **macros** | 宏 | 声明宏、过程宏、derive |
| **unsafe-rust** | Unsafe | 五种场景、未定义行为 |
| **ffi** | 外部函数接口 | extern "C", ABI,裸指针 |

### 第六阶段：内存管理

| 主题 | 说明 | 关键概念 |
|------|------|----------|
| **stack-vs-heap** | 栈与堆 | 内存布局、分配开销 |
| **arena-allocators** | Arena 分配 | Bump, typed_arena |
| **custom-allocators** | 自定义分配器 | Allocator API, GlobalAlloc |
| **reference-cycles** | 引用循环 | 内存泄漏、Weak |

### 第七阶段：模式匹配

| 主题 | 说明 | 关键概念 |
|------|------|----------|
| **pattern-matching** | 模式匹配 | 字面量、通配符、结构、范围 |
| **guards-ranges** | 守卫和范围 | Match 守卫、Range 模式 |

### 第八阶段：实战应用

| 主题 | 说明 | 关键概念 |
|------|------|----------|
| **data-structures** | 数据结构 | 链表、树、图、哈希表 |
| **parsing** | 解析 | 组合子、递归下降 |
| **networking** | 网络编程 | TCP/UDP、Tokio |
| **file-io** | 文件 I/O | BufRead, mmap, 异步 I/O |

### 第九阶段：性能优化

| 主题 | 说明 | 关键概念 |
|------|------|----------|
| **zero-cost** | 零成本抽象 | 内联、单态化 |
| **inline-caching** | 内联缓存 | 缓存友好性 |
| **simd** | SIMD 向量化 | portable_simd, packed_simd |
| **profile-optimization** | 性能分析 | perf, flamegraph, criterion |

## 每个 Crate 的组织规范

每个示例 crate 遵循以下结构：

```
crates/topic-name/
├── Cargo.toml           # 依赖配置
├── README.md            # 主题说明和学习目标
└── src/
    ├── main.rs          # 可执行的入口示例
    ├── basics.rs        # 基础示例
    ├── advanced.rs      # 进阶示例
    ├── pitfalls.rs      # 常见陷阱
    └── real_world.rs    # 实际应用场景
```

## 学习建议

1. **按顺序学习**：建议按学习路径顺序，从所有权开始
2. **动手实验**：每个示例都应亲自运行和修改
3. **阅读注释**：代码中有详细注释解释原理
4. **查看汇编**：使用 `cargo show-asm` 理解编译器输出
5. **阅读源码**：参考 std 和流行库的实现

## 参考资源

- [The Rust Programming Language](https://doc.rust-lang.org/book/)
- [Rust Reference](https://doc.rust-lang.org/reference/)
- [Rustonomicon](https://doc.rust-lang.org/nomicon/)
- [Rust API Guidelines](https://rust-lang.github.io/api-guidelines/)
