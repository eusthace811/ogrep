#!/usr/bin/env python3
"""Run the frozen Linux release evaluation sequentially and retain its manifest.

Run from the fork root after building target/release/zgrep. Benchmark roots and
baselines are separate worktrees/binaries under ../build, as documented in the
release report. No benchmark overlaps this runner's tests or builds.
"""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import time


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument('--output-directory', type=Path, required=True)
    args = ap.parse_args()
    out = args.output_directory
    out.mkdir(parents=True, exist_ok=True)
    engines = ['--engine', 'upstream=../build/baseline/release/tgrep', '--engine', 'release=../build/baseline/zgrep-0.1.0', '--engine', 'fork=target/release/zgrep']
    rust = '../build/corpora-pinned/rust'
    binary = Path('target/release/zgrep')
    report = {'source_commit': subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),
              'binary_sha256': hashlib.sha256(binary.read_bytes()).hexdigest(), 'commands': []}
    def run(name, command):
        started = time.time()
        with (out/(name+'.log')).open('w') as log:
            p = subprocess.run(command, stdout=log, stderr=subprocess.STDOUT)
        report['commands'].append({'name':name, 'command':command, 'exit':p.returncode,
                                   'started_unix':started, 'ended_unix':time.time()})
        (out/'run.json').write_text(json.dumps(report,indent=2)+'\n')
        print(name, 'PASS' if p.returncode==0 else 'FAIL',flush=True)
        if p.returncode:
            raise SystemExit(p.returncode)
    evaluations=[]
    for corpus, workload, queries, revision in [
        ('rust','broad','evidence/queries-upstream-rust.json','1a8fa555801329bd0e803d7384b5a21191c61f30'),
        ('rust','targeted','evidence/queries-rust.json','1a8fa555801329bd0e803d7384b5a21191c61f30'),
        ('cpython','targeted','evidence/queries-cpython.json','b54ae4cdfce4b84b22889b5c5b9ed0ab9bcb3702')]:
        for mode in ['local','server']:
            name=f'{corpus}-{workload}-{mode}'
            path=out/(name+'.json')
            evaluations.append(str(path))
            run(name,['python3','scripts/evaluate.py',*engines,'--corpus',f'../build/corpora-pinned/{corpus}',
                      '--expected-revision',revision,'--queries',queries,'--work',f'../build/production-eval-{corpus}',
                      '--mode',mode,'--rounds','9','--seed','261337','--output',str(path)])
    run('cache',['python3','scripts/evaluate-index-cache.py',
                 '--engine','upstream=../build/baseline/release/tgrep=../build/production-eval-rust/upstream/index',
                 '--engine','release=../build/baseline/zgrep-0.1.0=../build/production-eval-rust/release/index',
                 '--engine','fork=target/release/zgrep=../build/production-eval-rust/fork/index',
                 '--corpus',rust,'--output',str(out/'cache.json')])
    run('load',['python3','scripts/evaluate-load.py','--binary',str(binary),'--corpus',rust,
                '--index','../build/production-eval-rust/fork/index','--output',str(out/'load.json')])
    run('index-resources',['python3','scripts/evaluate-index-resources.py',*engines,'--corpus',rust,'--output',str(out/'index-resources.json')])
    run('startup',['python3','scripts/evaluate-startup.py','--binary',str(binary),'--corpus',rust,
                  '--index','../build/production-eval-rust/fork/index','--output',str(out/'startup.json')])
    run('recovery',['python3','scripts/evaluate-recovery.py','--binary',str(binary),'--output',str(out/'recovery.json')])
    run('qualification',['python3','scripts/qualify.py','--upstream','../build/baseline/release/tgrep','--output',str(out/'qualification.json')])
    run('scan-verification',['python3','scripts/verify-comparison.py',*evaluations,'--output',str(out/'scan-verification.json')])
    run('summary',['python3','scripts/summarize-evaluation.py',*evaluations,'--output',str(out/'summary.json')])
    run('audit',['../build/tools/bin/cargo-audit','audit','--json'])
    assert report['binary_sha256']==hashlib.sha256(binary.read_bytes()).hexdigest(), 'binary changed'


if __name__ == '__main__':
    main()
