//! A refresh receipt is never replaced with a local scan or a readiness flag.
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail, ensure};

pub fn run(root: &Path, index_path: Option<&Path>, timeout: Duration, json: bool) -> Result<()> {
    let deadline = Instant::now() + timeout;
    let root = std::fs::canonicalize(root).context("cannot resolve refresh root")?;
    ensure!(root.is_dir(), "refresh root must be a directory");
    let root_text = root
        .to_str()
        .context("refresh requires a UTF-8 repository root")?;
    let index = index_path
        .map(Path::to_path_buf)
        .unwrap_or_else(|| zgrep_core::builder::default_index_dir(&root));
    let info = crate::serve::ServerInfo::load(&index)
        .context("refresh requires a running server; start zgrep-code serve for this root")?;
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, info.port));
    let mut stream =
        TcpStream::connect_timeout(&address, remaining(deadline)?.min(Duration::from_secs(2)))
            .context("refresh server is unavailable; no freshness receipt was obtained")?;
    stream.set_write_timeout(Some(remaining(deadline)?))?;
    let request = serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "refresh",
        "params": {"root": root_text, "timeout_ms": remaining(deadline)?.as_millis().max(1)},
    });
    writeln!(stream, "{request}").context("could not send refresh request")?;
    let response = read_response(&mut stream, deadline)?;
    if let Some(error) = response.get("error") {
        bail!(
            "refresh failed ({}): {}; no freshness receipt was obtained",
            error["code"],
            error["message"].as_str().unwrap_or("invalid server error")
        );
    }
    ensure!(
        response["jsonrpc"] == "2.0" && response["id"] == 1,
        "invalid refresh response envelope"
    );
    let result = &response["result"];
    ensure!(
        result["status"] == "refreshed" && result["root"] == root_text,
        "server did not return a refresh receipt for this root"
    );
    ensure!(
        result["elapsed_ms"].as_u64().is_some() && result["num_files"].as_u64().is_some(),
        "invalid refresh receipt"
    );
    if json {
        println!("{result}");
    } else {
        println!(
            "Refreshed {} ({} content files, {} ms)",
            root.display(),
            result["num_files"],
            result["elapsed_ms"]
        );
    }
    Ok(())
}

fn remaining(deadline: Instant) -> Result<Duration> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    ensure!(
        !remaining.is_zero(),
        "refresh wait timed out; server work may continue; retry before relying on indexed freshness"
    );
    Ok(remaining)
}

fn read_response(stream: &mut TcpStream, deadline: Instant) -> Result<serde_json::Value> {
    const MAX_RECEIPT_BYTES: usize = 64 * 1024;
    let mut response = Vec::new();
    let mut buffer = [0; 4096];
    loop {
        // Reset to the remaining total budget on each read, so a trickling
        // peer cannot extend the client deadline with periodic partial data.
        stream.set_read_timeout(Some(remaining(deadline)?))?;
        let read = stream.read(&mut buffer)
            .context("refresh response unavailable or timed out; server work may continue; no freshness receipt was obtained")?;
        ensure!(read != 0, "server disconnected before the refresh receipt");
        let end = buffer[..read].iter().position(|byte| *byte == b'\n');
        response.extend_from_slice(&buffer[..end.unwrap_or(read)]);
        ensure!(
            response.len() <= MAX_RECEIPT_BYTES,
            "refresh response exceeded 64 KiB"
        );
        remaining(deadline)?;
        if end.is_some() {
            return serde_json::from_slice(&response).context("invalid refresh response JSON");
        }
    }
}
