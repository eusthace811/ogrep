#!/usr/bin/env python3
"""Summarize raw evaluations with paired-round bootstrap intervals (stdlib only)."""
import argparse
import json
import math
from pathlib import Path
import random
import statistics


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument('inputs', type=Path, nargs='+')
    ap.add_argument('--output', type=Path, required=True)
    args = ap.parse_args()
    output = {'method': 'ratio of medians, paired-round percentile bootstrap; '
              '5000 resamples; seed 1984; descriptive 95% intervals, no multiple-comparison correction',
              'evaluations': {}}
    for path in args.inputs:
        data = json.loads(path.read_text())
        rng = random.Random(1984)
        queries = {}
        for q in data['queries']:
            name = q['name']
            validated = data['validation'][name].values()
            assert len({(v['rows'], v['sha256'], v['exit']) for v in validated}) == 1, name
            rows = {}
            for engine in ('upstream', 'fork'):
                rows[engine] = {s['round']: s['ms'] for s in data['samples']
                                if s['engine'] == engine and s['query'] == name}
            rounds = sorted(rows['upstream'])
            assert rounds == sorted(rows['fork']) == list(range(data['rounds']))
            ratios = []
            for _ in range(5000):
                picked = rng.choices(rounds, k=len(rounds))
                ratios.append(statistics.median(rows['upstream'][i] for i in picked) /
                              statistics.median(rows['fork'][i] for i in picked))
            ratios.sort()
            summary = data['summary'][name]
            queries[name] = {'upstream_ms': summary['upstream']['median_ms'],
                             'fork_ms': summary['fork']['median_ms'],
                             'rg_ms': summary.get('rg', {}).get('median_ms'),
                             'speedup': summary['upstream']['median_ms'] / summary['fork']['median_ms'],
                             'speedup_ci95': [ratios[125], ratios[4874]]}
        speedups = [q['speedup'] for q in queries.values()]
        output['evaluations'][path.name] = {
            'queries': queries,
            'geomean_query_speedup': math.exp(statistics.mean(math.log(s) for s in speedups)),
            'sum_median_latency_speedup': sum(q['upstream_ms'] for q in queries.values()) /
                                         sum(q['fork_ms'] for q in queries.values()),
            'queries_faster_ci95': sum(q['speedup_ci95'][0] > 1 for q in queries.values()),
            'queries_slower_ci95': sum(q['speedup_ci95'][1] < 1 for q in queries.values())}
    args.output.write_text(json.dumps(output, indent=2) + '\n')


if __name__ == '__main__':
    main()
