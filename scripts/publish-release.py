#!/usr/bin/env python3
"""Publish a verified six-platform bundle without replacing existing assets."""
import argparse
import importlib.util
import json
from pathlib import Path
import re
import subprocess
import sys
import tempfile
from urllib.parse import quote

from package import TARGETS, sha256


class PublishError(RuntimeError):
    pass


class GitHub:
    def __init__(self, repository):
        if not re.fullmatch(r'[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+', repository):
            raise PublishError('Expected repository in owner/name form')
        self.repository = repository
        self.base = f'repos/{repository}'

    def command(self, *args):
        return subprocess.run(['gh', *map(str, args)], text=True, capture_output=True)

    def api(self, path, missing_ok=False):
        result = self.command('api', '--include', '--method', 'GET', f'{self.base}/{path}')
        match = re.match(r'HTTP/\S+\s+(\d+)\b', result.stdout)
        status = int(match[1]) if match else None
        if status == 404 and missing_ok:
            return None
        if result.returncode or status is None or not 200 <= status < 300:
            raise PublishError(f'GitHub API {path} failed (HTTP {status or "unknown"}): '
                               f'{result.stderr.strip()}')
        try:
            return json.loads(re.split(r'\r?\n\r?\n', result.stdout, maxsplit=1)[1])
        except (IndexError, json.JSONDecodeError) as error:
            raise PublishError(f'Invalid GitHub API response for {path}') from error

    def require_tag(self, tag, commit):
        obj = self.api(f'git/ref/tags/{quote(tag, safe="")}')['object']
        for _ in range(10):
            if obj['type'] == 'commit':
                if obj['sha'] != commit:
                    raise PublishError('Tag changed or does not match the verified package commit')
                return
            if obj['type'] != 'tag':
                break
            obj = self.api(f'git/tags/{obj["sha"]}')['object']
        raise PublishError('Tag does not resolve to a commit')

    def release(self, tag):
        return self.api(f'releases/tags/{quote(tag, safe="")}', missing_ok=True)

    def assets(self, release_id):
        assets = []
        page = 1
        while True:
            batch = self.api(f'releases/{release_id}/assets?per_page=100&page={page}')
            assets.extend(batch)
            if len(batch) < 100:
                return assets
            page += 1

    def create_draft(self, tag):
        result = self.command('release', 'create', tag, '--repo', self.repository,
                              '--verify-tag', '--draft', '--title', f'Ergent {tag}', '--generate-notes')
        if result.returncode:
            # No retry: another publisher may have created a release concurrently.
            raise PublishError(f'Creating draft failed: {result.stderr.strip()}')

    def asset_hash(self, tag, asset):
        digest = asset.get('digest') or ''
        if re.fullmatch(r'sha256:[0-9a-fA-F]{64}', digest):
            return digest.split(':', 1)[1].lower()
        # Older GitHub assets may not expose a digest. Compare the actual bytes.
        with tempfile.TemporaryDirectory(prefix='ergent-release-asset-') as tmp:
            result = self.command('release', 'download', tag, '--repo', self.repository,
                                  '--pattern', asset['name'], '--dir', tmp)
            if result.returncode:
                raise PublishError(f'Cannot verify existing asset {asset["name"]}: '
                                   f'{result.stderr.strip()}')
            path = Path(tmp) / asset['name']
            if not path.is_file() or path.is_symlink():
                raise PublishError(f'Existing asset download missing: {asset["name"]}')
            return sha256(path)

    def upload(self, tag, path):
        # Deliberately never use --clobber, delete-asset, or release edit.
        result = self.command('release', 'upload', tag, path, '--repo', self.repository)
        if result.returncode:
            raise PublishError(f'Upload failed for {path.name}: {result.stderr.strip()}')


def verified_files(directory, tag, commit):
    spec = importlib.util.spec_from_file_location('verify_release', Path(__file__).with_name('verify-release.py'))
    verifier = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(verifier)
    archives = sorted([*directory.glob('ergent-*.zip'), *directory.glob('ergent-*.tar.gz')])
    if len(archives) != len(TARGETS):
        raise PublishError('A release requires exactly six platform archives')
    platforms, checksum_lines, files = [], [], {}
    for path in archives:
        sidecar = path.with_name(path.name + '.sha256')
        if any(item.is_symlink() or not item.is_file() for item in (path, sidecar)):
            raise PublishError('Release assets must be regular files')
        manifest, checksum = verifier.verify(path)
        if (manifest['version'] != tag.removeprefix('v') or manifest['commit'] != commit
                or manifest['dirty'] is not False):
            raise PublishError('Package version, commit or clean-worktree state does not match the tag')
        platforms.append(manifest['platform'])
        checksum_lines.append(checksum)
        files[path.name] = path
        files[sidecar.name] = sidecar
    if len(set(platforms)) != len(TARGETS) or set(platforms) != set(TARGETS):
        raise PublishError('A release requires all six distinct platforms')
    sums = directory / 'SHA256SUMS'
    if sums.is_symlink() or not sums.is_file():
        raise PublishError('SHA256SUMS must be a regular file')
    if sums.read_text(encoding='utf-8') != ''.join(checksum_lines):
        raise PublishError('SHA256SUMS does not match the verified archives')
    files[sums.name] = sums
    if set(directory.iterdir()) != set(files.values()):
        raise PublishError('Unexpected files in release directory')
    if any(path.is_symlink() or not path.is_file() for path in files.values()):
        raise PublishError('Release assets must be regular files')
    return files


def missing_assets(github, tag, release, files, hashes):
    existing = {}
    for asset in github.assets(release['id']):
        name = asset['name']
        if name not in files:
            continue  # Preserve unrelated assets, too.
        if name in existing:
            raise PublishError(f'Duplicate existing asset: {name}')
        if asset.get('state') != 'uploaded' or asset['size'] != files[name].stat().st_size:
            raise PublishError(f'Existing asset is incomplete or conflicts: {name}')
        if github.asset_hash(tag, asset) != hashes[name]:
            raise PublishError(f'Existing asset hash conflicts: {name}')
        existing[name] = asset
    return sorted(set(files) - set(existing))


def publish(github, directory, tag, commit):
    if not re.fullmatch(r'v[0-9][A-Za-z0-9._-]*', tag) or not re.fullmatch(r'[0-9a-f]{40}', commit):
        raise PublishError('Expected a version tag and a full lowercase commit SHA')
    files = verified_files(directory, tag, commit)
    hashes = {name: sha256(path) for name, path in files.items()}
    github.require_tag(tag, commit)
    release = github.release(tag)
    if release is None:
        github.require_tag(tag, commit)
        github.create_draft(tag)
        release = github.release(tag)
    if release is None or release['tag_name'] != tag:
        raise PublishError('Release is missing or uses an unexpected tag')
    missing = missing_assets(github, tag, release, files, hashes)
    for name in missing:
        github.require_tag(tag, commit)
        current = github.release(tag)
        if current is None or current['id'] != release['id']:
            raise PublishError('Release changed during publication; stopped without replacing assets')
        if sha256(files[name]) != hashes[name]:
            raise PublishError(f'Local asset changed after verification: {name}')
        github.upload(tag, files[name])
        print(f'Uploaded {name}', flush=True)
    github.require_tag(tag, commit)
    final = github.release(tag)
    if final is None or final['id'] != release['id']:
        raise PublishError('Release changed during final verification')
    if missing_assets(github, tag, final, files, hashes):
        raise PublishError('Release is still missing verified assets')
    for name in sorted(files):
        print(f'SHA256 {hashes[name]}  {name}', flush=True)
    print(f'Verified {len(files)} release assets; uploaded {len(missing)}, '
          f'kept {len(files) - len(missing)} identical assets.')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('directory', type=Path)
    parser.add_argument('--repository', required=True)
    parser.add_argument('--tag', required=True)
    parser.add_argument('--commit', required=True)
    args = parser.parse_args()
    publish(GitHub(args.repository), args.directory, args.tag, args.commit)


if __name__ == '__main__':
    try:
        main()
    except (PublishError, OSError, ValueError, KeyError) as error:
        sys.exit(str(error))
