//! Merkle Patricia Trie (Simplified for Demo)
//!
//! 展示 Rust 在 Web3 区块链中的经典特性组合：
//! - Rc/RefCell 用于共享所有权和内部可变性
//! - 生命周期参数
//! - 递归数据结构
//!
//! 适用场景：以太坊状态树、IPFS

use sha3::{Digest, Keccak256};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

/// Nibbles (4-bit values)
#[derive(Debug, Clone)]
struct Nibbles(Vec<u8>);

#[allow(dead_code)] // full trie API kept for illustration
impl Nibbles {
    fn from_bytes(bytes: &[u8]) -> Self {
        let mut nibbles = Vec::with_capacity(bytes.len() * 2);
        for &b in bytes {
            nibbles.push(b >> 4);
            nibbles.push(b & 0x0F);
        }
        Self(nibbles)
    }

    fn common_prefix_len(&self, other: &Self) -> usize {
        self.0
            .iter()
            .zip(other.0.iter())
            .take_while(|(a, b)| a == b)
            .count()
    }

    fn split_at(&self, index: usize) -> (Self, Self) {
        let (left, right) = self.0.split_at(index);
        (Self(left.to_vec()), Self(right.to_vec()))
    }
}

/// 节点类型
#[derive(Debug)]
enum Node {
    Branch {
        children: [Option<Rc<RefCell<Node>>>; 16],
        value: Option<Vec<u8>>,
    },
    Leaf {
        key: Nibbles,
        value: Vec<u8>,
    },
}

impl Node {
    fn hash(&self) -> [u8; 32] {
        let encoded = format!("{:?}", self);
        Keccak256::digest(encoded.as_bytes()).into()
    }
}

/// Merkle Patricia Trie
#[derive(Debug)]
struct MerklePatriciaTrie {
    root: Option<Rc<RefCell<Node>>>,
}

impl MerklePatriciaTrie {
    fn new() -> Self {
        Self { root: None }
    }

    fn insert(&mut self, key: &[u8], value: Vec<u8>) {
        let key_nibbles = Nibbles::from_bytes(key);

        if self.root.is_none() {
            self.root = Some(Rc::new(RefCell::new(Node::Leaf {
                key: key_nibbles,
                value,
            })));
            return;
        }

        // 简化版：直接创建新根节点
        const NONE: Option<Rc<RefCell<Node>>> = None;
        let mut children: [Option<Rc<RefCell<Node>>>; 16] = [NONE; 16];

        // 保留旧的根
        children[0] = self.root.take();

        // 添加新值
        if !key_nibbles.0.is_empty() {
            let first = key_nibbles.0[0] as usize;
            children[first] = Some(Rc::new(RefCell::new(Node::Leaf {
                key: Nibbles(key_nibbles.0[1..].to_vec()),
                value,
            })));
        }

        self.root = Some(Rc::new(RefCell::new(Node::Branch {
            children,
            value: None,
        })));
    }

    fn get(&self, key: &[u8]) -> Option<Vec<u8>> {
        let key_nibbles = Nibbles::from_bytes(key);
        self.root.as_ref()?.borrow().get(&key_nibbles, 0)
    }

    fn root_hash(&self) -> Option<[u8; 32]> {
        self.root.as_ref().map(|r| r.borrow().hash())
    }
}

impl Node {
    fn get(&self, key: &Nibbles, depth: usize) -> Option<Vec<u8>> {
        match self {
            Node::Leaf { key: k, value } => {
                if &k.0 == &key.0.get(depth..).unwrap_or(&[]) {
                    Some(value.clone())
                } else {
                    None
                }
            }
            Node::Branch { children, value } => {
                if key.0.len() <= depth {
                    return value.clone();
                }

                let index = key.0[depth] as usize;
                children[index].as_ref()?.borrow().get(key, depth + 1)
            }
        }
    }
}

/// 轻量级状态数据库
struct StateDB {
    accounts: HashMap<Vec<u8>, Account>,
}

/// 账户状态
#[derive(Debug, Clone)]
#[allow(dead_code)] // Ethereum-like layout; fields used in tests / future extensions
struct Account {
    nonce: u64,
    balance: u64,
    storage_root: [u8; 32],
    code_hash: [u8; 32],
}

impl StateDB {
    fn new() -> Self {
        Self {
            accounts: HashMap::new(),
        }
    }

    fn update_account(&mut self, address: &[u8], account: Account) {
        self.accounts.insert(address.to_vec(), account);
    }

    fn get_account(&self, address: &[u8]) -> Option<Account> {
        self.accounts.get(address).cloned()
    }
}

fn main() {
    println!("=== Merkle Patricia Trie (Simplified) ===\n");

    // 创建 trie
    let mut trie = MerklePatriciaTrie::new();

    // 插入键值对
    println!("Inserting key-value pairs...");
    trie.insert(b"account_0x1234", b"balance:1000,nonce:5".to_vec());
    trie.insert(b"account_0x5678", b"balance:2000,nonce:3".to_vec());
    trie.insert(b"account_0x9abc", b"balance:500,nonce:1".to_vec());

    // 获取值
    println!("\nRetrieving values:");
    println!(
        "Key: account_0x1234, Value: {:?}",
        String::from_utf8_lossy(&trie.get(b"account_0x1234").unwrap_or_default())
    );
    println!(
        "Key: account_0x5678, Value: {:?}",
        String::from_utf8_lossy(&trie.get(b"account_0x5678").unwrap_or_default())
    );

    // 根哈希
    if let Some(root_hash) = trie.root_hash() {
        println!("\nRoot Hash: {}", hex::encode(root_hash));
    }

    // 状态数据库示例
    println!("\n=== State Database (Ethereum-like) ===\n");

    let mut db = StateDB::new();

    let addr1 = hex::decode("1234567890123456789012345678901234567890").unwrap();
    let account1 = Account {
        nonce: 5,
        balance: 1_000_000_000,
        storage_root: [0u8; 32],
        code_hash: Keccak256::digest(b"").into(),
    };

    db.update_account(&addr1, account1.clone());
    println!("Account 1: {:?}", db.get_account(&addr1));

    // 性能测试
    println!("\n=== Performance Test ===\n");

    use std::time::Instant;

    let mut perf_trie = MerklePatriciaTrie::new();
    let iterations = 1000;

    let start = Instant::now();
    for i in 0..iterations {
        let key = format!("key_{:08x}", i);
        let value = format!("value_{:08x}", i);
        perf_trie.insert(key.as_bytes(), value.into_bytes());
    }
    let insert_time = start.elapsed();

    let start = Instant::now();
    let mut hits = 0;
    for i in 0..iterations {
        let key = format!("key_{:08x}", i);
        if perf_trie.get(key.as_bytes()).is_some() {
            hits += 1;
        }
    }
    let get_time = start.elapsed();

    println!("Inserted {} items in {:?}", iterations, insert_time);
    println!(
        "Retrieved {} / {} items in {:?}",
        hits, iterations, get_time
    );
    println!(
        "Insert rate: {:.0} ops/sec",
        iterations as f64 / insert_time.as_secs_f64()
    );
    println!(
        "Get rate: {:.0} ops/sec",
        iterations as f64 / get_time.as_secs_f64()
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trie_insert_get() {
        let mut trie = MerklePatriciaTrie::new();
        trie.insert(b"test_key", b"test_value".to_vec());
        assert_eq!(trie.get(b"test_key"), Some(b"test_value".to_vec()));
    }

    #[test]
    fn test_state_db() {
        let mut db = StateDB::new();
        let addr = vec![0x12, 0x34];
        let acc = Account {
            nonce: 1,
            balance: 100,
            storage_root: [0u8; 32],
            code_hash: [0u8; 32],
        };
        db.update_account(&addr, acc.clone());
        assert_eq!(db.get_account(&addr).unwrap().nonce, 1);
    }
}
