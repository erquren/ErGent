"""Offline version checks; never build binaries or install dependencies."""
import contextlib
import io
import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest import mock


SCRIPTS = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SCRIPTS))
import package


def metadata(server='1.2.0', agent='1.2.0'):
    return {'packages': [
        {'name': name, 'version': version}
        for name, version in [('ergent-server', server), ('ergent-agent', agent)]
        if version is not None
    ]}


class VersionTests(unittest.TestCase):
    def test_matching_versions_accept_optional_version_or_tag(self):
        for expected in (None, '1.2.0', 'v1.2.0'):
            with self.subTest(expected=expected):
                self.assertEqual('1.2.0', package.validate_versions(metadata(), '1.2.0', expected))

    def test_web_mismatch_is_rejected_without_an_expected_tag(self):
        with self.assertRaisesRegex(ValueError, 'Web version is 1.1.0'):
            package.validate_versions(metadata(), '1.1.0')

    def test_server_and_agent_mismatch_is_rejected(self):
        with self.assertRaisesRegex(ValueError, 'Server and Agent'):
            package.validate_versions(metadata(agent='1.1.0'), '1.2.0')

    def test_missing_binary_metadata_is_rejected(self):
        for versions in ((None, '1.2.0'), ('1.2.0', None), (None, None)):
            with self.subTest(versions=versions):
                with self.assertRaisesRegex(ValueError, 'both be present'):
                    package.validate_versions(metadata(*versions), '1.2.0')

    def test_wrong_or_uppercase_tag_is_rejected(self):
        for tag in ('v1.1.0', 'V1.2.0'):
            with self.subTest(tag=tag):
                with self.assertRaisesRegex(ValueError, 'Expected'):
                    package.validate_versions(metadata(), '1.2.0', tag)

    def test_version_checks_are_not_pinned_to_this_release(self):
        self.assertEqual('2.0.0', package.validate_versions(
            metadata('2.0.0', '2.0.0'), '2.0.0', 'v2.0.0'))

    def test_cli_rejects_mismatches_before_install_or_build(self):
        for web_version, tag in (('1.1.0', 'v1.2.0'), ('1.2.0', 'v1.1.0')):
            with self.subTest(web_version=web_version, tag=tag), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp)
                web = root / 'apps/web'
                web.mkdir(parents=True)
                (web / 'package.json').write_text(json.dumps({'version': web_version}), encoding='utf-8')
                with (mock.patch.object(package, 'ROOT', root),
                      mock.patch.object(package, 'native_target', return_value='windows-x86_64'),
                      mock.patch.object(package, 'build_env', return_value={'PATH': ''}),
                      mock.patch.object(package.shutil, 'which', return_value='fixture-tool'),
                      mock.patch.object(package, 'run', return_value=json.dumps(metadata())) as run,
                      mock.patch.object(sys, 'argv', ['package.py', '--expect-version', tag]),
                      contextlib.redirect_stderr(io.StringIO())):
                    with self.assertRaises(SystemExit) as raised:
                        package.main()
                    self.assertEqual(2, raised.exception.code)
                    self.assertEqual(1, run.call_count)
                    self.assertEqual('metadata', run.call_args.args[0][1])
                self.assertFalse((root / 'release').exists())


if __name__ == '__main__':
    unittest.main()
