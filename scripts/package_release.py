#!/usr/bin/env python3
"""Build a local native archive with exact contents, provenance, and available notices."""
import argparse
import gzip
import hashlib
import io
import json
from pathlib import Path
import shutil
import subprocess
import tarfile

ROOT = Path(__file__).resolve().parents[1]


def run(*arguments):
    return subprocess.run(arguments, cwd=ROOT, check=True, capture_output=True, text=True).stdout


def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def third_party(stage, metadata, workspace):
    packages = {package['id']: package for package in metadata['packages']}
    nodes = {node['id']: node for node in metadata['resolve']['nodes']}
    selected = set()
    pending = list(workspace)
    while pending:
        package = pending.pop()
        if package in selected:
            continue
        selected.add(package)
        pending.extend(dependency['pkg'] for dependency in nodes[package]['deps'])
    lines = ['# Third-party metadata and available notices', '',
             'Inventory from the locked native dependency graph. SPDX expressions are package metadata, not a legal determination. Available license/notice texts are preserved below `licenses/`. Nouli uses the MIT license in LICENSE.', '']
    for identifier in sorted(selected, key=lambda item: (packages[item]['name'], packages[item]['version'])):
        if identifier in workspace:
            continue
        package = packages[identifier]
        name = f"{package['name']}-{package['version']}"
        source = Path(package['manifest_path']).parent
        files = []
        declared_license = package.get('license_file')
        declared_path = (source / declared_license).resolve() if declared_license else None
        for candidate in sorted(source.rglob('*')):
            if not candidate.is_file() or not candidate.resolve().is_relative_to(source.resolve()):
                continue
            # Preserve available notices, including bundled C libraries and their
            # nested third-party texts, without copying source or credentials.
            if candidate.name.lower().startswith(('license', 'licence', 'copying', 'notice')) or candidate.resolve() == declared_path:
                relative = candidate.relative_to(source)
                destination = stage / 'licenses' / name / relative
                destination.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(candidate, destination)
                files.append(str(relative))
        lines += [f"## {name}", f"License metadata: {package.get('license') or 'not declared'}", f"Source: {package.get('source') or 'workspace/path'}", f"Notice files: {', '.join(files) or 'none found by inventory; owner review required'}", '']
    (stage / 'THIRD_PARTY_NOTICES.md').write_text('\n'.join(lines))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True, help='New output directory, must not exist')
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    verbose = run('rustc', '--version', '--verbose')
    host = next(line.split(': ', 1)[1] for line in verbose.splitlines() if line.startswith('host: '))
    if host not in ('aarch64-apple-darwin', 'aarch64-unknown-linux-gnu'):
        raise SystemExit('Packaging is verified only for native aarch64 macOS and GNU/Linux.')
    metadata = json.loads(run('cargo', 'metadata', '--locked', '--format-version', '1', '--filter-platform', host))
    workspace = set(metadata['workspace_members'])
    package = next(package for package in metadata['packages'] if package['name'] == 'nouli-cli' and package['id'] in workspace)
    version = package['version']
    subprocess.run(['cargo', 'build', '--workspace', '--release', '--locked', '--target', host], cwd=ROOT, check=True)
    binary = Path(metadata['target_directory']) / host / 'release/nouli'
    name = f'nouli-{version}-{host}'
    stage = output / name
    (stage / 'bin').mkdir(parents=True)
    shutil.copyfile(binary, stage / 'bin/nouli')
    (stage / 'bin/nouli').chmod(0o755)
    for filename in ('README.md', 'LICENSE', 'Cargo.lock', 'rust-toolchain.toml'):
        shutil.copyfile(ROOT / filename, stage / filename)
    shutil.copyfile(ROOT / 'docs/release.md', stage / 'INSTALL.md')
    shutil.copyfile(ROOT / 'fixtures/config/providers.toml', stage / 'config.example.toml')
    for filename in run('git', 'ls-files', '--', 'docs/*.md', 'docs/contracts/*.md', 'docs/testing/*.md').splitlines():
        # Release evidence refers to separately identified archives;
        # excluding these records avoids recursive checksum dependencies.
        if filename.startswith(('docs/release/',)):
            continue
        destination = stage / filename
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(ROOT / filename, destination)
    (stage / 'DISTRIBUTION_STATUS.md').write_text(
        '# Owner review required before distribution\n\n'
        'Nouli and its workspace packages use the MIT license in LICENSE. Public distribution requires owner approval, license/metadata decisions, and review of the available dependency notices. This archive is a local validation artifact. It is unsigned and unnotarized. No registry upload, Git tag, GitHub release, push, API credential, or model is included.\n'
    )
    third_party(stage, metadata, workspace)
    source = run('git', 'rev-parse', 'HEAD').strip()
    epoch = int(run('git', 'log', '-1', '--format=%ct').strip())
    manifest = {
        'version': 1, 'package_version': version, 'target': host,
        'source_commit': source,
        'tracked_changes': run('git', 'diff', 'HEAD', '--name-only').splitlines(),
        'toolchain': verbose.strip(), 'archive_epoch': epoch,
        'binary_sha256': digest(stage / 'bin/nouli'),
        'files': {str(path.relative_to(stage)): digest(path) for path in sorted(stage.rglob('*')) if path.is_file()},
    }
    (stage / 'MANIFEST.json').write_text(json.dumps(manifest, indent=2, sort_keys=True) + '\n')
    archive = output / f'{name}.tar.gz'
    # Normalize archive metadata; binary bytes still depend on target/linker/build inputs.
    with archive.open('wb') as raw:
        with gzip.GzipFile(filename='', fileobj=raw, mode='wb', mtime=epoch) as compressed:
            with tarfile.open(fileobj=compressed, mode='w', format=tarfile.USTAR_FORMAT) as tar:
                for path in sorted(stage.rglob('*')):
                    if not path.is_file():
                        continue
                    data = path.read_bytes()
                    info = tarfile.TarInfo(f'{name}/{path.relative_to(stage)}')
                    info.size, info.mtime = len(data), epoch
                    info.mode = 0o755 if path == stage / 'bin/nouli' else 0o644
                    tar.addfile(info, io.BytesIO(data))
    checksum = digest(archive)
    archive.with_suffix(archive.suffix + '.sha256').write_text(f'{checksum}  {archive.name}\n')
    print(json.dumps({'archive': str(archive), 'sha256': checksum, 'binary_sha256': manifest['binary_sha256'], 'target': host, 'source_commit': source, 'tracked_changes': manifest['tracked_changes']}, sort_keys=True))


if __name__ == '__main__':
    main()
