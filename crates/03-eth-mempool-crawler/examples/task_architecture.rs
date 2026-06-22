//! Demonstrates the async task + MPSC channel architecture from the article.
//!
//! ```text
//! Producer ──► Handler ──► Processor ──► Consumer
//! ```

use anyhow::Result;
use tokio::sync::mpsc;
use tokio::time::{Duration, sleep};

#[derive(Debug, Clone)]
struct RawEvent {
    id: u64,
    payload: String,
}

#[derive(Debug, Clone)]
struct ProcessedEvent {
    id: u64,
    summary: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let (raw_tx, raw_rx) = mpsc::channel::<RawEvent>(32);
    let (processed_tx, mut processed_rx) = mpsc::channel::<ProcessedEvent>(32);

    // Task 1: NetworkManager analogue — produces raw events
    let producer = tokio::spawn(async move {
        for i in 0..5 {
            let event = RawEvent {
                id: i,
                payload: format!("NetworkTransactionEvent #{i}"),
            };
            println!("[NetworkManager] emitting event #{i}");
            raw_tx.send(event).await.ok();
            sleep(Duration::from_millis(300)).await;
        }
    });

    // Task 2: Tx Event Handler — decodes and forwards
    let handler = tokio::spawn(async move {
        let mut rx = raw_rx;
        while let Some(event) = rx.recv().await {
            println!("[TxHandler] handling event #{}", event.id);
            let processed = ProcessedEvent {
                id: event.id,
                summary: format!("decoded: {}", event.payload),
            };
            processed_tx.send(processed).await.ok();
        }
    });

    // Task 3: Consumer — display (UI task analogue)
    let consumer = tokio::spawn(async move {
        while let Some(event) = processed_rx.recv().await {
            println!("[UI] displaying: {:?}", event);
        }
    });

    producer.await?;
    handler.await?;
    consumer.await?;

    println!("\nAll tasks completed — this mirrors the crawler's decoupled architecture.");
    Ok(())
}
