//! Phase 0 benchmark sender.
//!
//! Pipes raw bytes from disk to a TCP socket. No framing, no handshake, no
//! VacuumDrop protocol -- measuring the un-framed ceiling is the entire point,
//! so that later phases can be measured against it.
//!
//! Spec: docs/phase1-benchmark.md

use std::error::Error;
use std::time::Instant;

use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use vacuumdrop_bench::{DEFAULT_CHUNK_SIZE, parse_size, print_usage, report};

const DEFAULT_ADDR: &str = "127.0.0.1:9000";

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let mut input: Option<String> = None;
    let mut addr = DEFAULT_ADDR.to_owned();
    let mut chunk_size = DEFAULT_CHUNK_SIZE;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--input" => input = Some(next_value(&mut args, "--input")?),
            "--addr" => addr = next_value(&mut args, "--addr")?,
            "--buffer-size" => {
                let raw = next_value(&mut args, "--buffer-size")?;
                chunk_size = parse_size(&raw)?;
            }
            "-h" | "--help" => {
                print_usage(
                    "sender",
                    "sender --input ./movie.mkv --addr 192.168.137.2:9000",
                );
                return Ok(());
            }
            other => return Err(format!("unknown argument: {other}").into()),
        }
    }

    let Some(input) = input else {
        print_usage(
            "sender",
            "sender --input ./movie.mkv --addr 192.168.137.2:9000",
        );
        return Err("--input is required".into());
    };

    let mut file = File::open(&input).await?;
    let file_size = file.metadata().await?.len();

    let mut socket = TcpStream::connect(&addr).await?;
    // Nagle would batch small writes and make the measurement a study of the
    // delayed-ACK timer rather than of the network.
    socket.set_nodelay(true)?;

    println!("sender -> {addr}");
    println!("source      : {input}");
    println!("source size : {file_size} bytes");
    println!("buffer      : {chunk_size} bytes");
    println!();

    // Allocated once, reused for every read/write cycle. A fresh allocation per
    // iteration would measure the allocator instead of the network.
    let mut buffer = vec![0u8; chunk_size];

    let start = Instant::now();
    let mut sent: u64 = 0;

    loop {
        let read = file.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        socket.write_all(&buffer[..read]).await?;
        sent += read as u64;
    }

    // Explicit shutdown so the receiver observes EOF and stops its own clock.
    // Dropping the socket would also work, but shutdown makes the intent
    // explicit and guards against a future write-buffering change.
    socket.shutdown().await?;

    report("sender result", sent, start.elapsed());
    Ok(())
}

fn next_value(
    args: &mut impl Iterator<Item = String>,
    flag: &str,
) -> Result<String, Box<dyn Error>> {
    args.next()
        .ok_or_else(|| format!("{flag} requires a value").into())
}
