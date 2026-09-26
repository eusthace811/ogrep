#!/usr/bin/env python3
"""Verify retained release evidence and exact artifact/source identities."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import tarfile


def sha(data):
    return hashlib.sha256(data).hexdigest()


def main():
    ap=argparse.ArgumentParser(description=__doc__)
    ap.add_argument('--evidence',type=Path,required=True)
    ap.add_argument('--output',type=Path,required=True)
    args=ap.parse_args()
    root=args.evidence
    def read(name):return json.loads((root/name).read_text())
    run=read('run.json');receipt=read('release-validation.json')
    binary_sha=run['binary_sha256']
    assert receipt['version']=='0.2.0'
    assert sha(Path('target/release/zgrep').read_bytes())==receipt['binary_sha256']==binary_sha
    assert len(run['commands'])==15 and all(c['exit']==0 for c in run['commands'])
    for source in [run['source_commit'],receipt['source_commit']]:
        assert not subprocess.check_output(['git','diff',source,'--','Cargo.toml','Cargo.lock','zgrep-core','zgrep-cli','scripts/agent'])
    archive=Path('dist')/receipt['archive']
    assert sha(archive.read_bytes())==receipt['archive_sha256']
    assert archive.with_suffix('.gz.sha256').read_text().strip()==f'{receipt["archive_sha256"]}  {archive.name}'
    with tarfile.open(archive,'r:gz') as tar:
        prefix=receipt['archive'].removesuffix('.tar.gz')
        assert sha(tar.extractfile(prefix+'/zgrep-code').read())==binary_sha
        assert json.load(tar.extractfile(prefix+'/BUILD.json'))['source_commit']==receipt['source_commit']
        assert len(tar.extractfile(prefix+'/OPERATIONS.md').read())>1000
    verification=read('scan-verification.json')['evaluations']
    assert len(verification)==6
    samples=0;queries=0
    for filename,v in verification.items():
        data=read(filename)
        for engine in data['engines'].values():
            assert sha(Path(engine['path']).read_bytes())==engine['sha256']
        assert data['engines']['fork']['sha256']==binary_sha
        corpus=data['corpus']
        assert subprocess.check_output(['git','-C',corpus,'rev-parse','HEAD'],text=True).strip()==data['corpus_revision']
        assert not subprocess.check_output(['git','-C',corpus,'status','--porcelain'])
        expected={(r,e,q['name']) for r in range(data['rounds']) for e in data['engines'] for q in data['queries']}
        actual={(s['round'],s['engine'],s['query']) for s in data['samples']}
        assert expected==actual and len(data['samples'])==len(expected)==v['timed_samples_checked']
        for q in data['queries']:
            scan=v['queries_checked_against_scan'][q['name']]
            assert all((r['rows'],r['sha256'],r['exit'])==(scan['rows'],scan['sha256'],scan['exit']) for r in data['validation'][q['name']].values())
        samples+=len(data['samples']);queries+=len(data['queries'])
    assert (samples,queries)==(6804,252)
    qualification=read('qualification.json')
    assert qualification['binary_sha256']==binary_sha
    assert len(qualification['checks'])==9 and all(c['exit']==0 for c in qualification['checks'])
    assert next(c['passed'] for c in qualification['checks'] if c['name']=='workspace')==951
    for name in ['output-modes-020.json','packaged-output-modes.json']:
        x=read(name);assert x['binary_sha256']==binary_sha and len(x['checks'])==96 and all(c['correct'] for c in x['checks'])
    load=read('load.json');assert load['binary_sha256']==binary_sha
    assert [r['clients'] for r in load['loads']]==[1,4,16]
    assert all(r['requests']==len(r['samples'])==1024 and r['errors']==0 and all(s['correct'] for s in r['samples']) for r in load['loads'])
    recovery=read('recovery.json');assert recovery['binary_sha256']==binary_sha
    assert len(recovery['modes'])==3 and all(len(r['checks'])==5 and all(c['correct'] for c in r['checks']) and r['concurrent_queries']>0 for r in recovery['modes'])
    cache=read('cache.json');assert cache['engines']['fork']['binary_sha256']==binary_sha
    assert len(cache['samples'])==180 and all(s['correct'] and s['resident_after_eviction']==0 and s['evicted_index_pages']>0 for s in cache['samples'])
    startup=read('startup.json');assert startup['binary_sha256']==binary_sha
    assert len(startup['samples'])==10 and all(s['correct'] for s in startup['samples'])
    assert read('index-resources.json')['engines']['fork']['binary_sha256']==binary_sha
    for name in ['audit.log','dependency-audit-at-packaging.json']:
        audit=read(name);assert audit['vulnerabilities']['count']==0 and not audit['warnings']
    result={'status':'PASS','scope':'trusted local Linux/WSL CLI and agent service',
            'runtime_source_commit':run['source_commit'],'packaging_source_commit':receipt['source_commit'],
            'binary_sha256':binary_sha,'archive_sha256':receipt['archive_sha256'],
            'timed_invocations':samples,'independent_scan_combinations':queries,'workspace_tests':951,
            'load_requests':3072,'recovery_checkpoints':15,'cold_warm_index_checks':180,'restarts':10,
            'output_checks':96,'packaged_output_checks':96,'dependency_vulnerabilities':0,'dependency_warnings':0,
            'checks':'Current runtime/lock source matches measured and packaged commits; current baseline binaries and frozen corpus identities match; artifact bytes, checksum file and embedded provenance agree; all recorded gates inspected.',
            'limits':'No whole-machine cold-cache, native macOS/Windows, shared-service or atomic-freshness claim. See PRODUCTION_REPORT.md for regressions and operating limits.'}
    args.output.write_text(json.dumps(result,indent=2)+'\n')
    print(json.dumps(result,indent=2))


if __name__=='__main__':main()
