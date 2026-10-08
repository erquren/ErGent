#!/usr/bin/env python3
"""Verify archives without extracting/executing them; optionally require a full release."""
import argparse
import hashlib
import json
from pathlib import Path
import tarfile
import zipfile
from package import TARGETS, sha256


def verify(archive):
    sidecar = archive.with_name(archive.name + '.sha256')
    expected = f'{sha256(archive)}  {archive.name}\n'
    if sidecar.read_text(encoding='utf-8') != expected:
        raise ValueError(f'Archive checksum mismatch: {archive.name}')
    if archive.name.endswith('.zip'):
        with zipfile.ZipFile(archive) as z:
            entries = [(p.filename, z.read(p)) for p in z.infolist() if not p.is_dir()]
    else:
        with tarfile.open(archive, 'r:gz') as tar:
            entries = []
            for p in tar.getmembers():
                if p.isdir():
                    continue
                if not p.isfile():
                    raise ValueError(f'Unexpected link/special file: {p.name}')
                entries.append((p.name, tar.extractfile(p).read()))
    names = [name for name, _ in entries]
    if len(names) != len(set(names)):
        raise ValueError('Duplicate archive entries')
    roots = {name.split('/')[0] for name in names}
    if len(roots) != 1 or any('..' in name.split('/') or name.startswith('/') for name in names):
        raise ValueError('Invalid archive paths')
    root = roots.pop()
    contents = {name.removeprefix(root + '/'): data for name, data in entries}
    manifest = json.loads(contents.pop('manifest.json'))
    target = manifest['platform']
    if TARGETS[target] != manifest['target']:
        raise ValueError('Wrong Rust target in manifest')
    extension = '.zip' if target.startswith('windows') else '.tar.gz'
    if archive.name != f"ergent-{manifest['version']}-{target}{extension}" or root + extension != archive.name:
        raise ValueError('Archive name differs from manifest')
    suffix = '.exe' if extension == '.zip' else ''
    required = {'ergent-server' + suffix, 'ergent-agent' + suffix, 'web/index.html', 'README.md',
                'start-server.cmd' if suffix else 'start-server.sh'}
    if not required.issubset(contents):
        raise ValueError('Required runtime files missing')
    allowed = required - {'web/index.html'}
    if any(p not in allowed and not p.startswith('web/') for p in contents):
        raise ValueError('Unexpected files in release package')
    actual = {name: hashlib.sha256(data).hexdigest() for name, data in contents.items()}
    if actual != manifest['files']:
        raise ValueError('Package file hashes differ from manifest')
    return manifest, expected


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('directory', type=Path)
    parser.add_argument('--require-all', action='store_true')
    args = parser.parse_args()
    archives = sorted([*args.directory.glob('ergent-*.zip'), *args.directory.glob('ergent-*.tar.gz')])
    if not archives:
        parser.error('No release archives found')
    manifests, checksums = zip(*(verify(path) for path in archives))
    if args.require_all:
        if len(manifests) != len(TARGETS) or {m['platform'] for m in manifests} != set(TARGETS):
            parser.error('A full release must contain exactly all six platforms')
        if len({(m['version'], m['commit']) for m in manifests}) != 1 or any(m['dirty'] for m in manifests):
            parser.error('All release packages must come from the same clean commit and version')
        (args.directory / 'SHA256SUMS').write_text(''.join(checksums), encoding='utf-8')
    print(f'Verified {len(archives)} package(s).')


if __name__ == '__main__':
    main()
