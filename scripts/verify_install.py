#!/usr/bin/env python3
"""Verify archive/checksums, extract safely, and exercise a fresh staged CLI install."""
import argparse
import hashlib
import json
import platform
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
import tempfile
ROOT = Path(__file__).resolve().parents[1]


def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('archive', type=Path)
    args = parser.parse_args()
    archive = args.archive.resolve(strict=True)
    expected = archive.with_suffix(archive.suffix + '.sha256').read_text().split()
    actual = digest(archive)
    if len(expected) != 2 or expected != [actual, archive.name]:
        raise ValueError('archive checksum or filename mismatch')
    with tempfile.TemporaryDirectory(prefix='nouli-install-') as directory:
        root = Path(directory)
        with tarfile.open(archive, 'r:gz') as tar:
            members = tar.getmembers()
            for member in members:
                path = Path(member.name)
                if not member.isfile() or path.is_absolute() or '..' in path.parts:
                    raise ValueError('unsupported archive entry')
                destination = root / path
                if destination.exists():
                    raise ValueError('duplicate archive entry')
                destination.parent.mkdir(parents=True, exist_ok=True)
                with tar.extractfile(member) as source, destination.open('xb') as target:
                    shutil.copyfileobj(source, target)
                destination.chmod(member.mode & 0o755)
        manifests = list(root.glob('*/MANIFEST.json'))
        if len(manifests) != 1:
            raise ValueError('expected exactly one package manifest')
        stage = manifests[0].parent
        manifest = json.loads(manifests[0].read_text())
        native = {('Darwin', 'arm64'): 'aarch64-apple-darwin',
                  ('Linux', 'aarch64'): 'aarch64-unknown-linux-gnu'}.get((platform.system(), platform.machine()))
        if manifest['target'] != native:
            raise SystemExit(f"Cannot run staged {manifest['target']} binary on this host; verify installation on its matching native target.")
        for relative, expected_hash in manifest['files'].items():
            path = stage / relative
            if not path.resolve().is_relative_to(stage.resolve()) or digest(path) != expected_hash:
                raise ValueError('package content hash mismatch')
        binary = stage / 'bin/nouli'
        if digest(binary) != manifest['binary_sha256']:
            raise ValueError('binary checksum mismatch')
        installed = root / 'install/bin/nouli'
        installed.parent.mkdir(parents=True)
        shutil.copyfile(binary, installed)
        installed.chmod(0o755)
        result = subprocess.run([sys.executable, str(ROOT / 'scripts/walkthrough.py'), '--binary', str(installed)], capture_output=True, check=True, text=True, timeout=60)
        print(json.dumps({'archive_sha256': actual, 'binary_sha256': digest(installed), 'target': manifest['target'], 'source_commit': manifest['source_commit'], 'walkthrough': json.loads(result.stdout), 'status': 'passed'}, sort_keys=True))


if __name__ == '__main__':
    main()
