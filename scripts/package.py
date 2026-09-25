#!/usr/bin/env python3
"""Build a local Linux release from clean source; verify the unpacked binary."""
import hashlib
import json
from pathlib import Path
import platform
import shutil
import subprocess
import tarfile
import tempfile
import time

ROOT = Path(__file__).resolve().parent.parent


def checked(*command, cwd=ROOT):
    return subprocess.run(command, cwd=cwd, check=True, capture_output=True, text=True).stdout.strip()


def smoke(binary, temporary):
    corpus = temporary / 'corpus'
    corpus.mkdir()
    (corpus / '.git').mkdir()
    for name, content in [('hit.txt', 'abcde\nKELVIN\n'), ('scatter.txt', 'abcdXbcde\n')]:
        (corpus / name).write_text(content)
    checked(str(binary), 'index', str(corpus))

    def search(server):
        p = subprocess.run([str(binary), '--no-config', '-Fi', '-l', '--explain',
                            '--color', 'never', '--', 'kelvin', '.'],
                           cwd=corpus, capture_output=True, text=True, check=True)
        assert p.stdout.splitlines() == ['./hit.txt'], p.stdout
        assert ('(via server)' in p.stderr) == server, p.stderr
        p = subprocess.run([str(binary), '--no-config', '-F', '-l', '--explain',
                            '--color', 'never', '--', 'abcde', '.'],
                           cwd=corpus, capture_output=True, text=True, check=True)
        assert p.stdout.splitlines() == ['./hit.txt'], p.stdout
        work = json.loads(next(line.removeprefix('Index work: ') for line in p.stderr.splitlines()
                               if line.startswith('Index work: ')))
        assert work['position_rejections'] == 1, work

    search(False)
    with (temporary / 'server.log').open('w') as log:
        server = subprocess.Popen([str(binary), 'serve', str(corpus), '--no-watch'],
                                  stdout=log, stderr=log)
        try:
            deadline = time.monotonic() + 30
            while True:
                if server.poll() is not None or time.monotonic() > deadline:
                    raise RuntimeError('Packaged server failed to become ready')
                try:
                    descriptor = json.loads((corpus / '.zgrep' / 'serve.json').read_text())
                except (FileNotFoundError, json.JSONDecodeError):
                    descriptor = {}
                if descriptor.get('pid') == server.pid:
                    break
                time.sleep(.05)
            search(True)
        finally:
            server.terminate()
            try:
                server.wait(timeout=10)
            except subprocess.TimeoutExpired:
                server.kill()
                server.wait()


def main():
    if platform.system() != 'Linux':
        raise SystemExit('This local packaging gate currently supports Linux only.')
    if checked('git', 'status', '--porcelain'):
        raise SystemExit('Commit source and evidence before packaging; checkout must be clean.')
    revision = checked('git', 'rev-parse', 'HEAD')
    subprocess.run(['cargo', 'build', '--release', '--locked'], cwd=ROOT, check=True)
    binary = ROOT / 'target/release/zgrep'
    version = checked(str(binary), '--version').split()[-1]
    rustc = checked('rustc', '-vV')
    host = next(line.removeprefix('host: ') for line in rustc.splitlines() if line.startswith('host: '))
    name = f'zgrep-code-{version}-{host}'
    output = ROOT / 'dist'
    output.mkdir(exist_ok=True)
    archive = output / f'{name}.tar.gz'
    manifest = {'version': version, 'source_commit': revision, 'upstream_commit': '56f8c62',
                'rustc': rustc, 'platform': platform.platform(),
                'binary_sha256': hashlib.sha256(binary.read_bytes()).hexdigest()}
    with tempfile.TemporaryDirectory(prefix='zgrep-package-') as directory:
        temporary = Path(directory)
        package = temporary / name
        package.mkdir()
        shutil.copy2(binary, package / 'zgrep-code')
        for filename in ('LICENSE', 'CHANGELOG.md', 'AGENTS.md'):
            shutil.copy2(ROOT / filename, package / filename)
        (package / 'OPERATIONS.md').write_text((ROOT / 'docs/OPERATIONS.md').read_text().replace('../AGENTS.md', 'AGENTS.md'))
        (package / 'BUILD.json').write_text(json.dumps(manifest, indent=2) + '\n')
        (package / 'README.md').write_text(
            '# zgrep-code\n\nCopy `zgrep-code` to a directory on PATH.\n\n'
            '```sh\nzgrep-code index /path/to/repository\n'
            'zgrep-code -F --explain -- some_identifier /path/to/repository\n```\n\n'
            'See [OPERATIONS.md](OPERATIONS.md) for supported behavior, resource controls and recovery.\n\n'
            'For automatic updates, run `zgrep-code serve /path/to/repository`. '
            'Watchers are asynchronous; use `--no-index` to read current eligible files. '
            'Keep `.zgrep/` out of Git. Unix `zgrep` is an unrelated gzip utility.\n\n'
            f'Source commit: `{revision}`. See source BENCHMARKS.md and docs/OPERATIONS.md '
            'for evidence, eligibility and freshness limits. This is a locally built, unsigned Linux release.\n')
        with tarfile.open(archive, 'w:gz') as tar:
            tar.add(package, arcname=name)
        unpacked = temporary / 'unpacked'
        with tarfile.open(archive, 'r:gz') as tar:
            tar.extractall(unpacked, filter='data')
        packaged_binary = unpacked / name / 'zgrep-code'
        assert hashlib.sha256(packaged_binary.read_bytes()).hexdigest() == manifest['binary_sha256']
        smoke(packaged_binary, temporary)
        subprocess.run(['python3', str(ROOT / 'scripts/evaluate-output-modes.py'),
                        '--binary', str(packaged_binary),
                        '--output', str(output / 'packaged-output-modes.json')],
                       cwd=ROOT, check=True)
    checksum = hashlib.sha256(archive.read_bytes()).hexdigest()
    archive.with_suffix(archive.suffix + '.sha256').write_text(f'{checksum}  {archive.name}\n')
    receipt = {**manifest, 'archive': archive.name, 'archive_sha256': checksum,
               'smoke': 'PASS: extracted binary; local/server Unicode matching, positional filtering, diagnostics; 96 scan/local/server exact-output checks against ripgrep'}
    (output / 'release-validation.json').write_text(json.dumps(receipt, indent=2) + '\n')
    print(json.dumps(receipt, indent=2))


if __name__ == '__main__':
    main()
