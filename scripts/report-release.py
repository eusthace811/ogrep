#!/usr/bin/env python3
"""Render the 0.3.0 release report from complete retained measurements."""
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
        return json.loads((root / name).read_text())
    def link(name, title=None):
        return f'[{title or name}]({root}/{name})'
    run = read('run.json')
    assert run['version'] == '0.3.0' and len(run['commands']) == 20 and all(c['exit'] == 0 for c in run['commands'])
    identity = run['binary_sha256']
    qualified = read('qualification.json')
    assert qualified['binary_sha256'] == identity and all(c['exit'] == 0 for c in qualified['checks'])
    summaries = {name: read(filename)['evaluations'] for name, filename in [('upstream', 'summary.json'), ('release', 'summary-release.json')]}
    evaluations = {name: read(name) for name in summaries['upstream']}
    assert len(evaluations) == 6 and all(d['engines']['fork']['sha256'] == identity for d in evaluations.values())
    def speed(data, baseline):
        return math.exp(statistics.mean(math.log(q[baseline]['median_ms'] / q['fork']['median_ms']) for q in data['summary'].values()))
    local = speed(evaluations['rust-broad-local.json'], 'upstream')
    server = speed(evaluations['rust-broad-server.json'], 'upstream')
    lines = ['# zgrep-code 0.3.0 evaluation and release qualification', '',
             f'The 102-query Rust workload measured **{local:.3f}× local speedup** and **{server:.3f}× server-backed CLI speedup** versus tgrep. Source-handle reuse reduces local read work; compact file-list replies reduce response work. Bounded admission controls concurrent search/response allocations, and CLI/RPC/agent refresh receipts make completed edits searchable before success. These are workload-specific improvements, not a universal ranking.', '',
             'This report covers trusted local Linux/WSL use. It does not qualify a shared network service or native macOS/Windows operation. Previous [0.2.0 evidence](evidence/release-0.2.0/PRODUCTION_REPORT.md) and [development experiments](evidence/next-release/README.md) are preserved.', '',
             '## Search latency', '',
             'Speedups are geometric means of per-query median ratios; above one means faster. The broad set contains 102 upstream queries. Each targeted set has 12 queries chosen to exercise strengths and weaknesses, not a representative usage distribution.', '',
             '| Workload | Mode | Queries | vs tgrep | vs 0.2.0 |', '| --- | --- | ---: | ---: | ---: |']
    for name, data in evaluations.items():
        lines.append(f'| {name.removesuffix(".json").rsplit("-", 1)[0]} | {data["mode"]} | {len(data["queries"])} | {speed(data, "upstream"):.3f}× | {speed(data, "release"):.3f}× |')
    total = sum(len(d['samples']) for d in evaluations.values())
    lines += ['', f'All **{total:,} timed invocations** reproduced their validated file sets and exit codes. Independent ripgrep scans checked **252 workload/query combinations** across the three engines. Nine randomized, interleaved rounds, two untimed warmups, four query threads; ordinary `-l` semantics, including CLI startup and TCP transport in server mode. The server startup itself is excluded. Source pages are warm; the workstation is not isolated from unrelated OS activity. The runner overlaps no builds, tests or other benchmarks with timing.', '',
              link('scan-verification.json', 'Independent scan checks') + '; ' + link('run.json', 'Exact commands') + '.', '',
              'Per-query 95% intervals use 5,000 paired-round bootstrap resamples without multiple-comparison correction. They describe within-run variability, not uncertainty across machines or user query distributions.', '',
              '| Workload/mode | Baseline | Intervals entirely faster | Entirely slower |', '| --- | --- | ---: | ---: |']
    for baseline, records in summaries.items():
        for name, summary in records.items():
            lines.append(f'| {name.removesuffix(".json")} | {"tgrep" if baseline == "upstream" else "0.2.0"} | {summary["queries_faster_ci95"]}/{len(evaluations[name]["queries"])} | {summary["queries_slower_ci95"]}/{len(evaluations[name]["queries"])} |')
    lines += ['', link('summary.json', 'All tgrep comparisons') + '; ' + link('summary-release.json', 'All 0.2.0 comparisons') + '.', '',
              'The five lowest broad-server speedups against each baseline are retained below. A sub-one ratio is slower; small differences may be noise. All individual regressions remain in the linked raw summaries.', '',
              '| Baseline | Query | Baseline ms | 0.3.0 ms | Speedup [95% interval] |', '| --- | --- | ---: | ---: | ---: |']
    patterns = {q['name']: q['pattern'] for q in evaluations['rust-broad-server.json']['queries']}
    for baseline, records in summaries.items():
        for name, query in sorted(records['rust-broad-server.json']['queries'].items(), key=lambda pair: pair[1]['speedup'])[:5]:
            low, high = query['speedup_ci95']
            pattern = patterns[name].replace('|', '&#124;').replace('`', '\\`')
            lines.append(f'| {"tgrep" if baseline == "upstream" else "0.2.0"} | `{pattern}` | {query["baseline_ms"]:.2f} | {query["fork_ms"]:.2f} | {query["speedup"]:.3f} [{low:.3f}, {high:.3f}] |')
    lines += ['', '## Load, CPU and memory', '',
              'Each engine ran 1,024 loopback requests at each of 1/4/16 clients, mixing six regexes. Every file set matched ripgrep. All clients request optional compact replies; older servers ignore that parameter and send legacy rows. The current daemon uses the default four active search slots. Status is sampled during load. These are end-to-end local RPC results, including Python client parsing; the generator can constrain throughput.', '',
              '| Engine | Clients | Requests/s | Median ms | p95 ms | p99 ms | Server CPU s/1024 | Peak RSS MiB |',
              '| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |']
    for label, filename in [('tgrep', 'load-upstream.json'), ('0.2.0', 'load-release.json'), ('0.3.0', 'load.json')]:
        data = read(filename)
        for row in data['loads']:
            lines.append(f'| {label} | {row["clients"]} | {row["requests_per_second"]:.1f} | {row["median_ms"]:.2f} | {row["p95_ms"]:.2f} | {row["p99_ms"]:.2f} | {row["server_cpu_seconds"]:.2f} | {row["resources_after"]["VmHWM"] / 1024:.1f} |')
    lines += ['', 'Peak RSS is cumulative across concurrency levels in each process. Client CPU/RSS and all latency samples are retained. This comparison combines compact replies and admission; it cannot attribute the total gain to either alone. [Repeated admission-only experiments](docs/SERVICE_ADMISSION.md) found lower memory and tail latency with a modest throughput cost and higher median latency. Do not describe the limiter itself as a general speedup. Per-query output and total RSS are not bounded by the request count.', '',
              link('load.json', 'Current load') + '; ' + link('load-release.json', '0.2.0 load') + '; ' + link('load-upstream.json', 'tgrep load') + '.', '',
              'Fresh indexing used one build per engine on warm source pages. These are descriptive resource measurements; no indexing-speed improvement is claimed.', '',
              '| Engine | Wall s | CPU s | Peak RSS MiB | Index MiB |', '| --- | ---: | ---: | ---: | ---: |']
    for engine, row in read('index-resources.json')['engines'].items():
        lines.append(f'| {engine} | {row["elapsed_seconds"]:.2f} | {row["user_seconds"] + row["system_seconds"]:.2f} | {row["peak_rss_kib"] / 1024:.1f} | {row["index_bytes"] / 1048576:.2f} |')
    lines += ['', link('index-resources.json', 'Index resource data') + '.', '', '## Refresh correctness and operating cost', '',
              'Explicit refresh rereads eligible content and returns success after searchable publication. It covers completed edits whose content and eligibility stay stable through completion; concurrent mutations may or may not appear. It is not an atomic filesystem snapshot. Timeout and cancellation do not acknowledge freshness, and server work may continue. Agent `freshness: current` remains a direct scan.', '',
              '| Mode | Search clients | Refresh receipts | Median edit refresh ms | Maximum edit refresh ms | Maximum status ms | Checked concurrent searches |',
              '| --- | ---: | ---: | ---: | ---: | ---: | ---: |']
    refresh = read('refresh-completion.json')
    assert refresh['binary_sha256'] == identity
    for case in refresh['cases']:
        row = case['summary']
        lines.append(f'| {case["mode"]} | {case["clients"]} | {len(case["checks"])} | {row["refresh_median_ms"]:.2f} | {row["refresh_max_ms"]:.2f} | {row["status_max_ms"]:.2f} | {row["search_count"]:,} |')
    receipts = sum(len(c['checks']) for c in refresh['cases'])
    concurrent = sum(len(c['concurrent_search_ms']) for c in refresh['cases'])
    lines += ['', f'All **{receipts} receipts** passed immediate exact line-output and filename-set comparisons with independent ripgrep scans (**{receipts * 2} comparisons**), without polling retries. All **{concurrent:,} concurrent searches** matched the independently checked stable file set. The fixture uses 1,000 stable files plus edited subjects and covers 12 mutation types, ignore changes, restored mtime, binary filenames, hidden files and quiet-mode restarts. tgrep returns method-not-found for this explicit RPC; that is a capability comparison, not a speed ratio. '+link('refresh-completion.json', 'Raw refresh matrix')+'.', '']
    cost = read('refresh-cost.json')
    assert cost['binary_sha256'] == identity
    summary = cost['summary']
    lines += [f'On the pinned Rust tree, **{cost["rounds"][0]["receipt"]["num_files"]:,} content files**, one warmup plus 15 timed unchanged-tree refreshes gave median **{summary["refresh_median_ms"] / 1000:.3f} s**, range **{summary["refresh_min_ms"] / 1000:.3f}–{summary["refresh_max_ms"] / 1000:.3f} s**, and median **{summary["cpu_median_seconds"]:.2f} server CPU seconds**. Cumulative peak RSS was **{summary["peak_rss_mib"]:.1f} MiB**; worst status latency **{summary["status_max_ms"]:.2f} ms**. All 192 follow-up query checks agreed with independent scans.', '',
              'Refresh regenerates postings for every eligible file, so it is a batch operation with substantial CPU and memory cost. Repeated-pass observations do not establish an indefinite memory bound. The [development cost and stability report](docs/REFRESH_EVALUATION.md) retains both earlier favorable and unfavorable resource measurements. Avoid invoking it for every keystroke; a direct scan can be preferable for one query. '+link('refresh-cost.json', 'All release refresh-cost samples')+'.', '',
              '## Cache, recovery and correctness', '']
    stability = read('refresh-stability.json')
    assert stability['binary_sha256'] == identity and len(stability['rounds']) == 60
    stable = stability['summary']
    last = stability['rounds'][-20:]
    lines[-2:-2] = [f'A separate **60-pass** follow-up (plus warmup) retained the same binary and 12 independently checked queries after each receipt: **732 comparisons**. Median refresh was **{stable["refresh_median_ms"] / 1000:.3f} s**, cumulative peak **{stable["peak_rss_mib"]:.1f} MiB**, and worst status latency **{stable["status_max_ms"]:.2f} ms**. Across the final 20 passes, post-query RSS ranged **{min(r["rss_kib"] for r in last) / 1024:.1f}–{max(r["rss_kib"] for r in last) / 1024:.1f} MiB**, and cumulative peak ranged **{min(r["peak_rss_kib"] for r in last) / 1024:.1f}–{max(r["peak_rss_kib"] for r in last) / 1024:.1f} MiB**. The original 15-pass run remains above; the follow-up does not replace it or prove indefinite stability. '+link('refresh-stability.json', 'Extended observation')+'.', '']
    startup = read('startup.json')['samples']
    checks = {c['name']: c for c in qualified['checks']}
    agents = re.search(r'Ran (\d+) tests', (root / 'qualification-agent.log').read_text()).group(1)
    installer = (root / 'qualification-installer.log').read_text().count('PASS:')
    churn = sum(c['concurrent_queries'] for c in read('recovery.json')['modes'])
    lines += [f'- **{checks["workspace"]["passed"]} Rust workspace tests**, all features, plus the separate ignored polling measurement and sparse-gram example/property tests; format and all-target/all-feature Clippy pass.',
              f'- **{agents} agent integration/installer tests**, including real pi→MCP→CLI→server refresh, cancellation/shutdown, and explicit overload without scan fallback; **{installer} shell installer scenarios**. Agent tests explicitly use the measured release binary.',
              '- **96 exact output/exit comparisons** with ripgrep across scan/local/server modes; source line bytes, JSON offsets, counts, context, Unicode, multiline, scopes and error exits. JSON normalization removes performance counters only. This remains a documented subset of ripgrep compatibility.',
              f'- **15 recovery checkpoints** across native, poll and forced polling fallback, with **{churn:,} concurrent queries** during bulk changes; SIGKILL and 50 edits across restart are included.',
              '- **180 cold/warm index checks** require `mincore`-verified zero resident index pages after eviction. Source, executable, metadata and device caches were not established cold.',
              f'- **10 restarts**, median reconciled readiness **{statistics.median(s["ready_ms"] for s in startup):.1f} ms**, including up to 50 ms polling delay; median first query **{statistics.median(s["first_query_ms"] for s in startup):.2f} ms**.',
              '- Dependency audit has zero reported vulnerabilities and warnings against its recorded RustSec database; packaging repeats the audit. Earlier failures and fixes remain in historical evidence.', '',
              link('qualification.json', 'Qualification commands')+'; '+link('output-modes.json', 'Output cases')+'; '+link('recovery.json', 'Recovery')+'; '+link('cache.json', 'Cache')+'; '+link('startup.json', 'Restarts')+'; '+link('unicode.json', 'Known-positive Unicode comparison')+'; '+link('audit.log', 'Dependency audit')+'.', '',
              '## Provenance, delivery and limits', '',
              f'- Measured source: `{run["source_commit"]}`; executable SHA-256: `{identity}`.',
              '- Baselines: tgrep `56f8c62`, version 1.0.10; retained zgrep 0.2.0. Frozen Rust `1a8fa555801329bd0e803d7384b5a21191c61f30` and CPython `b54ae4cdfce4b84b22889b5c5b9ed0ab9bcb3702`; clean states, index provenance, binary identities and routes are verified.',
              '- Workstation: Intel Core i9-14900KS, Linux/WSL2 x86-64. Full platform, affinity and toolchain records are in the raw evaluations and qualification manifest.',
              '- Reproduce from a clean, built checkout with `python3 scripts/evaluate-release.py --output-directory evidence/NEW-RUN --work-root ../build/NEW-RUN`. Output must be empty; the refresh-cost work directory must be new.', '']
    path = root / 'release-validation.json'
    if path.exists():
        receipt = read('release-validation.json')
        assert receipt['binary_sha256'] == identity and receipt['version'] == '0.3.0'
        lines += [f'Local archive: [{receipt["archive"]}](dist/{receipt["archive"]}), SHA-256 `{receipt["archive_sha256"]}`. Packaging source `{receipt["source_commit"]}`. The extracted executable passes refresh, Unicode/position smoke checks and all 96 output cases. '+link('release-validation.json', 'Packaging receipt')+'.', '']
    else:
        lines += ['Packaging and extracted-executable validation remain required. This report is not yet a completion claim.', '']
    lines += ['The loopback service has no multi-user authorization. Admission bounds active/pending work, not output size or total RSS. Large refreshes can be expensive. No remote publication/signing, native macOS/Windows qualification, whole-machine cold-cache test or universal performance proof is claimed. [Operations](docs/OPERATIONS.md) documents freshness, scope, compatibility and recovery.', '',
              'The next optional experiments are boolean-only verification, matcher scratch reuse, avoiding JSON tree copies and content-identity reuse during refresh. They require new profiles and differential tests; none is an additional speed promise. [Research](docs/RESEARCH_NEXT.md), [local profiling](docs/LOCAL_PROFILING.md) and [admission ablation](docs/SERVICE_ADMISSION.md) record why the shipped changes were chosen.']
    args.output.write_text('\n'.join(lines) + '\n')


if __name__ == '__main__':
    main()
