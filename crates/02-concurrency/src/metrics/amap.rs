use anyhow::Result;
use std::{
    collections::HashMap,
    fmt,
    sync::{
        Arc,
        atomic::{AtomicI64, Ordering},
    },
};

#[derive(Debug)]
pub struct AmapMetrics {
    data: Arc<HashMap<&'static str, AtomicI64>>,
}
