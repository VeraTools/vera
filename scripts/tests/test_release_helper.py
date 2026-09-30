import os
import subprocess
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).resolve().parents[1] / 'release.sh'


class ReleaseHelperTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix='vera release ')
        self.addCleanup(self.temporary.cleanup)
        root = Path(self.temporary.name)
        self.origin = root / 'remote.git'
        self.repo = root / 'checkout'
        subprocess.run(['git', 'init', '--bare', str(self.origin)], check=True, capture_output=True)
        subprocess.run(['git', 'clone', str(self.origin), str(self.repo)], check=True, capture_output=True)
        self.git('config', 'user.email', 'fixture@example.invalid')
        self.git('config', 'user.name', 'Release Fixture')
        self.git('checkout', '-b', 'master')
        (self.repo / 'source').write_text('fixture')
        self.git('add', 'source')
        self.git('commit', '-m', 'test fixture')
        self.git('push', '-u', 'origin', 'master')
        tools = root / 'tools'
        tools.mkdir()
        gh = tools / 'gh'
        gh.write_text('#!/bin/sh\necho "${FIXTURE_CI_GREEN:-true}"\n')
        gh.chmod(0o755)
        self.env = {**os.environ, 'PATH': str(tools) + os.pathsep + os.environ['PATH']}
        self.commit = self.git('rev-parse', 'HEAD').stdout.strip()

    def git(self, *args):
        return subprocess.run(['git', *args], cwd=self.repo, text=True, check=True, capture_output=True)

    def release(self, version='2.0.0'):
        return subprocess.run(['bash', str(SCRIPT), version], cwd=self.repo, env=self.env,
                              text=True, capture_output=True, timeout=30)

    def assert_rejected(self, message):
        result = self.release()
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn(message, result.stdout + result.stderr)
        tags = subprocess.check_output(['git', '--git-dir', str(self.origin), 'tag'], text=True)
        self.assertNotIn('v2.0.0', tags)
        return result

    def test_green_synchronized_master_tags_exact_commit(self):
        result = self.release()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        tagged = subprocess.check_output(
            ['git', '--git-dir', str(self.origin), 'rev-parse', 'refs/tags/v2.0.0'], text=True).strip()
        self.assertEqual(tagged, self.commit)
        self.assertNotEqual(self.release().returncode, 0)

    def test_hyphenated_prerelease_tag(self):
        result = self.release('2.0.0-rc-1')
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(self.git('rev-parse', 'v2.0.0-rc-1').stdout.strip(), self.commit)

    def test_dirty_wrong_branch_and_failed_ci_do_not_tag(self):
        (self.repo / 'source').write_text('dirty')
        self.assert_rejected('uncommitted changes')
        self.git('checkout', '--', 'source')
        self.git('checkout', '-b', 'feature')
        self.assert_rejected('release from master')
        self.git('checkout', 'master')
        self.env['FIXTURE_CI_GREEN'] = 'false'
        self.assert_rejected('CI must pass')

    def test_local_ahead_and_remote_ahead_do_not_tag(self):
        (self.repo / 'source').write_text('ahead')
        self.git('commit', '-am', 'ahead')
        self.assert_rejected('local master must match')
        self.git('push', 'origin', 'master')
        self.git('reset', '--hard', self.commit)
        self.assert_rejected('local master must match')

    def test_detached_invalid_version_and_remote_tag_do_not_tag(self):
        self.git('checkout', '--detach')
        self.assert_rejected('release from master')
        self.git('checkout', 'master')
        invalid = self.release('bad-version')
        self.assertNotEqual(invalid.returncode, 0)
        self.assertIn('not a valid semver version', invalid.stdout)
        self.git('tag', 'v2.0.0')
        self.git('push', 'origin', 'refs/tags/v2.0.0')
        self.git('tag', '-d', 'v2.0.0')
        result = self.release()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('tag v2.0.0 already exists', result.stdout)
        self.assertEqual(self.git('rev-parse', 'refs/tags/v2.0.0').stdout.strip(), self.commit)


if __name__ == '__main__':
    unittest.main()
