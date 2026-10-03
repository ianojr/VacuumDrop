//! Phase 0 benchmark receiver.
//!
//! Listens on a TCP socket, writes the incoming byte stream to disk, and reports
//! throughput and elapsed time on completion. No framing, no handshake, no
//! VacuumDrop protocol.
//!
//! Spec: docs/phase1-benchmark.md

use std::error::Error;
use std::time::Instant;

use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use vacuumdrop_bench::{DEFAULT_CHUNK_SIZE, parse_size, print_usage, report};

const DEFAULT_ADDR: &str = "0.0.0.0:9000";
const DEFAULT_OUTPUT: &str = "receiver-out.bin";

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let mut output = DEFAULT_OUTPUT.to_owned();
    let mut addr = DEFAULT_ADDR.to_owned();
    let mut chunk_size = DEFAULT_CHUNK_SIZE;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--output" => output = next_value(&mut args, "--output")?,
            "--addr" => addr = next_value(&mut args, "--addr")?,
            "--buffer-size" => {
                let raw = next_value(&mut args, "--buffer-size")?;
                chunk_size = parse_size(&raw)?;
            }
            "-h" | "--help" => {
                print_usage(
                    "receiver",
                    "receiver --addr 0.0.0.0:9000 --output ./received.mkv",
                );
                return Ok(());
            }
            other => return Err(format!("unknown argument: {other}").into()),
        }
    }

    let listener = TcpListener::bind(&addr).await?;
    let local = listener.local_addr()?;

    println!("receiver listening on {local}");
    println!("output      : {output}");
    println!("buffer      : {chunk_size} bytes");
    println!("waiting for sender...");
    println!();

    let (mut socket, peer) = listener.accept().await?;
    socket.set_nodelay(true)?;
    println!("peer connected: {peer}");

    let mut file = File::create(&output).await?;

    // Allocated once, reused for every read/write cycle.
    let mut buffer = vec![0u8; chunk_size];

    let start = Instant::now();
    let mut received: u64 = 0;

    loop {
        let read = socket.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        file.write_all(&buffer[..read]).await?;
        received += read as u64;
    }

    file.flush().await?;
    // Stop the clock only once the bytes are on the device. Without this the
    // measurement reports page-cache write speed, not disk write speed, which
    // would flatter the result and mislead every later phase.
    file.sync_all().await?;
    drop(file);

    let elapsed = start.elapsed();
    let written = tokio::fs::metadata(&output).await?.len();

    report("receiver result", received, elapsed);
    println!("  on disk    : {written} bytes");
    println!();
    if written == received {
        println!(
            "byte count matches received count (no framing in Phase 0; content is NOT verified)"
        );
    } else {
        println!("WARNING: on-disk size {written} != received {received}");
    }

    Ok(())
}

fn next_value(
    args: &mut impl Iterator<Item = String>,
    flag: &str,
) -> Result<String, Box<dyn Error>> {
    args.next()
        .ok_or_else(|| format!("{flag} requires a value").into())
}
