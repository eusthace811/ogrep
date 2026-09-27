#!/usr/bin/env python3
"""Run the frozen Linux release evaluation sequentially and retain its manifest.

Run from the fork root after building target/release/ogrep. Benchmark roots and
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
    ap.add_argument('--work-root', type=Path, default=Path('../build/release-0.3.0-eval'))
    args = ap.parse_args()
    out = args.output_directory
    out.mkdir(parents=True, exist_ok=True)
    if any(out.iterdir()):
        ap.error('output directory must be empty; preserve prior evaluations')
    engines = ['--engine', 'upstream=../build/baseline/release/tgrep', '--engine', 'release=../build/baseline/ogrep-0.2.0', '--engine', 'fork=target/release/ogrep']
    rust = '../build/corpora-pinned/rust'
    work_rust = args.work_root / 'rust'
    binary = Path('target/release/ogrep')
    assert not subprocess.check_output(['git', 'status', '--porcelain']), 'commit source before final evaluation'
    assert subprocess.check_output([str(binary), '--version'], text=True).strip() == 'ogrep 0.3.0'
    report = {'version': '0.3.0', 'source_commit': subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),
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
                      '--expected-revision',revision,'--queries',queries,'--work',str(args.work_root/corpus),
                      '--mode',mode,'--rounds','9','--seed','273337','--output',str(path)])
    run('cache',['python3','scripts/evaluate-index-cache.py',
                 '--engine',f'upstream=../build/baseline/release/tgrep={work_rust}/upstream/index',
                 '--engine',f'release=../build/baseline/ogrep-0.2.0={work_rust}/release/index',
                 '--engine',f'fork=target/release/ogrep={work_rust}/fork/index',
                 '--corpus',rust,'--output',str(out/'cache.json')])
    for name, executable in [('upstream', '../build/baseline/release/tgrep'), ('release', '../build/baseline/ogrep-0.2.0')]:
        run(f'load-{name}', ['python3', 'scripts/evaluate-load.py', '--binary', executable, '--corpus', rust,
                            '--index', str(work_rust/name/'index'), '--compact-files', '--observe-status', '--output', str(out/f'load-{name}.json')])
    run('load',['python3','scripts/evaluate-load.py','--binary',str(binary),'--corpus',rust,
                '--index',str(work_rust/'fork/index'),'--compact-files','--observe-status','--output',str(out/'load.json')])
    run('index-resources',['python3','scripts/evaluate-index-resources.py',*engines,'--corpus',rust,'--output',str(out/'index-resources.json')])
    run('startup',['python3','scripts/evaluate-startup.py','--binary',str(binary),'--corpus',rust,
                  '--index',str(work_rust/'fork/index'),'--output',str(out/'startup.json')])
    run('recovery',['python3','scripts/evaluate-recovery.py','--binary',str(binary),'--output',str(out/'recovery.json')])
    run('refresh-completion', ['python3', 'scripts/evaluate-refresh-completion.py', '--binary', str(binary),
                               '--upstream', '../build/baseline/release/tgrep', '--output', str(out/'refresh-completion.json')])
    run('refresh-cost', ['python3', 'scripts/evaluate-refresh-cost.py', '--binary', str(binary), '--corpus', rust,
                        '--expected-revision', '1a8fa555801329bd0e803d7384b5a21191c61f30',
                        '--index-source', str(work_rust/'fork/index'), '--work', str(args.work_root/'refresh-cost-rust'),
                        '--queries', 'evidence/queries-rust.json', '--rounds', '15', '--output', str(out/'refresh-cost.json')])
    run('qualification',['python3','scripts/qualify.py','--upstream','../build/baseline/release/tgrep','--output',str(out/'qualification.json')])
    run('scan-verification',['python3','scripts/verify-comparison.py',*evaluations,'--output',str(out/'scan-verification.json')])
    run('summary',['python3','scripts/summarize-evaluation.py',*evaluations,'--output',str(out/'summary.json')])
    run('summary-release',['python3','scripts/summarize-evaluation.py',*evaluations,'--baseline','release','--output',str(out/'summary-release.json')])
    run('audit',['../build/tools/bin/cargo-audit','audit','--json'])
    assert report['binary_sha256']==hashlib.sha256(binary.read_bytes()).hexdigest(), 'binary changed'


if __name__ == '__main__':
    main()
