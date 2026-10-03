//! Phase 0 benchmark: shared helpers for `sender` and `receiver`.
//!
//! Deliberately tiny: this crate exists to measure a raw TCP ceiling, so every
//! line here that is not strictly necessary to that measurement is a liability.
//!
//! Spec: docs/phase1-benchmark.md

/// The hardcoded experiment buffer from Doc 2 section 2.
///
/// Phase 0 fixes this at 8 MiB. It is an *experiment*, not a permanent
/// requirement -- Charter 22 requires benchmarking 256 KiB .. 32 MiB and
/// choosing from data. `--buffer-size` exists so the sweep needs no code edits.
pub const DEFAULT_CHUNK_SIZE: usize = 8 * 1024 * 1024;

/// Bytes in one decimal megabyte (the unit network throughput is reported in).
pub const BYTES_PER_MB: u64 = 1_000_000;

/// Bytes in one binary mebibyte.
pub const BYTES_PER_MIB: u64 = 1024 * 1024;

/// Parses a byte count.
///
/// Accepts a bare integer (`8388608`) or a suffixed value (`8MiB`, `8MB`,
/// `512KiB`, `1GB`). Suffixes are binary except `MB`/`GB`, which are decimal
/// to match how throughput is conventionally reported.
pub fn parse_size(raw: &str) -> Result<usize, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("size is empty".to_owned());
    }

    let split = trimmed
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(trimmed.len());
    let (digits, suffix) = trimmed.split_at(split);

    let value: u64 = digits
        .parse()
        .map_err(|_| format!("`{digits}` is not a valid byte count"))?;

    let multiplier: u64 = match suffix.trim().to_ascii_lowercase().as_str() {
        "" | "b" => 1,
        "k" | "kb" => 1_000,
        "kib" => 1 << 10,
        "m" | "mb" => 1_000_000,
        "mib" => 1 << 20,
        "g" | "gb" => 1_000_000_000,
        "gib" => 1 << 30,
        other => return Err(format!("unknown size suffix `{other}`")),
    };

    value
        .checked_mul(multiplier)
        .map(|bytes| bytes as usize)
        .ok_or_else(|| format!("`{trimmed}` overflows a usize"))
}

/// Prints one consistent result block for either side of the transfer.
///
/// Both binaries report the same shape so the two lines can be compared
/// directly -- the delta between them is socket cost, which is exactly what the
/// baseline needs to expose.
pub fn report(label: &str, bytes: u64, elapsed: std::time::Duration) {
    let secs = elapsed.as_secs_f64();
    let mb_per_sec = if secs > 0.0 {
        bytes as f64 / BYTES_PER_MB as f64 / secs
    } else {
        f64::INFINITY
    };
    let mib_per_sec = if secs > 0.0 {
        bytes as f64 / BYTES_PER_MIB as f64 / secs
    } else {
        f64::INFINITY
    };

    println!("{label}");
    println!("  bytes      : {bytes}");
    println!("  elapsed    : {secs:.3} s");
    println!("  MB/s       : {mb_per_sec:.2}");
    println!("  MiB/s      : {mib_per_sec:.2}");
}

/// Prints usage text for `prog`.
pub fn print_usage(prog: &str, example: &str) {
    println!("usage: {prog} [options]");
    println!();
    println!("  --input <path>       source file to send");
    println!("  --output <path>      destination file to write");
    println!("  --addr <ip:port>     address to connect to / listen on");
    println!("  --buffer-size <n>    buffer size, e.g. 8MiB (default {DEFAULT_CHUNK_SIZE} bytes)");
    println!("  -h, --help           show this help");
    println!();
    println!("example: {example}");
}

#[cfg(test)]
mod tests {
    use super::{BYTES_PER_MB, BYTES_PER_MIB, parse_size};

    #[test]
    fn parses_bare_integers() {
        assert_eq!(parse_size("0").unwrap(), 0);
        assert_eq!(parse_size("1").unwrap(), 1);
        assert_eq!(parse_size("8388608").unwrap(), 8 * 1024 * 1024);
    }

    #[test]
    fn parses_binary_suffixes() {
        assert_eq!(parse_size("8MiB").unwrap(), 8 * 1024 * 1024);
        assert_eq!(parse_size("1KiB").unwrap(), 1024);
        assert_eq!(parse_size("2GiB").unwrap(), 2 * 1024 * 1024 * 1024);
    }

    #[test]
    fn parses_decimal_suffixes() {
        // MB/GB are decimal to match the unit throughput is reported in.
        assert_eq!(parse_size("1MB").unwrap(), BYTES_PER_MB as usize);
        assert_eq!(parse_size("1GB").unwrap(), 1_000_000_000);
        assert_eq!(parse_size("8MB").unwrap(), 8 * BYTES_PER_MB as usize);
    }

    #[test]
    fn suffix_matching_is_case_insensitive() {
        assert_eq!(parse_size("8mib").unwrap(), 8 * 1024 * 1024);
        assert_eq!(parse_size("8MiB").unwrap(), 8 * 1024 * 1024);
        assert_eq!(parse_size("8MIB").unwrap(), 8 * 1024 * 1024);
    }

    #[test]
    fn tolerates_surrounding_whitespace() {
        assert_eq!(parse_size("  8MiB  ").unwrap(), 8 * 1024 * 1024);
    }

    #[test]
    fn rejects_empty_input() {
        assert!(parse_size("").is_err());
        assert!(parse_size("   ").is_err());
    }

    #[test]
    fn rejects_unknown_suffixes() {
        assert!(parse_size("8PB").is_err());
        assert!(parse_size("8ZiB").is_err());
        assert!(parse_size("8MiBs").is_err());
    }

    #[test]
    fn rejects_non_numeric_input() {
        assert!(parse_size("MiB").is_err());
        assert!(parse_size("abc").is_err());
    }

    #[test]
    fn rejects_overflow_rather_than_wrapping() {
        // Wrapping to a small buffer would silently corrupt a transfer.
        assert!(parse_size("99999999999999999999GiB").is_err());
    }

    #[test]
    fn byte_units_are_distinct() {
        assert_ne!(BYTES_PER_MB, BYTES_PER_MIB);
        assert_eq!(BYTES_PER_MB, 1_000_000);
        assert_eq!(BYTES_PER_MIB, 1_048_576);
    }
}
