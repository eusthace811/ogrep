#!/usr/bin/env python3
"""Render the release report from retained measurements; no new measurements."""
import argparse
import json
import math
from pathlib import Path
import re
import statistics


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument('--evidence', type=Path, required=True)
    ap.add_argument('--output', type=Path, required=True)
    args = ap.parse_args()
    root = args.evidence
    def read(name):
        return json.loads((root/name).read_text())
    def link(name, title=None):
        return f'[{title or name}]({root}/{name})'
    run = read('run.json')
    assert all(c['exit']==0 for c in run['commands']) and len(run['commands'])==15
    qualification = read('qualification.json')
    assert all(c['exit']==0 for c in qualification['checks'])
    summary = read('summary.json')['evaluations']
    scan = read('scan-verification.json')['evaluations']
    audit = read('audit.log')
    assert audit['vulnerabilities']['count']==0 and not audit['warnings']
    evaluations = {name:read(name) for name in summary}
    expected_hash = run['binary_sha256']
    assert all(x['engines']['fork']['sha256']==expected_hash for x in evaluations.values())
    for name in ['qualification.json','cache.json','load.json','index-resources.json','startup.json','recovery.json']:
        data=read(name)
        actual = data['engines']['fork']['binary_sha256'] if name in ['cache.json','index-resources.json'] else data['binary_sha256']
        assert actual==expected_hash,(name,actual)
    lines=['# zgrep-code 0.2.0 evaluation and release qualification','',
           'The measured improvement is faster local search startup: the path membership table is now sorted only when needed. Exact output, hidden-file type selection and dependency fixes also add value. Server throughput is not claimed to improve generally. This report qualifies trusted local Linux/WSL use; it does not qualify a shared network service or native macOS/Windows execution.','',
           '## Search latency','',
           'Speedups below are geometric means of per-query median ratios; above 1 means faster. The broader workload comes from 102 upstream queries. The two 12-query workloads were selected to exercise planner strengths and weaknesses, so they are not representative usage distributions.','',
           '| Workload | Mode | Queries | vs tgrep | vs zgrep 0.1.0 |',
           '| --- | --- | ---: | ---: | ---: |']
    for name,data in evaluations.items():
        ratios={e:math.exp(statistics.mean(math.log(v[e]['median_ms']/v['fork']['median_ms']) for v in data['summary'].values())) for e in ['upstream','release']}
        lines.append(f'| {name.removesuffix(".json").rsplit("-",1)[0]} | {data["mode"]} | {len(data["queries"])} | {ratios["upstream"]:.3f}× | {ratios["release"]:.3f}× |')
    lines += ['',f'All **{sum(len(x["samples"]) for x in evaluations.values()):,} timed invocations** reproduced their validated sorted file lists and exit codes. Independent ripgrep scans checked **{sum(len(x["queries_checked_against_scan"]) for x in scan.values())} workload/query combinations** across the three engines. '+link('scan-verification.json','Scan verification')+'.','',
              'Nine rounds per workload, two untimed warmups, seeded randomized query/engine order, four query threads. Timings include CLI startup and, in server mode, TCP transport. Warm cache; server startup is excluded. No builds, tests or other benchmarks launched by this runner overlap the timed workloads. This workstation is not isolated from unrelated OS activity.','',
              'Descriptive per-query 95% intervals use 5,000 paired-round bootstrap resamples, with no multiple-comparison correction. They measure run variability, not uncertainty across machines or query populations. '+link('summary.json','All medians and intervals')+'.','',
              '| Broad Rust workload | Queries with interval entirely faster than tgrep | Entirely slower |',
              '| --- | ---: | ---: |']
    for mode in ['local','server']:
        r=summary[f'rust-broad-{mode}.json']
        lines.append(f'| {mode} | {r["queries_faster_ci95"]}/102 | {r["queries_slower_ci95"]}/102 |')
    lines += ['', 'Five lowest observed speedups in the broad server workload are retained below; this is not a claim that every query improved.','',
              '| Query | tgrep ms | 0.2.0 ms | Speedup [95% interval] |','| --- | ---: | ---: | ---: |']
    data=evaluations['rust-broad-server.json']
    patterns={q['name']:q['pattern'] for q in data['queries']}
    for name,q in sorted(summary['rust-broad-server.json']['queries'].items(),key=lambda kv:kv[1]['speedup'])[:5]:
        pattern=patterns[name].replace('|','&#124;')
        lo,hi=q['speedup_ci95']
        lines.append(f'| `{pattern}` | {q["upstream_ms"]:.2f} | {q["fork_ms"]:.2f} | {q["speedup"]:.3f} [{lo:.3f}, {hi:.3f}] |')
    lines += ['', '## Cold index pages and restarts','',
              'The cache test uses disposable index copies, `posix_fadvise(DONTNEED)` and `mincore`. Every cold trial required zero resident index pages before running. All 180 cold/warm invocations agreed with independent scans. Source-file pages, executable pages, filesystem metadata and device caches were **not** established cold.','',
              '| Cold-index query | tgrep median ms | 0.1.0 median ms | 0.2.0 median ms |', '| --- | ---: | ---: | ---: |']
    cache=read('cache.json')
    for pattern in sorted({s['pattern'] for s in cache['samples']}):
        vals=[statistics.median(s['latency_ms'] for s in cache['samples'] if s['engine']==e and s['pattern']==pattern and s['cache']=='cold-index') for e in ['upstream','release','fork']]
        label='absent literal' if pattern.startswith('ZGREP_') else '`NonNull`'
        lines.append(f'| {label} | {vals[0]:.2f} | {vals[1]:.2f} | {vals[2]:.2f} |')
    startup=read('startup.json')['samples']
    lines += ['',f'Ten process restarts all completed startup reconciliation and returned the correct first-query files. Median readiness was **{statistics.median(s["ready_ms"] for s in startup):.1f} ms**, including up to 50 ms polling delay; median first RPC query was **{statistics.median(s["first_query_ms"] for s in startup):.2f} ms**. '+link('startup.json','Restart data')+'; '+link('cache.json','cache data')+'.','',
              '## Load, memory and indexing','',
              'Each concurrency level ran 1,024 loopback RPC requests against the frozen Rust tree, mixing six regexes (including a broad query returning 24,778 files). Each response file set was checked against ripgrep. These are end-to-end local RPC latencies with a four-thread server pool, not distributed-service measurements.','',
              '| Clients | Requests/s | Median ms | p95 ms | p99 ms | Server peak RSS MiB | Incorrect results |',
              '| ---: | ---: | ---: | ---: | ---: | ---: | ---: |']
    for r in read('load.json')['loads']:
        lines.append(f'| {r["clients"]} | {r["requests_per_second"]:.1f} | {r["median_ms"]:.2f} | {r["p95_ms"]:.2f} | {r["p99_ms"]:.2f} | {r["resources_after"]["VmHWM"]/1024:.1f} | {r["errors"]} |')
    lines += ['', 'Higher concurrency increases queueing and memory substantially. The server is suitable for trusted local use; four clients is a more conservative operating point on this measured workstation. The load generator and large result transport can also limit throughput. A worker-thread limit is not request admission control. '+link('load.json','Raw load and CPU data')+'.','',
              'Fresh indexing, one build per engine on warm source pages (descriptive measurements, not a statistical speed claim):','',
              '| Engine | Wall seconds | CPU seconds | Peak RSS MiB | Index MiB |','| --- | ---: | ---: | ---: | ---: |']
    for e,r in read('index-resources.json')['engines'].items():
        lines.append(f'| {e} | {r["elapsed_seconds"]:.2f} | {r["user_seconds"]+r["system_seconds"]:.2f} | {r["peak_rss_kib"]/1024:.1f} | {r["index_bytes"]/1048576:.2f} |')
    lines += ['', 'The posting format and practical index size are unchanged; tiny byte differences include metadata. No indexing-resource reduction is claimed. '+link('index-resources.json','Resource records')+'.','', '## Correctness and recovery','']
    checks={r['name']:r for r in qualification['checks']}
    agent=re.search(r'Ran (\d+) tests', (root/'qualification-agent.log').read_text()).group(1)
    installer=(root/'qualification-installer.log').read_text().count('PASS:')
    recovery=read('recovery.json')['modes']
    queries=sum(r['concurrent_queries'] for r in recovery)
    lines += [f'- **{checks["workspace"]["passed"]} Rust workspace tests**, plus the separate polling measurement and sparse-gram property test; generated planner checks cover 161,098 pattern/document comparisons.',
              f'- **{agent} agent integration tests** and **{installer} installer scenarios**; format and all-target Clippy checks pass.',
              '- **96 exact-output comparisons** against ripgrep across scan/local/server modes: lines, counts, JSON, context, quiet, file listing, types/globs, Unicode, multiline and error exits. JSON normalization removes performance counters only; error wording is not compared.',
              f'- **15 exact-output recovery checkpoints** across native notifications, explicit polling and forced polling fallback; **{queries:,} concurrent queries** during bulk edits and ignore changes. Each mode covers 400 bulk mutations, ignore exclusion/restoration, SIGKILL, 50 edits around the crash and restart.',
              '- Dependency audit: **zero vulnerabilities and zero warnings** against the recorded RustSec database. Prior failing audits and fixes are retained under `evidence/production/`.',
              '- The known-positive Unicode fixture still shows four indexed false negatives in tgrep, all corrected by the fork; all scanners and the ASCII control agree.','',
              link('qualification.json','Commands and test provenance')+'; '+link('output-modes-020.json','output cases')+'; '+link('recovery.json','recovery records')+'; '+link('audit.log','audit')+'; '+link('unicode-020.json','Unicode comparison')+'.','',
              'The new differential tests exposed inherited CRLF stripping, invented newlines in JSON, missing server JSON byte offsets and hidden-file type filtering. Initial failures are preserved, alongside focused regression tests. `--crlf` was silently ignored upstream; 0.2.0 rejects that unsupported matching mode with exit 2. Default LF matching preserves CR source bytes. This release is not full ripgrep compatibility.','',
              '## Provenance and delivery','',
              f'- Measured runtime source commit: `{run["source_commit"]}`.',
              f'- Release executable SHA-256: `{expected_hash}`.',
              '- Upstream tgrep: commit `56f8c62`, version 1.0.10; retained 0.1.0 release is the second baseline.',
              '- Rust corpus: `1a8fa555801329bd0e803d7384b5a21191c61f30`; CPython: `b54ae4cdfce4b84b22889b5c5b9ed0ab9bcb3702`. Separate detached worktrees protect benchmarks from changes to the user’s normal clones. Index provenance, clean corpus states, binary hashes and execution routes are verified.',
              '- Intel Core i9-14900KS, 32 available logical CPUs, WSL2 Linux 6.6.87.2 x86-64, Rust 1.98.1. No macOS/Windows runtime qualification or remote publication.',
              '- Run `python3 scripts/evaluate-release.py --output-directory evidence/NEW-RUN` after preparing the documented frozen corpora and baselines. '+link('run.json','Exact command manifest')+'.','']
    receipt=root/'release-validation.json'
    if receipt.exists():
        r=json.loads(receipt.read_text());assert r['binary_sha256']==expected_hash
        lines += [f'Packaged archive: [{r["archive"]}](dist/{r["archive"]}). Archive SHA-256: `{r["archive_sha256"]}`. Packaging source commit: `{r["source_commit"]}`; the executable hash matches every final evaluation. The extracted executable passes the Unicode/position smoke checks and all 96 output comparisons. '+link('release-validation.json','Packaging receipt')+'.','']
    else:
        lines += ['Packaging and extracted-executable validation are the remaining release gate.','']
    lines += ['## Operating limits and further improvements','',
              'Indexes and watchers are asynchronous; these checks do not create an atomic current-filesystem snapshot. For current eligible-file reads use `--no-index`. The trusted loopback service has no multi-user authorization. Search scope, unsupported options, memory controls and recovery are documented in [OPERATIONS.md](docs/OPERATIONS.md).','',
              'There is room to improve further: bound concurrent work and reduce file-list response overhead at the measured concurrency knee; profile parallel local verification for broad queries; add an explicit refresh-completion contract for agents. These are hypotheses to evaluate, not shipped capabilities. Sparse-gram indexing remains rejected by the retained experiment because its posting growth did not justify its selectivity. See [research](docs/RESEARCH.md) and [next evaluations](docs/NEXT_EVALUATION.md).','',
              'This evidence establishes useful improvements for the measured workloads and a qualified local release. It is not a formal proof for all regexes, filesystems, input encodings or operating systems.']
    args.output.write_text('\n'.join(lines)+'\n')


if __name__ == '__main__':
    main()
