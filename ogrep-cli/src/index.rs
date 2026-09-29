/// `ogrep index` — build the trigram index.
use std::path::Path;
use std::time::Instant;

use anyhow::{Context, Result};
use ogrep_core::builder::{self, BuildOptions, IndexStrategy};

use crate::mem;

/// Convert `--index-buffer <MB>` to bytes.
///
/// Rejects values this platform cannot represent instead of quietly falling
/// back to the default: silently indexing with a 64 MB arena after the user
/// asked for something else would misreport what actually ran.
fn buffer_bytes_from_mb(mb: u64) -> Result<usize> {
    let max_mb = (usize::MAX / (1024 * 1024)) as u64;
    mb.checked_mul(1024 * 1024)
        .and_then(|bytes| usize::try_from(bytes).ok())
        .with_context(|| {
            format!("--index-buffer {mb} is too large for this platform (maximum {max_mb} MB)")
        })
}

/// Everything `ogrep index` needs to build an index.
///
/// A struct rather than a long parameter list: the arguments are all plain
/// scalars, so positional calls are easy to transpose without the compiler
/// noticing.
pub struct RunOptions<'a> {
    pub root: &'a Path,
    pub index_path: Option<&'a Path>,
    pub include_hidden: bool,
    pub no_ignore: bool,
    pub no_require_git: bool,
    pub exclude_dirs: &'a [String],
    pub strategy: IndexStrategy,
    pub index_buffer_mb: Option<u64>,
    pub max_file_size: Option<u64>,
    pub sparse_grams: bool,
}

pub fn run(opts: RunOptions<'_>) -> Result<()> {
    let RunOptions {
        root,
        index_path,
        include_hidden,
        no_ignore,
        no_require_git,
        exclude_dirs,
        strategy,
        index_buffer_mb,
        max_file_size,
        sparse_grams,
    } = opts;
    let explicit_buffer = index_buffer_mb.is_some();
    let buffer_bytes = index_buffer_mb
        .map(buffer_bytes_from_mb)
        .transpose()?
        .unwrap_or(builder::DEFAULT_INDEX_BUFFER_BYTES);

    // External is the default, so announcing it on every run would be noise.
    // Confirm the arena only when the user explicitly tuned it.
    if strategy == IndexStrategy::External && explicit_buffer {
        eprintln!(
            "Using external merge sort (arena {} MB before spilling)",
            buffer_bytes / (1024 * 1024)
        );
    }

    let started = Instant::now();
    // Dropped after the build so the sampled peak (on platforms without a
    // kernel high-water mark) covers the whole of it.
    let sampler = mem::PrivatePeakSampler::start();
    builder::build_index_with_options(
        root,
        index_path,
        &BuildOptions {
            include_hidden,
            no_ignore,
            no_require_git,
            exclude_dirs: exclude_dirs.to_vec(),
            strategy,
            buffer_bytes,
            max_file_size,
            sparse_grams,
            ..Default::default()
        },
    )?;
    let elapsed = started.elapsed();
    drop(sampler);

    let strategy_label = match strategy {
        IndexStrategy::InMemory => "memory",
        IndexStrategy::External => "external",
    };
    // Peak is an OS high-water mark for the whole process. `ogrep index` does
    // nothing substantial before the build, so it reads as the build's peak.
    match mem::format_peak_memory() {
        Some(peak) => eprintln!(
            "Indexed in {:.1}s using {} strategy (peak memory {})",
            elapsed.as_secs_f64(),
            strategy_label,
            peak
        ),
        None => eprintln!(
            "Indexed in {:.1}s using {} strategy",
            elapsed.as_secs_f64(),
            strategy_label
        ),
    }
    Ok(())
}

/// Build or update the history index next to the file index (`index --history`).
pub fn run_history(
    root: &Path,
    index_path: Option<&Path>,
    opts: &ogrep_core::history::HistoryOptions,
    force: bool,
) -> Result<()> {
    let repo = ogrep_core::history::repo_root(root)
        .map_err(|e| anyhow::anyhow!("--history needs a git repository: {e}"))?;
    let index_dir = match index_path {
        Some(path) => path.to_path_buf(),
        None => builder::default_index_dir(&std::fs::canonicalize(root)?),
    };
    let started = Instant::now();
    let outcome = ogrep_core::history::build_or_update(&repo, &index_dir, opts, force)?;
    let what = if outcome.rebuilt {
        format!("indexed {} commits", outcome.total)
    } else if outcome.added == 0 {
        format!("up to date ({} commits)", outcome.total)
    } else {
        format!("added {} commits ({} in all)", outcome.added, outcome.total)
    };
    let truncated = if outcome.truncated > 0 {
        format!(
            "; {} huge commits indexed in part (first {} MiB of changes)",
            outcome.truncated,
            ogrep_core::history::DOC_CAP_BYTES >> 20
        )
    } else {
        String::new()
    };
    eprintln!(
        "History {what} in {:.1}s{truncated}",
        started.elapsed().as_secs_f64()
    );
    Ok(())
}
