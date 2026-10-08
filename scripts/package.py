#!/usr/bin/env python3
"""Build a native release bundle. CI runs this once on each supported platform."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import sys
import tarfile
import tempfile
import zipfile

ROOT = Path(__file__).resolve().parents[1]
TARGETS = {
    'windows-x86_64': 'x86_64-pc-windows-msvc',
    'windows-arm64': 'aarch64-pc-windows-msvc',
    'linux-x86_64': 'x86_64-unknown-linux-gnu',
    'linux-arm64': 'aarch64-unknown-linux-gnu',
    'macos-x86_64': 'x86_64-apple-darwin',
    'macos-arm64': 'aarch64-apple-darwin',
}


def native_target():
    system = {'Darwin': 'macos', 'Linux': 'linux', 'Windows': 'windows'}.get(platform.system())
    arch = {'amd64': 'x86_64', 'x86_64': 'x86_64', 'aarch64': 'arm64', 'arm64': 'arm64'}.get(platform.machine().lower())
    key = f'{system}-{arch}'
    if key not in TARGETS:
        raise RuntimeError(f'Unsupported host: {platform.system()} {platform.machine()}')
    return key


def build_env():
    env = os.environ.copy()
    local = ROOT / '.tools/cargo'
    if (local / 'bin/cargo').is_file():
        env.update(CARGO_HOME=str(local), RUSTUP_HOME=str(ROOT / '.tools/rustup'))
        env['PATH'] = str(local / 'bin') + os.pathsep + env['PATH']
    node = ROOT / '.tools/node/node_modules/.bin'
    if node.is_dir():
        env['PATH'] = str(node) + os.pathsep + env['PATH']
    return env


def run(args, env, capture=False):
    print('+ ' + ' '.join(map(str, args)), flush=True)
    return subprocess.run(list(map(str, args)), cwd=ROOT, env=env, check=True,
                          text=True, stdout=subprocess.PIPE if capture else None).stdout


def sha256(path):
    with path.open('rb') as stream:
        digest = hashlib.sha256()
        for chunk in iter(lambda: stream.read(1024 * 1024), b''):
            digest.update(chunk)
    return digest.hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--target', choices=TARGETS, help='Defaults to the native OS/CPU; use Release CI for all six.')
    parser.add_argument('--out-dir', type=Path, default=ROOT / 'release')
    parser.add_argument('--expect-version', help='Fail if this version/tag differs from Cargo metadata.')
    parser.add_argument('--list-targets', action='store_true')
    args = parser.parse_args()
    if args.list_targets:
        print(json.dumps(TARGETS, indent=2))
        return
    target = args.target or native_target()
    if target != native_target():
        parser.error(f'{target} requires its matching host. Use the GitHub Actions Release workflow to build all platforms.')
    env = build_env()
    cargo = shutil.which('cargo', path=env['PATH'])
    pnpm = shutil.which('pnpm', path=env['PATH'])
    if not cargo or not pnpm:
        parser.error('Install Rust (rust-toolchain.toml), Node.js 22 and pnpm 10.17.1 first.')
    metadata = json.loads(run([cargo, 'metadata', '--no-deps', '--format-version', '1', '--locked'], env, True))
    versions = {p['version'] for p in metadata['packages'] if p['name'] in ('ergent-server', 'ergent-agent')}
    if len(versions) != 1:
        parser.error('Server and Agent must have the same version.')
    version = versions.pop()
    if args.expect_version and args.expect_version.removeprefix('v') != version:
        parser.error(f'Expected {args.expect_version}, but Cargo version is {version}.')
    triple = TARGETS[target]
    # Isolate release builds from running development services and inherited target dirs.
    env['CARGO_TARGET_DIR'] = str(ROOT / 'target/package')
    if target.startswith('macos'):
        env['MACOSX_DEPLOYMENT_TARGET'] = '11.0'
    run([pnpm, 'install', '--frozen-lockfile'], env)
    run([pnpm, 'build'], env)
    run([cargo, 'build', '--locked', '--release', '--target', triple,
         '-p', 'ergent-server', '-p', 'ergent-agent'], env)
    web = ROOT / 'apps/web/dist'
    if not (web / 'index.html').is_file():
        raise RuntimeError('Frontend build did not produce index.html')
    suffix = '.exe' if target.startswith('windows') else ''
    out = args.out_dir.resolve()
    out.mkdir(parents=True, exist_ok=True)
    name = f'ergent-{version}-{target}'
    with tempfile.TemporaryDirectory(prefix='ergent-package-') as tmp:
        bundle = Path(tmp) / name
        bundle.mkdir()
        for binary in ('ergent-server', 'ergent-agent'):
            dest = bundle / (binary + suffix)
            shutil.copy2(Path(env['CARGO_TARGET_DIR']) / triple / 'release' / dest.name, dest)
            # Execute the actual distributed binaries, not debug copies.
            subprocess.run([str(dest), '--help'], cwd=bundle, env=env, check=True, stdout=subprocess.DEVNULL)
        shutil.copytree(web, bundle / 'web')
        shutil.copy2(ROOT / 'scripts/release/README.md', bundle / 'README.md')
        launcher = 'start-server.cmd' if suffix else 'start-server.sh'
        shutil.copy2(ROOT / 'scripts/release' / launcher, bundle / launcher)
        if not suffix:
            (bundle / launcher).chmod(0o755)
        commit = run(['git', 'rev-parse', 'HEAD'], env, True).strip()
        dirty = bool(run(['git', 'status', '--porcelain', '--untracked-files=normal'], env, True).strip())
        manifest = {'version': version, 'target': triple, 'platform': target,
                    'commit': commit, 'dirty': dirty,
                    'files': {p.relative_to(bundle).as_posix(): sha256(p)
                              for p in sorted(bundle.rglob('*')) if p.is_file()}}
        (bundle / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n', encoding='utf-8')
        extension = '.zip' if suffix else '.tar.gz'
        archive = out / (name + extension)
        pending = out / (archive.name + '.tmp')
        try:
            if suffix:
                with zipfile.ZipFile(pending, 'w', zipfile.ZIP_DEFLATED) as z:
                    for path in sorted(bundle.rglob('*')):
                        if path.is_file():
                            z.write(path, path.relative_to(bundle.parent).as_posix())
            else:
                with tarfile.open(pending, 'w:gz') as tar:
                    tar.add(bundle, arcname=name)
            pending.replace(archive)
        finally:
            pending.unlink(missing_ok=True)
        (out / (archive.name + '.sha256')).write_text(f'{sha256(archive)}  {archive.name}\n', encoding='utf-8')
    print(f'Packaged: {archive}')


if __name__ == '__main__':
    try:
        main()
    except (RuntimeError, subprocess.CalledProcessError) as error:
        sys.exit(str(error))
