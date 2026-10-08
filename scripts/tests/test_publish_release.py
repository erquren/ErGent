"""Offline publication safety tests; never invoke gh or contact GitHub."""
import contextlib
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile
import unittest
from unittest import mock
import zipfile


SCRIPTS = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SCRIPTS))
SPEC = importlib.util.spec_from_file_location('publish_release', SCRIPTS / 'publish-release.py')
publisher = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(publisher)
COMMIT = 'a' * 40
TAG = 'v1.0.0'


def digest(data):
    return hashlib.sha256(data).hexdigest()


def make_archive(directory, platform, *, version='1.0.0', commit=COMMIT,
                 dirty=False, wrong_file_hash=False, wrong_target=False,
                 duplicate_entry=False):
    windows = platform.startswith('windows')
    suffix = '.exe' if windows else ''
    extension = '.zip' if windows else '.tar.gz'
    root = f'ergent-{version}-{platform}'
    files = {
        'ergent-server' + suffix: b'fixture server\n',
        'ergent-agent' + suffix: b'fixture agent\n',
        'web/index.html': b'<html>Fixture UI</html>\n',
        'README.md': b'Fixture readme\n',
        'start-server.cmd' if windows else 'start-server.sh': b'fixture launcher\n',
    }
    manifest = {
        'platform': platform,
        'target': 'wrong-target' if wrong_target else publisher.TARGETS[platform],
        'version': version,
        'commit': commit,
        'dirty': dirty,
        'files': {name: digest(data) for name, data in files.items()},
    }
    if wrong_file_hash:
        manifest['files']['README.md'] = '0' * 64
    files['manifest.json'] = json.dumps(manifest).encode()
    path = directory / (root + extension)
    if windows:
        with zipfile.ZipFile(path, 'w', zipfile.ZIP_DEFLATED) as archive:
            for name, data in files.items():
                archive.writestr(root + '/' + name, data)
            if duplicate_entry:
                # Duplicate entries are deliberately invalid test inputs.
                import warnings
                with warnings.catch_warnings():
                    warnings.simplefilter('ignore', UserWarning)
                    archive.writestr(root + '/README.md', files['README.md'])
    else:
        with tarfile.open(path, 'w:gz') as archive:
            for name, data in files.items():
                member = tarfile.TarInfo(root + '/' + name)
                member.size = len(data)
                archive.addfile(member, io.BytesIO(data))
            if duplicate_entry:
                member = tarfile.TarInfo(root + '/README.md')
                member.size = len(files['README.md'])
                archive.addfile(member, io.BytesIO(files['README.md']))
    sidecar = path.with_name(path.name + '.sha256')
    sidecar.write_text(f'{digest(path.read_bytes())}  {path.name}\n', encoding='utf-8')
    return path


def refresh_sums(directory):
    archives = sorted([*directory.glob('ergent-*.zip'), *directory.glob('ergent-*.tar.gz')])
    (directory / 'SHA256SUMS').write_text(''.join(
        path.with_name(path.name + '.sha256').read_text(encoding='utf-8')
        for path in archives), encoding='utf-8')


def fixture_files(directory):
    for platform in publisher.TARGETS:
        make_archive(directory, platform)
    refresh_sums(directory)
    return {path.name: path for path in directory.iterdir()}


class FakeGitHub(publisher.GitHub):
    """Run production API parsing and publication methods over in-memory gh I/O."""

    def __init__(self, files, *, existing=True, names=()):
        super().__init__('erquren/ErGent')
        self.files = files
        self.calls = []
        self.release_value = {
            'id': 321, 'tag_name': TAG, 'draft': False, 'prerelease': False,
            'name': 'Keep this title', 'body': 'Keep these custom release notes',
        } if existing else None
        self.remote_assets = []
        self.remote_bytes = {}
        self.tag_commit = COMMIT
        self.annotated = False
        self.tag_calls = 0
        self.before_tag = None
        self.before_upload = None
        self.api_errors = {}
        self.download_error = False
        self.upload_error = False
        self.drop_upload = False
        self.create_error = False
        for name in names:
            self.add_asset(name, files[name].read_bytes())

    def add_asset(self, name, data, *, state='uploaded', include_digest=True):
        asset = {'id': 1000 + len(self.remote_assets), 'name': name,
                 'size': len(data), 'state': state}
        if include_digest:
            asset['digest'] = 'sha256:' + digest(data)
        self.remote_assets.append(asset)
        self.remote_bytes[name] = data
        return asset

    def result(self, args, *, status=200, payload=None, code=0, stderr=''):
        return subprocess.CompletedProcess(args, code,
            f'HTTP/2.0 {status} Fixture\r\nx-fixture: true\r\n\r\n' + json.dumps(payload), stderr)

    def command(self, *args):
        args = tuple(map(str, args))
        self.calls.append(args)
        if args[0] == 'api':
            if args[1:4] != ('--include', '--method', 'GET'):
                raise AssertionError(f'Unexpected API mutation: {args}')
            path = args[4].removeprefix(self.base + '/')
            if path in self.api_errors:
                status = self.api_errors[path]
                if status is None:
                    return subprocess.CompletedProcess(args, 1, '', 'network error')
                return self.result(args, status=status, code=1, payload={'message': 'failure'})
            if path.startswith('git/ref/tags/'):
                self.tag_calls += 1
                if self.before_tag:
                    self.before_tag(self)
                return self.result(args, payload={'object': {
                    'type': 'tag' if self.annotated else 'commit',
                    'sha': 'b' * 40 if self.annotated else self.tag_commit}})
            if path == 'git/tags/' + 'b' * 40:
                return self.result(args, payload={'object': {'type': 'commit', 'sha': self.tag_commit}})
            if path.startswith('releases/tags/'):
                return self.result(args, status=200 if self.release_value else 404,
                    code=0 if self.release_value else 1, payload=self.release_value)
            if path.startswith('releases/321/assets?'):
                page = int(path.split('page=')[-1])
                return self.result(args, payload=self.remote_assets[(page - 1) * 100:page * 100])
            raise AssertionError(f'Unexpected API request: {args}')
        if args[:2] == ('release', 'create'):
            if self.create_error:
                return subprocess.CompletedProcess(args, 1, '', 'create failed')
            if self.release_value is not None:
                raise AssertionError('Tried to recreate an existing release')
            if '--draft' not in args or '--verify-tag' not in args:
                raise AssertionError('Release must be created as a draft with an existing tag')
            self.release_value = {'id': 321, 'tag_name': TAG, 'draft': True,
                                  'prerelease': False, 'name': 'Ergent ' + TAG,
                                  'body': 'Generated notes'}
            return subprocess.CompletedProcess(args, 0, 'created', '')
        if args[:2] == ('release', 'download'):
            if self.download_error:
                return subprocess.CompletedProcess(args, 1, '', 'download failed')
            name = args[args.index('--pattern') + 1]
            directory = Path(args[args.index('--dir') + 1])
            (directory / name).write_bytes(self.remote_bytes[name])
            return subprocess.CompletedProcess(args, 0, '', '')
        if args[:2] == ('release', 'upload'):
            if self.before_upload:
                self.before_upload(self)
            if self.upload_error:
                return subprocess.CompletedProcess(args, 1, '', 'upload failed')
            path = Path(args[3])
            if any(asset['name'] == path.name for asset in self.remote_assets):
                return subprocess.CompletedProcess(args, 1, '', 'already exists')
            if not self.drop_upload:
                self.add_asset(path.name, path.read_bytes())
            return subprocess.CompletedProcess(args, 0, '', '')
        raise AssertionError(f'Forbidden or unexpected command: {args}')

    @property
    def uploads(self):
        return [args for args in self.calls if args[:2] == ('release', 'upload')]

    @property
    def creates(self):
        return [args for args in self.calls if args[:2] == ('release', 'create')]


class PublishTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix='ergent-publish-test-')
        self.addCleanup(self.tmp.cleanup)
        self.directory = Path(self.tmp.name)
        self.files = fixture_files(self.directory)

    def publish(self, github, *, tag=TAG, commit=COMMIT):
        with contextlib.redirect_stdout(io.StringIO()):
            publisher.publish(github, self.directory, tag, commit)

    def assert_rejected_without_upload(self, github, *, error=(publisher.PublishError, ValueError, OSError, KeyError)):
        with self.assertRaises(error):
            self.publish(github)
        self.assertEqual([], github.uploads)
        self.assertEqual([], github.creates)

    def test_fixture_has_thirteen_verified_files(self):
        self.assertEqual(set(self.files), set(publisher.verified_files(self.directory, TAG, COMMIT)))
        self.assertEqual(13, len(self.files))

    def test_absent_release_created_draft(self):
        github = FakeGitHub(self.files, existing=False)
        self.publish(github)
        self.assertEqual(1, len(github.creates))
        self.assertEqual(13, len(github.uploads))
        self.assertTrue(github.release_value['draft'])
        self.assertEqual(set(self.files), set(github.remote_bytes))

    def test_existing_empty_release_keeps_metadata_and_visibility(self):
        github = FakeGitHub(self.files)
        original = dict(github.release_value)
        self.publish(github)
        self.assertEqual(original, github.release_value)
        self.assertEqual([], github.creates)
        self.assertEqual(13, len(github.uploads))

    def test_existing_partial_release_uploads_only_missing(self):
        present = sorted(self.files)[:5]
        github = FakeGitHub(self.files, names=present)
        self.publish(github)
        self.assertEqual(8, len(github.uploads))
        self.assertEqual(set(self.files) - set(present), {Path(args[3]).name for args in github.uploads})

    def test_existing_full_release_is_noop(self):
        github = FakeGitHub(self.files, names=self.files)
        self.publish(github)
        self.assertEqual([], github.creates)
        self.assertEqual([], github.uploads)

    def test_existing_draft_and_prerelease_are_preserved(self):
        github = FakeGitHub(self.files)
        github.release_value.update(draft=True, prerelease=True)
        original = dict(github.release_value)
        self.publish(github)
        self.assertEqual(original, github.release_value)

    def test_unrelated_assets_are_preserved_with_pagination(self):
        github = FakeGitHub(self.files, names=list(self.files)[:1])
        for number in range(101):
            github.add_asset(f'old-unrelated-{number}.txt', b'unchanged')
        unrelated = {name: data for name, data in github.remote_bytes.items() if name.startswith('old-')}
        self.publish(github)
        self.assertEqual(unrelated, {name: github.remote_bytes[name] for name in unrelated})
        self.assertTrue(any('page=2' in args[-1] for args in github.calls))

    def test_same_size_conflict_fails_before_any_upload(self):
        github = FakeGitHub(self.files)
        name = sorted(self.files)[-1]
        original = self.files[name].read_bytes()
        github.add_asset(name, b'!' + original[1:])
        self.assert_rejected_without_upload(github)

    def test_size_conflict_fails_before_any_upload(self):
        github = FakeGitHub(self.files)
        github.add_asset(sorted(self.files)[-1], b'wrong size')
        self.assert_rejected_without_upload(github)

    def test_missing_digest_downloads_and_checks_bytes(self):
        github = FakeGitHub(self.files)
        name = sorted(self.files)[-1]
        github.add_asset(name, self.files[name].read_bytes(), include_digest=False)
        self.publish(github)
        self.assertEqual(12, len(github.uploads))
        downloads = [args for args in github.calls if args[:2] == ('release', 'download')]
        self.assertEqual(2, len(downloads))  # Initial and final verification.

    def test_missing_digest_conflict_fails_before_any_upload(self):
        github = FakeGitHub(self.files)
        name = sorted(self.files)[-1]
        original = self.files[name].read_bytes()
        github.add_asset(name, b'!' + original[1:], include_digest=False)
        self.assert_rejected_without_upload(github)

    def test_missing_digest_download_failure_is_fatal(self):
        github = FakeGitHub(self.files)
        name = sorted(self.files)[-1]
        github.add_asset(name, self.files[name].read_bytes(), include_digest=False)
        github.download_error = True
        self.assert_rejected_without_upload(github)

    def test_unrecognized_digest_uses_download_fallback(self):
        github = FakeGitHub(self.files, names=self.files)
        github.remote_assets[0]['digest'] = 'sha512:unsupported'
        self.publish(github)
        self.assertEqual([], github.uploads)
        self.assertTrue(any(args[:2] == ('release', 'download') for args in github.calls))

    def test_duplicate_expected_asset_fails_before_any_upload(self):
        github = FakeGitHub(self.files)
        name = sorted(self.files)[-1]
        github.add_asset(name, self.files[name].read_bytes())
        github.add_asset(name, self.files[name].read_bytes())
        self.assert_rejected_without_upload(github)

    def test_incomplete_expected_asset_fails_before_any_upload(self):
        github = FakeGitHub(self.files)
        name = sorted(self.files)[-1]
        github.add_asset(name, self.files[name].read_bytes(), state='starter')
        self.assert_rejected_without_upload(github)

    def test_auth_server_and_network_errors_do_not_create_release(self):
        for status in (401, 403, 429, 500, 502, None):
            with self.subTest(status=status):
                github = FakeGitHub(self.files, existing=False)
                github.api_errors['releases/tags/' + TAG] = status
                self.assert_rejected_without_upload(github)

    def test_missing_tag_does_not_create_release(self):
        github = FakeGitHub(self.files, existing=False)
        github.api_errors['git/ref/tags/' + TAG] = 404
        self.assert_rejected_without_upload(github)

    def test_initial_tag_mismatch_fails_without_mutations(self):
        github = FakeGitHub(self.files, existing=False)
        github.tag_commit = 'c' * 40
        self.assert_rejected_without_upload(github)

    def test_annotated_tag_is_resolved(self):
        github = FakeGitHub(self.files, names=self.files)
        github.annotated = True
        self.publish(github)
        self.assertEqual([], github.uploads)

    def test_tag_change_before_create_fails_without_mutations(self):
        github = FakeGitHub(self.files, existing=False)
        github.before_tag = lambda gh: setattr(gh, 'tag_commit', 'c' * 40) if gh.tag_calls == 2 else None
        self.assert_rejected_without_upload(github)

    def test_tag_change_before_first_upload_fails_without_mutations(self):
        github = FakeGitHub(self.files)
        github.before_tag = lambda gh: setattr(gh, 'tag_commit', 'c' * 40) if gh.tag_calls == 2 else None
        self.assert_rejected_without_upload(github)

    def test_tag_change_mid_upload_stops_further_uploads(self):
        github = FakeGitHub(self.files)
        github.before_upload = lambda gh: setattr(gh, 'tag_commit', 'c' * 40)
        with self.assertRaises(publisher.PublishError):
            self.publish(github)
        self.assertEqual(1, len(github.uploads))

    def test_release_change_before_upload_stops(self):
        github = FakeGitHub(self.files)
        def change_release(gh):
            if gh.tag_calls == 2:
                gh.release_value = dict(gh.release_value, id=999)
        github.before_tag = change_release
        self.assert_rejected_without_upload(github)

    def test_release_disappears_before_upload_stops(self):
        github = FakeGitHub(self.files)
        github.before_tag = lambda gh: setattr(gh, 'release_value', None) if gh.tag_calls == 2 else None
        self.assert_rejected_without_upload(github)

    def test_failed_create_is_not_retried(self):
        github = FakeGitHub(self.files, existing=False)
        github.create_error = True
        with self.assertRaises(publisher.PublishError):
            self.publish(github)
        self.assertEqual(1, len(github.creates))
        self.assertEqual([], github.uploads)

    def test_failed_upload_is_not_retried_or_overwritten(self):
        github = FakeGitHub(self.files)
        github.upload_error = True
        with self.assertRaises(publisher.PublishError):
            self.publish(github)
        self.assertEqual(1, len(github.uploads))

    def test_final_verification_detects_missing_uploads(self):
        github = FakeGitHub(self.files)
        github.drop_upload = True
        with self.assertRaisesRegex(publisher.PublishError, 'still missing'):
            self.publish(github)

    def test_local_asset_changed_after_verification_is_not_uploaded(self):
        github = FakeGitHub(self.files)
        name = sorted(self.files)[0]
        def mutate(gh):
            if gh.tag_calls == 2:
                self.files[name].write_bytes(b'changed after validation')
        github.before_tag = mutate
        self.assert_rejected_without_upload(github)

    def test_archive_wrong_version_is_rejected(self):
        for path in self.directory.iterdir():
            path.unlink()
        for platform in publisher.TARGETS:
            make_archive(self.directory, platform, version='1.0.1')
        refresh_sums(self.directory)
        self.assert_rejected_without_upload(FakeGitHub(self.files))

    def test_manifest_commit_dirty_target_and_file_hash_are_rejected(self):
        for invalid in ({'commit': 'd' * 40}, {'dirty': True}, {'dirty': 0},
                        {'wrong_target': True}, {'wrong_file_hash': True}, {'duplicate_entry': True}):
            with self.subTest(invalid=invalid):
                make_archive(self.directory, 'linux-arm64', **invalid)
                refresh_sums(self.directory)
                self.assert_rejected_without_upload(FakeGitHub(self.files))
        make_archive(self.directory, 'linux-arm64')

    def test_archive_checksum_failure_is_rejected(self):
        path = next(self.directory.glob('*.tar.gz'))
        path.write_bytes(path.read_bytes() + b'corrupt')
        self.assert_rejected_without_upload(FakeGitHub(self.files))

    def test_sidecar_failure_is_rejected(self):
        next(self.directory.glob('*.sha256')).write_text('wrong\n', encoding='utf-8')
        self.assert_rejected_without_upload(FakeGitHub(self.files))

    def test_sums_failure_is_rejected(self):
        (self.directory / 'SHA256SUMS').write_text('wrong\n', encoding='utf-8')
        self.assert_rejected_without_upload(FakeGitHub(self.files))

    def test_missing_platform_is_rejected(self):
        next(self.directory.glob('*.tar.gz')).unlink()
        self.assert_rejected_without_upload(FakeGitHub(self.files))

    def test_extra_file_is_rejected(self):
        (self.directory / 'unreviewed.txt').write_text('extra', encoding='utf-8')
        self.assert_rejected_without_upload(FakeGitHub(self.files))

    def test_symlink_asset_is_rejected(self):
        path = self.directory / 'SHA256SUMS'
        data = path.read_bytes()
        with tempfile.TemporaryDirectory() as outside:
            target = Path(outside) / 'SHA256SUMS'
            target.write_bytes(data)
            path.unlink()
            path.symlink_to(target)
            self.assert_rejected_without_upload(FakeGitHub(self.files))

    def test_invalid_tag_and_commit_are_rejected(self):
        for tag, commit in [('1.0.0', COMMIT), ('v1.0.0/unsafe', COMMIT), (TAG, 'short'), (TAG, 'A' * 40)]:
            with self.subTest(tag=tag, commit=commit):
                github = FakeGitHub(self.files)
                with self.assertRaises(publisher.PublishError):
                    self.publish(github, tag=tag, commit=commit)
                self.assertEqual([], github.calls)

    def test_no_overwrite_delete_edit_or_tag_mutation_commands(self):
        github = FakeGitHub(self.files, existing=False)
        self.publish(github)
        for call in github.calls:
            self.assertNotIn('--clobber', call)
            self.assertNotIn('--force', call)
            self.assertNotIn('delete', call)
            self.assertNotIn('delete-asset', call)
            self.assertNotIn('edit', call)
            if call[0] == 'api':
                self.assertEqual('GET', call[3])


class ApiParsingTests(unittest.TestCase):
    def setUp(self):
        self.github = publisher.GitHub('erquren/ErGent')

    def test_only_http_404_is_absent(self):
        result = subprocess.CompletedProcess([], 1, 'HTTP/2.0 404 Not Found\n\n{}', 'not found')
        with mock.patch.object(self.github, 'command', return_value=result):
            self.assertIsNone(self.github.release(TAG))

    def test_stderr_404_without_http_status_is_error(self):
        result = subprocess.CompletedProcess([], 1, '', 'proxy network failure 404')
        with mock.patch.object(self.github, 'command', return_value=result):
            with self.assertRaises(publisher.PublishError):
                self.github.release(TAG)

    def test_bad_json_and_bad_status_are_errors(self):
        for stdout in ['HTTP/2.0 200 OK\r\n\r\nnot JSON', 'not HTTP\n\n{}', 'HTTP/2.0 200 OK']:
            with self.subTest(stdout=stdout):
                result = subprocess.CompletedProcess([], 0, stdout, '')
                with mock.patch.object(self.github, 'command', return_value=result):
                    with self.assertRaises(publisher.PublishError):
                        self.github.release(TAG)

    def test_failure_exit_with_200_is_error(self):
        result = subprocess.CompletedProcess([], 1, 'HTTP/2.0 200 OK\n\n{}', 'transfer failed')
        with mock.patch.object(self.github, 'command', return_value=result):
            with self.assertRaises(publisher.PublishError):
                self.github.release(TAG)

    def test_bad_repository_is_rejected(self):
        for repository in ('erquren', 'erquren/ErGent/path', '../x/y', 'owner/name?token=value'):
            with self.subTest(repository=repository):
                with self.assertRaises(publisher.PublishError):
                    publisher.GitHub(repository)


if __name__ == '__main__':
    unittest.main()
