#!/usr/bin/env python3
"""Verify the full 0.3.0 evidence matrix and exact measured/package identities."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess
import tarfile

UPSTREAM = 'b1bb0f7d512a7824a82f33c745aa776cceb17332130d6c2e78f20576dada0754'
PREVIOUS = '45777bbc84a9043cf3de690b781cf623fa1806368ae377ae5de4386f5450351d'
WORKLOADS = {f'{corpus}-{workload}-{mode}' for corpus, workload in [('rust', 'broad'), ('rust', 'targeted'), ('cpython', 'targeted')] for mode in ['local', 'server']}
COMMANDS = WORKLOADS | {'cache', 'load-upstream', 'load-release', 'load', 'index-resources', 'startup', 'recovery', 'refresh-completion', 'refresh-cost', 'qualification', 'scan-verification', 'summary', 'summary-release', 'audit'}


def sha(data):
    return hashlib.sha256(data).hexdigest()


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument('--evidence', type=Path, required=True)
    ap.add_argument('--output', type=Path, required=True)
    args = ap.parse_args()
    root = args.evidence
    def read(name):
        return json.loads((root / name).read_text())
    run, receipt = read('run.json'), read('release-validation.json')
    binary_sha = run['binary_sha256']
    assert run['version'] == receipt['version'] == '0.3.0'
    assert sha(Path('target/release/ogrep').read_bytes()) == receipt['binary_sha256'] == binary_sha
    assert len(run['commands']) == len(COMMANDS) and {c['name'] for c in run['commands']} == COMMANDS
    assert all(c['exit'] == 0 for c in run['commands'])
    assert all(a['ended_unix'] <= b['started_unix'] for a, b in zip(run['commands'], run['commands'][1:]))
    for source in [run['source_commit'], receipt['source_commit']]:
        assert not subprocess.check_output(['git', 'diff', source, '--', 'Cargo.toml', 'Cargo.lock', 'ogrep-core', 'ogrep-cli', 'scripts/agent', 'scripts/evaluate*.py', 'scripts/qualify.py', 'scripts/verify-comparison.py', 'scripts/summarize-evaluation.py'])
    archive = Path('dist') / receipt['archive']
    assert sha(archive.read_bytes()) == receipt['archive_sha256']
    assert archive.with_suffix('.gz.sha256').read_text().strip() == f'{receipt["archive_sha256"]}  {archive.name}'
    with tarfile.open(archive, 'r:gz') as tar:
        prefix = receipt['archive'].removesuffix('.tar.gz')
        assert sha(tar.extractfile(prefix + '/ogrep').read()) == binary_sha
        assert json.load(tar.extractfile(prefix + '/BUILD.json'))['source_commit'] == receipt['source_commit']
        for filename, source in [('OPERATIONS.md', 'docs/OPERATIONS.md'), ('AGENTS.md', 'AGENTS.md'), ('CHANGELOG.md', 'CHANGELOG.md')]:
            expected = Path(source).read_text()
            if filename == 'OPERATIONS.md':
                expected = expected.replace('../AGENTS.md', 'AGENTS.md')
            assert tar.extractfile(prefix + '/' + filename).read().decode() == expected
        assert 'refresh' in tar.extractfile(prefix + '/README.md').read().decode()
    verification = read('scan-verification.json')['evaluations']
    assert set(verification) == {name + '.json' for name in WORKLOADS}
    samples = queries = 0
    for filename, verified in verification.items():
        data = read(filename)
        assert set(data['engines']) == {'upstream', 'release', 'fork'}
        assert data['engines']['upstream']['sha256'] == UPSTREAM
        assert data['engines']['release']['sha256'] == PREVIOUS
        assert data['engines']['fork']['sha256'] == binary_sha
        for engine in data['engines'].values():
            assert sha(Path(engine['path']).read_bytes()) == engine['sha256']
        corpus = data['corpus']
        assert subprocess.check_output(['git', '-C', corpus, 'rev-parse', 'HEAD'], text=True).strip() == data['corpus_revision']
        assert not subprocess.check_output(['git', '-C', corpus, 'status', '--porcelain'])
        expected = {(r, e, q['name']) for r in range(9) for e in data['engines'] for q in data['queries']}
        assert data['rounds'] == 9 and {(s['round'], s['engine'], s['query']) for s in data['samples']} == expected
        assert len(data['samples']) == len(expected) == verified['timed_samples_checked']
        for query in data['queries']:
            scan = verified['queries_checked_against_scan'][query['name']]
            for result in data['validation'][query['name']].values():
                assert (result['rows'], result['sha256'], result['exit']) == (scan['rows'], scan['sha256'], scan['exit'])
                if data['mode'] == 'local':
                    assert 'Query plan:' in result['diagnostics']
                else:
                    assert '(via server)' in result['diagnostics']
        samples += len(data['samples'])
        queries += len(data['queries'])
    assert (samples, queries) == (6804, 252)
    for name in ['summary.json', 'summary-release.json']:
        assert set(read(name)['evaluations']) == set(verification)
    qualification = read('qualification.json')
    assert qualification['binary_sha256'] == binary_sha
    checks = {c['name']: c for c in qualification['checks']}
    assert len(checks) == len(qualification['checks']) == 9 and all(c['exit'] == 0 for c in checks.values())
    assert checks['workspace']['passed'] == 979 and checks['workspace']['ignored'] == 1
    assert '--all-features' in checks['workspace']['command'] and '--all-features' in checks['clippy']['command']
    assert checks['polling']['passed'] >= 1 and checks['sparse-example']['passed'] >= 1
    agent_log = (root / 'qualification-agent.log').read_text()
    assert re.search(r'Ran 43 tests', agent_log) and 'OK' in agent_log and 'skipped' not in agent_log
    assert (root / 'qualification-installer.log').read_text().count('PASS:') == 49
    for name in ['output-modes.json', 'packaged-output-modes.json']:
        output = read(name)
        assert output['binary_sha256'] == binary_sha and len(output['checks']) == 96 and all(c['correct'] for c in output['checks'])
    for name, identity in [('load-upstream.json', UPSTREAM), ('load-release.json', PREVIOUS), ('load.json', binary_sha)]:
        load = read(name)
        assert load['binary_sha256'] == identity and load['compact_files'] and load['observe_status']
        assert [r['clients'] for r in load['loads']] == [1, 4, 16]
        for row in load['loads']:
            assert row['requests'] == len(row['samples']) == 1024 and row['errors'] == 0 and all(s['correct'] for s in row['samples'])
            assert row['status_observations'] and all('error' not in s for s in row['status_observations'])
            if name == 'load.json':
                for status in row['status_observations']:
                    assert status['search_admission']['capacity'] == 4 and status['search_admission']['active'] <= 4 and status['search_admission']['queued'] <= 32
                    assert status['connection_admission']['capacity'] == 64 and status['connection_admission']['active'] <= 64
    recovery = read('recovery.json')
    assert recovery['binary_sha256'] == binary_sha and {r['mode'] for r in recovery['modes']} == {'native', 'poll', 'fallback'}
    assert all(len(r['checks']) == 5 and all(c['correct'] for c in r['checks']) and r['concurrent_queries'] > 0 for r in recovery['modes'])
    cache = read('cache.json')
    assert cache['engines']['fork']['binary_sha256'] == binary_sha
    assert len(cache['samples']) == 180 and all(s['correct'] and s['resident_after_eviction'] == 0 and s['evicted_index_pages'] > 0 for s in cache['samples'])
    startup = read('startup.json')
    assert startup['binary_sha256'] == binary_sha and len(startup['samples']) == 10 and all(s['correct'] for s in startup['samples'])
    assert read('index-resources.json')['engines']['fork']['binary_sha256'] == binary_sha
    refresh = read('refresh-completion.json')
    assert refresh['binary_sha256'] == binary_sha and refresh['files'] == 1000 and refresh['cycles'] == 3
    assert {(r['mode'], r['clients']) for r in refresh['cases']} == {(m, n) for m in ['native', 'disabled', 'poll'] for n in [0, 16]}
    operations = ['edit', 'create', 'rename', 'atomic_replace', 'delete', 'ignore', 'unignore', 'preserved_mtime', 'subtree_create', 'subtree_recreate', 'binary_filename', 'hidden']
    for case in refresh['cases']:
        labels = {'initial-build'} | {f'{c}/{op}' for c in range(3) for op in operations}
        if case['clients'] == 0:
            labels.add('offline-edit-and-restart')
        else:
            assert len(case['concurrent_search_ms']) > 0
        assert {c['label'] for c in case['checks']} == labels and len(case['checks']) == len(labels)
        assert all(c['correct'] and c['receipt']['status'] == 'refreshed' for c in case['checks'])
        assert case['status_samples'] and all('error' not in s for s in case['status_samples'])
        assert any(s['manual_refresh']['running'] for s in case['status_samples'])
        assert all(s['search_admission']['active'] <= 4 for s in case['status_samples'])
    assert refresh['upstream_capability']['binary_sha256'] == UPSTREAM and refresh['upstream_capability']['reply']['error']['code'] == -32601
    cost = read('refresh-cost.json')
    assert cost['binary_sha256'] == binary_sha and len(cost['rounds']) == 15
    assert [r['round'] for r in cost['rounds']] == list(range(15))
    for row in [cost['warmup'], *cost['rounds']]:
        assert row['receipt']['status'] == 'refreshed' and len(row['checks']) == 12 and all(c['correct'] for c in row['checks'].values())
    assert cost['status_samples'] and all('error' not in s for s in cost['status_samples'])
    stability = read('refresh-stability.json')
    assert stability['binary_sha256'] == binary_sha and len(stability['rounds']) == 60
    assert stability['revision'] == cost['revision'] and stability['query_file_sha256'] == cost['query_file_sha256']
    assert [r['round'] for r in stability['rounds']] == list(range(60))
    for row in [stability['warmup'], *stability['rounds']]:
        assert row['receipt']['status'] == 'refreshed' and len(row['checks']) == 12 and all(c['correct'] for c in row['checks'].values())
    assert stability['status_samples'] and all('error' not in s for s in stability['status_samples'])
    for name in ['audit.log', 'dependency-audit-at-packaging.json']:
        audit = read(name)
        assert audit['vulnerabilities']['count'] == 0 and not audit['warnings']
    result = {'status': 'PASS', 'version': '0.3.0', 'scope': 'trusted local Linux/WSL CLI and agent service',
              'runtime_source_commit': run['source_commit'], 'packaging_source_commit': receipt['source_commit'],
              'binary_sha256': binary_sha, 'archive_sha256': receipt['archive_sha256'],
              'timed_invocations': samples, 'independent_scan_combinations': queries, 'workspace_tests': 979,
              'agent_tests': 43, 'installer_scenarios': 49, 'load_requests': 9216, 'recovery_checkpoints': 15,
              'cold_warm_index_checks': 180, 'restarts': 10, 'output_checks': 96, 'packaged_output_checks': 96,
              'refresh_receipts': sum(len(c['checks']) for c in refresh['cases']),
              'refresh_concurrent_queries': sum(len(c['concurrent_search_ms']) for c in refresh['cases']),
              'refresh_cost_passes': 16, 'refresh_cost_query_checks': 192,
              'refresh_stability_passes': 61, 'refresh_stability_query_checks': 732,
              'dependency_vulnerabilities': 0, 'dependency_warnings': 0,
              'checks': 'Current runtime, lock, agent and evaluator sources match measured/package commits; baseline and corpus identities match; artifact bytes and embedded source agree; complete retained matrices and all release gates inspected.',
              'limits': 'No whole-machine cold-cache, native macOS/Windows, shared-network-service, fixed-RSS-bound or atomic-filesystem-snapshot claim. Refresh covers completed edits stable through completion. See PRODUCTION_REPORT.md.'}
    args.output.write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps(result, indent=2))


if __name__ == '__main__':
    main()
