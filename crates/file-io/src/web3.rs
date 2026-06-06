//! # Web3 / 区块链生产场景下的文件 I/O
//!
//! Web3 的文件负载特征：
//! - **追加型日志**：indexer 按 block 顺序写 event，reorg 要能回滚
//! - **大快照**：链状态 checkpoint 动辄 GB，需要 mmap / 分片
//! - **配置多链**：RPC URL、合约地址热更新，原子替换
//! - **产物缓存**：ABI/IPFS manifest 校验后本地缓存
//!
//! 下面 6 个场景对应 indexer、归档节点、bot、钱包后端里的常见写法。

#![allow(dead_code)]

use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

pub type BlockNum = u64;
pub type LogIndex = u32;

// ============================================================================
// 场景 1：indexer 追加型 event 日志
// ============================================================================
/// **生产问题**：每秒数千条 `eth_getLogs` 结果要写盘；重启后从上次 block 续扫。
///
/// **套路**：`(block, log_index)` 主键去重；sidecar cursor 文件记录进度。
pub mod indexer_event_log {
    use super::*;

    #[derive(Debug, Clone, PartialEq, Eq, Hash)]
    pub struct EventKey {
        pub block: BlockNum,
        pub log_index: LogIndex,
    }

    pub struct EventLog {
        path: PathBuf,
        seen: HashSet<EventKey>,
        pub written: u64,
    }

    impl EventLog {
        pub fn open(path: PathBuf) -> io::Result<Self> {
            let mut log = Self {
                path,
                seen: HashSet::new(),
                written: 0,
            };
            log.rebuild_index()?;
            Ok(log)
        }

        fn rebuild_index(&mut self) -> io::Result<()> {
            let content = fs::read_to_string(&self.path).unwrap_or_default();
            for line in content.lines() {
                let mut it = line.split(',');
                let block: BlockNum = it.next().unwrap().parse().unwrap();
                let idx: LogIndex = it.next().unwrap().parse().unwrap();
                self.seen.insert(EventKey { block, log_index: idx });
            }
            Ok(())
        }

        pub fn append(&mut self, key: EventKey, topic: &str) -> io::Result<bool> {
            if !self.seen.insert(key.clone()) {
                return Ok(false);
            }
            let mut f = OpenOptions::new().create(true).append(true).open(&self.path)?;
            writeln!(f, "{},{},{}", key.block, key.log_index, topic)?;
            self.written += 1;
            Ok(true)
        }
    }

    pub fn save_cursor(path: &Path, block: BlockNum) -> io::Result<()> {
        let tmp = path.with_extension("tmp");
        fs::write(&tmp, block.to_string())?;
        File::open(&tmp)?.sync_all()?;
        fs::rename(tmp, path)?;
        Ok(())
    }

    pub fn demonstrate() {
        println!("## 场景 1：indexer event 追加日志");
        let dir = std::env::temp_dir().join("file-io-web3-indexer");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let mut log = EventLog::open(dir.join("events.log")).unwrap();
        let k = EventKey { block: 100, log_index: 0 };
        log.append(k.clone(), "Transfer").unwrap();
        log.append(k, "Transfer").unwrap(); // 重复
        save_cursor(&dir.join("cursor"), 100).unwrap();
        println!("去重后写入 {} 条", log.written);
        println!("关键：幂等主键；cursor 原子写；大批量用 BufWriter + 定期 fsync\n");
    }
}

// ============================================================================
// 场景 2：合约 ABI 产物加载与校验缓存
// ============================================================================
/// **生产问题**：同一合约 ABI 被多个服务重复拉取；文件损坏会导致 decode 静默错误。
///
/// **套路**：`{address}.json` + sha256 sidecar；校验失败拒绝加载。
pub mod abi_artifact_cache {
    use super::*;
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    fn checksum(data: &str) -> u64 {
        let mut h = DefaultHasher::new();
        data.hash(&mut h);
        h.finish()
    }

    pub struct AbiCache {
        dir: PathBuf,
    }

    impl AbiCache {
        pub fn new(dir: PathBuf) -> Self {
            Self { dir }
        }

        pub fn store(&self, address: &str, abi_json: &str) -> io::Result<()> {
            fs::create_dir_all(&self.dir)?;
            let path = self.dir.join(format!("{address}.json"));
            let cs_path = self.dir.join(format!("{address}.sha256"));
            fs::write(&path, abi_json)?;
            fs::write(cs_path, checksum(abi_json).to_string())?;
            Ok(())
        }

        pub fn load(&self, address: &str) -> io::Result<String> {
            let path = self.dir.join(format!("{address}.json"));
            let cs_path = self.dir.join(format!("{address}.sha256"));
            let data = fs::read_to_string(&path)?;
            let expected: u64 = fs::read_to_string(cs_path)?.trim().parse().unwrap();
            if checksum(&data) != expected {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "checksum mismatch"));
            }
            Ok(data)
        }
    }

    pub fn demonstrate() {
        println!("## 场景 2：ABI 产物缓存");
        let dir = std::env::temp_dir().join("file-io-web3-abi");
        let _ = fs::remove_dir_all(&dir);
        let cache = AbiCache::new(dir);
        let abi = r#"[{"type":"function","name":"transfer"}]"#;
        cache.store("0xabc", abi).unwrap();
        let loaded = cache.load("0xabc").unwrap();
        println!("加载 ABI 长度 = {} 字节", loaded.len());
        println!("关键：链上 address 做 key；升级后 bump 版本目录；CI 预拉产物\n");
    }
}

// ============================================================================
// 场景 3：链状态快照 checkpoint
// ============================================================================
/// **生产问题**：全量 sync 要几天；必须在 block N 快照后增量追块。
///
/// **套路**：快照文件头写 `(block_hash, state_root, offset)`；恢复时校验再 mmap 数据区。
pub mod chain_snapshot {
    use super::*;

    #[derive(Debug, Clone)]
    pub struct SnapshotHeader {
        pub block: BlockNum,
        pub state_root: [u8; 32],
        pub data_offset: u64,
    }

    pub fn write_snapshot(path: &Path, header: &SnapshotHeader, data: &[u8]) -> io::Result<()> {
        let tmp = path.with_extension("tmp");
        let mut f = File::create(&tmp)?;
        f.write_all(&header.block.to_le_bytes())?;
        f.write_all(&header.state_root)?;
        f.write_all(&header.data_offset.to_le_bytes())?;
        f.write_all(data)?;
        f.sync_all()?;
        fs::rename(tmp, path)?;
        Ok(())
    }

    pub fn read_header(path: &Path) -> io::Result<SnapshotHeader> {
        let mut f = File::open(path)?;
        let mut buf = [0u8; 8 + 32 + 8];
        f.read_exact(&mut buf)?;
        Ok(SnapshotHeader {
            block: u64::from_le_bytes(buf[0..8].try_into().unwrap()),
            state_root: buf[8..40].try_into().unwrap(),
            data_offset: u64::from_le_bytes(buf[40..48].try_into().unwrap()),
        })
    }

    pub fn demonstrate() {
        println!("## 场景 3：链状态快照");
        let dir = std::env::temp_dir().join("file-io-web3-snapshot");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("snap.bin");
        let h = SnapshotHeader {
            block: 19_000_000,
            state_root: [0xAB; 32],
            data_offset: 48,
        };
        write_snapshot(&path, &h, b"trie_nodes_blob").unwrap();
        let loaded = read_header(&path).unwrap();
        println!("快照 block={} root[0]={:#02x}", loaded.block, loaded.state_root[0]);
        println!("关键：头固定长便于校验；数据区可压缩；恢复前先 verify hash\n");
    }
}

// ============================================================================
// 场景 4：IPFS pin manifest 版本化
// ============================================================================
/// **生产问题**：NFT metadata / 合约源码分散 pin，需要可审计的 manifest。
///
/// **套路**：`manifest.json` 列 CID + 版本；发布走原子替换。
pub mod ipfs_pin_manifest {
    use super::*;

    #[derive(Debug)]
    pub struct PinEntry {
        pub cid: String,
        pub label: String,
    }

    pub struct Manifest {
        pub version: u64,
        pub pins: Vec<PinEntry>,
    }

    impl Manifest {
        pub fn serialize(&self) -> String {
            let body: Vec<String> = self
                .pins
                .iter()
                .map(|p| format!("{}:{}", p.cid, p.label))
                .collect();
            format!("v{}\n{}", self.version, body.join("\n"))
        }

        pub fn parse(text: &str) -> Option<Self> {
            let mut lines = text.lines();
            let ver_line = lines.next()?;
            let version: u64 = ver_line.strip_prefix('v')?.parse().ok()?;
            let pins = lines
                .filter_map(|l| {
                    let (cid, label) = l.split_once(':')?;
                    Some(PinEntry {
                        cid: cid.to_string(),
                        label: label.to_string(),
                    })
                })
                .collect();
            Some(Self { version, pins })
        }
    }

    pub fn publish(dir: &Path, m: &Manifest) -> io::Result<()> {
        fs::create_dir_all(dir)?;
        let final_path = dir.join("manifest.txt");
        let tmp = dir.join("manifest.txt.tmp");
        fs::write(&tmp, m.serialize())?;
        File::open(&tmp)?.sync_all()?;
        fs::rename(tmp, final_path)?;
        Ok(())
    }

    pub fn demonstrate() {
        println!("## 场景 4：IPFS pin manifest");
        let dir = std::env::temp_dir().join("file-io-web3-ipfs");
        let _ = fs::remove_dir_all(&dir);
        let m = Manifest {
            version: 3,
            pins: vec![
                PinEntry {
                    cid: "Qmabc".into(),
                    label: "metadata".into(),
                },
                PinEntry {
                    cid: "Qmdef".into(),
                    label: "image".into(),
                },
            ],
        };
        publish(&dir, &m).unwrap();
        let text = fs::read_to_string(dir.join("manifest.txt")).unwrap();
        let loaded = Manifest::parse(&text).unwrap();
        println!("manifest v{} pins={}", loaded.version, loaded.pins.len());
        println!("关键：manifest 是唯一真相；pin 任务读 manifest 再拉 CID\n");
    }
}

// ============================================================================
// 场景 5：多链 RPC 配置原子热更新
// ============================================================================
/// **生产问题**：公共 RPC 限流/宕机；配置要不停机切换；不能读到半写 endpoints。
///
/// **套路**：`chains.toml` 原子替换 + 每链多 endpoint 健康分排序。
pub mod multichain_config {
    use super::*;

    #[derive(Debug, Clone)]
    pub struct ChainEndpoint {
        pub url: String,
        pub weight: u32,
    }

    #[derive(Debug, Clone)]
    pub struct ChainConfig {
        pub chain_id: u64,
        pub endpoints: Vec<ChainEndpoint>,
    }

    pub struct ConfigStore {
        path: PathBuf,
        pub chains: HashMap<u64, ChainConfig>,
    }

    impl ConfigStore {
        pub fn load(path: PathBuf) -> io::Result<Self> {
            let text = fs::read_to_string(&path)?;
            let mut chains = HashMap::new();
            for line in text.lines() {
                let mut it = line.split(',');
                let chain_id: u64 = it.next().unwrap().parse().unwrap();
                let url = it.next().unwrap().to_string();
                let weight: u32 = it.next().unwrap().parse().unwrap();
                chains
                    .entry(chain_id)
                    .or_insert(ChainConfig {
                        chain_id,
                        endpoints: Vec::new(),
                    })
                    .endpoints
                    .push(ChainEndpoint { url, weight });
            }
            for c in chains.values_mut() {
                c.endpoints.sort_by(|a, b| b.weight.cmp(&a.weight));
            }
            Ok(Self { path, chains })
        }

        pub fn best_endpoint(&self, chain_id: u64) -> Option<&str> {
            self.chains
                .get(&chain_id)
                .and_then(|c| c.endpoints.first())
                .map(|e| e.url.as_str())
        }

        pub fn publish(&self, content: &str) -> io::Result<()> {
            let tmp = self.path.with_extension("tmp");
            fs::write(&tmp, content)?;
            File::open(&tmp)?.sync_all()?;
            fs::rename(tmp, &self.path)?;
            Ok(())
        }
    }

    pub fn demonstrate() {
        println!("## 场景 5：多链 RPC 配置");
        let dir = std::env::temp_dir().join("file-io-web3-rpc");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("chains.csv");
        fs::write(
            &path,
            "1,https://eth-a.example,10\n1,https://eth-b.example,5\n137,https://polygon.example,8\n",
        )
        .unwrap();
        let store = ConfigStore::load(path.clone()).unwrap();
        println!("ETH 首选 = {}", store.best_endpoint(1).unwrap());
        store
            .publish("1,https://eth-c.example,20\n1,https://eth-a.example,10\n")
            .unwrap();
        let updated = ConfigStore::load(path).unwrap();
        println!("热更新后首选 = {}", updated.best_endpoint(1).unwrap());
        println!("关键：原子 publish；health probe 调 weight；失败降级不阻塞读配置\n");
    }
}

// ============================================================================
// 场景 6：reorg 安全的事件游标
// ============================================================================
/// **生产问题**：链重组后已写盘的 block N 事件作废，indexer 必须回滚并重扫。
///
/// **套路**：`head.json` 记录 canonical tip；reorg 时 truncate log 到安全 block。
pub mod reorg_safe_cursor {
    use super::*;

    #[derive(Debug, Clone)]
    pub struct ChainHead {
        pub block: BlockNum,
        pub hash: String,
    }

    pub struct ReorgHandler {
        log_path: PathBuf,
        head_path: PathBuf,
        pub lines: Vec<String>,
    }

    impl ReorgHandler {
        pub fn new(dir: PathBuf) -> Self {
            Self {
                log_path: dir.join("events.log"),
                head_path: dir.join("head.json"),
                lines: Vec::new(),
            }
        }

        pub fn ingest(&mut self, block: BlockNum, line: &str) -> io::Result<()> {
            self.lines.push(format!("{block},{line}"));
            let mut f = OpenOptions::new().create(true).append(true).open(&self.log_path)?;
            writeln!(f, "{block},{line}")?;
            self.save_head(ChainHead {
                block,
                hash: format!("0x{block:064x}"),
            })
        }

        fn save_head(&self, head: ChainHead) -> io::Result<()> {
            let tmp = self.head_path.with_extension("tmp");
            fs::write(&tmp, format!("{},{}", head.block, head.hash))?;
            fs::rename(tmp, &self.head_path)?;
            Ok(())
        }

        pub fn on_reorg(&mut self, safe_block: BlockNum) -> usize {
            let before = self.lines.len();
            self.lines.retain(|l| {
                let b: BlockNum = l.split(',').next().unwrap().parse().unwrap();
                b <= safe_block
            });
            let truncated = self.lines.join("\n");
            fs::write(&self.log_path, truncated).unwrap();
            before - self.lines.len()
        }
    }

    pub fn demonstrate() {
        println!("## 场景 6：reorg 安全游标");
        let dir = std::env::temp_dir().join("file-io-web3-reorg");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let mut h = ReorgHandler::new(dir);
        h.ingest(100, "ev1").unwrap();
        h.ingest(101, "ev2").unwrap();
        h.ingest(102, "ev3").unwrap();
        let rolled = h.on_reorg(100);
        println!("reorg 到 block 100，回滚 {} 条", rolled);
        println!("关键：深度 k 确认后再 finalize；log truncate + 重扫；metric 记 reorg 次数\n");
    }
}

pub fn demonstrate() {
    indexer_event_log::demonstrate();
    abi_artifact_cache::demonstrate();
    chain_snapshot::demonstrate();
    ipfs_pin_manifest::demonstrate();
    multichain_config::demonstrate();
    reorg_safe_cursor::demonstrate();
}
