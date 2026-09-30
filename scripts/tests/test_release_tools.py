import hashlib
import io
import json
import os
import subprocess
import sys
import tarfile
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from docker_publication import image_digest, main as docker_main, newest_stable
from generate_release_manifest import TARGETS, build_manifest, validate_manifest
from verify_release_archive import stage_binary
from preserve_release_assets import preserve_assets


class ReleaseToolsTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        for target, extension in TARGETS.items():
            (self.root / f'vera-{target}.{extension}').write_bytes(b'archive')

    def manifest(self):
        return build_manifest(self.root, 'v2.0.0', 'VeraTools/vera')

    def test_complete_six_target_manifest(self):
        manifest = self.manifest()
        self.assertEqual(set(manifest['assets']), set(TARGETS))
        validate_manifest(manifest, 'v2.0.0', 'VeraTools/vera')

    def test_missing_unknown_wrong_and_duplicate_archives(self):
        target = 'x86_64-pc-windows-msvc'
        expected = self.root / f'vera-{target}.zip'
        expected.unlink()
        with self.assertRaises(ValueError):
            self.manifest()
        expected.write_bytes(b'archive')
        for extra in ['vera-made-up.zip', f'vera-{target}.tar.gz', 'unrelated.zip']:
            with self.subTest(extra=extra):
                path = self.root / extra
                path.write_bytes(b'archive')
                with self.assertRaises(ValueError):
                    self.manifest()
                path.unlink()

    def test_manifest_identity_and_checksum_fields(self):
        for key, value in [('tag', 'v1.4.2'), ('version', '1.4.2'), ('repo', 'other/repo')]:
            manifest = self.manifest()
            manifest[key] = value
            with self.assertRaises(ValueError):
                validate_manifest(manifest, 'v2.0.0', 'VeraTools/vera')
        for key, value in [('sha256', 'abc'), ('size', 0), ('size', True), ('archive', 'other.zip')]:
            manifest = self.manifest()
            manifest['assets']['x86_64-unknown-linux-gnu'][key] = value
            with self.assertRaises(ValueError):
                validate_manifest(manifest, 'v2.0.0', 'VeraTools/vera')

    def bundle(self, extras=()):
        archive = self.root / 'vera-x86_64-unknown-linux-gnu.tar.gz'
        expected = 'vera-x86_64-unknown-linux-gnu/vera'
        binary = b'#!/bin/sh\necho vera 2.0.0\n'
        with tarfile.open(archive, 'w:gz') as bundle:
            info = tarfile.TarInfo(expected)
            info.size = len(binary)
            bundle.addfile(info, io.BytesIO(binary))
            for name, kind in extras:
                info = tarfile.TarInfo(name)
                info.type = kind
                if kind == tarfile.SYMTYPE:
                    info.linkname = '/outside'
                bundle.addfile(info, io.BytesIO(b''))
        manifest = self.manifest()
        path = self.root / 'release-manifest.json'
        path.write_text(json.dumps(manifest))
        return path, archive, binary

    def test_verified_binary_only_staged_atomically(self):
        manifest, archive, binary = self.bundle()
        output = self.root / 'dist/vera'
        stage_binary(manifest, archive, 'v2.0.0', 'VeraTools/vera', output)
        self.assertEqual(output.read_bytes(), binary)
        self.assertEqual(output.stat().st_mode & 0o777, 0o755)
        self.assertEqual(list(output.parent.iterdir()), [output])

    def test_hash_size_and_unsafe_layout_leave_existing_binary(self):
        output = self.root / 'dist/vera'
        output.parent.mkdir()
        output.write_bytes(b'existing')
        for extras in [
            [('vera-x86_64-unknown-linux-gnu/vera', tarfile.REGTYPE)],
            [('../outside', tarfile.REGTYPE)],
            [('/outside', tarfile.REGTYPE)],
            [('vera-x86_64-unknown-linux-gnu/link', tarfile.SYMTYPE)],
        ]:
            with self.subTest(extras=extras):
                manifest, archive, _ = self.bundle(extras)
                with self.assertRaises(ValueError):
                    stage_binary(manifest, archive, 'v2.0.0', 'VeraTools/vera', output)
                self.assertEqual(output.read_bytes(), b'existing')
        for mutation in ['same-size', 'extra-byte']:
            manifest, archive, _ = self.bundle()
            data = archive.read_bytes()
            archive.write_bytes(bytes([data[0] ^ 1]) + data[1:] if mutation == 'same-size' else data + b'x')
            with self.assertRaises(ValueError):
                stage_binary(manifest, archive, 'v2.0.0', 'VeraTools/vera', output)
            self.assertEqual(output.read_bytes(), b'existing')

    def test_publication_retry_never_replaces_different_existing_assets(self):
        manifest = self.manifest()
        (self.root / 'release-manifest.json').write_text(json.dumps(manifest))
        assets = [{'name': asset['archive'], 'size': asset['size'],
                   'digest': 'sha256:' + asset['sha256']} for asset in manifest['assets'].values()]
        result = subprocess.CompletedProcess([], 0, json.dumps({'assets': assets}), '')
        with patch('preserve_release_assets.subprocess.run', return_value=result):
            preserve_assets(self.root, 'v2.0.0', 'VeraTools/vera')
        assets[0]['digest'] = 'sha256:' + '0' * 64
        result.stdout = json.dumps({'assets': assets})
        with patch('preserve_release_assets.subprocess.run', return_value=result):
            with self.assertRaises(ValueError):
                preserve_assets(self.root, 'v2.0.0', 'VeraTools/vera')
        for error, exception in [('gh: Not Found (HTTP 404)', None), ('gh: Forbidden (HTTP 403)', RuntimeError)]:
            result = subprocess.CompletedProcess([], 1, '', error)
            with patch('preserve_release_assets.subprocess.run', return_value=result):
                if exception:
                    with self.assertRaises(exception):
                        preserve_assets(self.root, 'v2.0.0', 'VeraTools/vera')
                else:
                    preserve_assets(self.root, 'v2.0.0', 'VeraTools/vera')

    def test_newest_stable_is_numeric_and_ignores_drafts_and_prereleases(self):
        releases = [
            {'tag_name': 'v9.0.0'}, {'tag_name': 'v10.0.0'},
            {'tag_name': 'v11.0.0', 'draft': True},
            {'tag_name': 'v12.0.0', 'prerelease': True},
            {'tag_name': 'v13.0.0-rc.1'}, {'tag_name': 'v99.01.0'},
        ]
        self.assertEqual(newest_stable(releases), 'v10.0.0')
        self.assertIsNone(newest_stable([]))

    def test_registry_missing_is_distinct_from_auth_and_network_failure(self):
        for error in ['ERROR: image: not found', 'manifest unknown']:
            result = subprocess.CompletedProcess([], 1, '', error)
            with patch('docker_publication.subprocess.run', return_value=result):
                self.assertIsNone(image_digest('image:tag'))
        for error in ['unauthorized', 'connection timed out']:
            result = subprocess.CompletedProcess([], 1, '', error)
            with patch('docker_publication.subprocess.run', return_value=result):
                with self.assertRaises(RuntimeError):
                    image_digest('image:tag')

    def test_existing_version_is_not_rebuilt_and_older_release_is_not_promoted(self):
        output = self.root / 'output'
        digest = 'sha256:' + hashlib.sha256(b'image').hexdigest()
        args = ['tool', 'check', 'VeraTools/vera', 'v1.4.2', 'ghcr.io/veratools/vera', 'cpu']
        with patch.object(sys, 'argv', args), patch.dict(os.environ, GITHUB_OUTPUT=str(output)), \
                patch('docker_publication.image_digest', return_value=digest):
            docker_main()
        self.assertIn('build=false', output.read_text())
        pages = json.dumps([[{'tag_name': 'v1.4.2'}, {'tag_name': 'v2.0.0'}]])
        args[1] = 'promote'
        with patch.object(sys, 'argv', args), patch('docker_publication.image_digest', return_value=digest), \
                patch('release_versions.subprocess.check_output', return_value=pages), \
                patch('docker_publication.subprocess.run') as run:
            docker_main()
            run.assert_not_called()
            args[3] = 'v2.0.0'
            docker_main()
            command = run.call_args.args[0]
            self.assertIn('ghcr.io/veratools/vera@' + digest, command)
            self.assertIn('ghcr.io/veratools/vera:cpu', command)


if __name__ == '__main__':
    unittest.main()
