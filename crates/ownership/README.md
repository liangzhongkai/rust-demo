# Ownership (所有权)

> 所有权是 Rust 的核心特性，它让 Rust 无需垃圾回收器就能保证内存安全

## 核心规则

1. **每个值都有一个所有者** (Each value has an owner)
2. **同一时间只能有一个所有者** (A value can have only one owner at a time)
3. **所有者离开作用域时值被丢弃** (When the owner goes out of scope, the value is dropped)

## 学习目标

- 理解 Move 语义的本质
- 掌握 Copy vs Clone 的区别
- 理解 Drop trait 和析构函数
- 学会避免常见的所有权陷阱
- 理解栈上和堆上数据的所有权差异

## 文件说明

| 文件 | 内容 |
|------|------|
| `main.rs` | 运行所有示例的入口 |
| `basics.rs` | 基础概念：Move、Copy、作用域 |
| `advanced.rs` | 进阶：部分移动、所有权转移优化 |
| `pitfalls.rs` | 常见陷阱和错误诊断 |
| `real_world.rs` | 实际应用场景 |

## 关键概念

### Move 语义

```rust
let s1 = String::from("hello");
let s2 = s1;  // s1 的所有权移动到 s2
// println!("{}", s1);  // 错误！s1 不再有效
```

### Copy trait

实现了 Copy 的类型在赋值时会自动复制：
- 基本类型：i32, f64, bool, char
- 元组（如果成员都是 Copy）
- 不可变引用

### Drop trait

类型离开作用域时自动调用：
```rust
impl Drop for MyType {
    fn drop(&mut self) {
        // 清理资源
    }
}
```

## 运行示例

```bash
cargo run -p ownership
```

## 思考问题

1. 为什么 String 没有 Copy trait？
2. Move 语义是如何保证内存安全的？
3. 如何设计 API 来避免不必要的所有权转移？
