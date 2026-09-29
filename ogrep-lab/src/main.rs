//! ogrep-lab: a local workbench for an ogrep index.
//!
//! Serves a web page on the loopback interface for inspecting the index,
//! explaining query plans, searching, navigating files and the file graph,
//! reading history, and running evaluations. Everything it shows comes from
//! the index on disk or from the `ogrep` binary itself; nothing leaves the
//! machine and nothing is fetched.
//!
//! It is a separate binary so that none of this costs `ogrep` anything: no
//! larger executable, no extra start-up work on every search.

mod api;
mod cli;
mod evals;
mod files;
mod http;
mod overview;
mod plan;
mod search;
#[cfg(test)]
mod tests;
mod workspace;

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::Parser;

#[derive(Parser)]
#[command(
    name = "ogrep-lab",
    version,
    about = "A local workbench for an ogrep index: inspect it, explain query plans, search, \
             navigate the file graph and history, and run evaluations",
    after_help = "The page is served on 127.0.0.1 only, behind a random token in the printed URL."
)]
struct Args {
    /// Repository to open; its index is the nearest .ogrep at or above it.
    #[arg(default_value = ".")]
    path: PathBuf,

    /// Use this index directory instead.
    #[arg(long, value_name = "DIR")]
    index_path: Option<PathBuf>,

    /// Port to listen on (0 picks a free one).
    #[arg(long, default_value_t = 0)]
    port: u16,

    /// Print the URL without opening a browser.
    #[arg(long)]
    no_browser: bool,

    /// The ogrep binary to run searches with (default: next to ogrep-lab, else on PATH).
    #[arg(long, value_name = "BIN")]
    ogrep: Option<PathBuf>,
}

fn main() {
    if let Err(e) = run() {
        eprintln!("ogrep-lab: {e:#}");
        std::process::exit(2);
    }
}

fn run() -> Result<()> {
    let args = Args::parse();
    let ws = workspace::Workspace::open(&args.path, args.index_path.as_deref(), args.ogrep)?;
    let server = http::Server::bind(args.port)
        .with_context(|| format!("cannot listen on port {}", args.port))?;
    let url = server.url();

    println!("ogrep-lab {}", env!("CARGO_PKG_VERSION"));
    println!("  repository  {}", ws.root.display());
    println!("  index       {}", ws.index_dir.display());
    match (&ws.ogrep, &ws.ogrep_version) {
        (Some(bin), Some(v)) => println!("  ogrep       {} ({v})", bin.display()),
        (Some(bin), None) => println!("  ogrep       {}", bin.display()),
        (None, _) => {
            println!("  ogrep       not found; search and evaluations are off (pass --ogrep)")
        }
    }
    println!();
    println!("  {url}");
    println!();
    println!("Ctrl-C to stop.");
    if !args.no_browser {
        open_browser(&url);
    }

    let app = Arc::new(api::App {
        ws: Arc::new(ws),
        jobs: evals::Jobs::default(),
    });
    server.run(Arc::new(move |req| app.handle(req)));
    Ok(())
}

/// Best effort: `$BROWSER`, then the platform's opener. Under WSL the Windows
/// browser is reached through `wslview` or `cmd.exe`. Failure is silent, since
/// the URL is printed anyway.
fn open_browser(url: &str) {
    use std::process::{Command, Stdio};
    let try_run = |prog: &str, args: &[&str]| {
        Command::new(prog)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .is_ok()
    };
    if let Ok(b) = std::env::var("BROWSER")
        && !b.is_empty()
        && try_run(&b, &[url])
    {
        return;
    }
    if cfg!(target_os = "macos") {
        try_run("open", &[url]);
    } else if cfg!(windows) {
        try_run("cmd", &["/C", "start", "", url]);
    } else {
        let wsl = std::fs::read_to_string("/proc/version")
            .is_ok_and(|v| v.to_lowercase().contains("microsoft"));
        if wsl
            && (try_run("wslview", &[url])
                || try_run("cmd.exe", &["/C", "start", "", &url.replace('&', "^&")]))
        {
            return;
        }
        try_run("xdg-open", &[url]);
    }
}
