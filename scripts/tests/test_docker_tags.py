"""Offline checks for Docker publication tag selection."""
from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from docker_tags import image_tag


class DockerTagTests(unittest.TestCase):
    def test_branch_publication_uses_project_version(self):
        for branch in ('main', 'release/next'):
            self.assertEqual('docker.io/erquren/ergent:1.2.0',
                             image_tag('1.2.0', '1.2.0', 'branch', branch))

    def test_matching_git_tag_uses_version_without_v(self):
        self.assertEqual('docker.io/erquren/ergent:2.0.0',
                         image_tag('2.0.0', '2.0.0', 'tag', 'v2.0.0'))

    def test_prerelease_keeps_its_own_tag(self):
        self.assertEqual('docker.io/erquren/ergent:2.0.0-rc.1',
                         image_tag('2.0.0-rc.1', '2.0.0-rc.1', 'tag', 'v2.0.0-rc.1'))

    def test_web_mismatch_rejected(self):
        with self.assertRaisesRegex(ValueError, 'does not match'):
            image_tag('1.2.0', '1.1.0', 'branch', 'main')

    def test_wrong_git_tag_rejected(self):
        for tag in ('v1.1.0', 'V1.2.0', '1.2.0'):
            with self.subTest(tag=tag), self.assertRaisesRegex(ValueError, 'Expected version tag'):
                image_tag('1.2.0', '1.2.0', 'tag', tag)

    def test_invalid_docker_tag_rejected(self):
        for version in ('', '1.2.0+build.1', '1.2.0\nlatest', '1' * 129):
            with self.subTest(version=version), self.assertRaisesRegex(ValueError, 'valid Docker tag'):
                image_tag(version, version, 'branch', 'main')

    def test_unexpected_ref_rejected(self):
        with self.assertRaisesRegex(ValueError, 'Unsupported Git ref type'):
            image_tag('1.2.0', '1.2.0', 'unknown', 'main')


if __name__ == '__main__':
    unittest.main()
