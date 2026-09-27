#!/usr/bin/env python3
"""Assemble an offline rdpilot payload using unchanged pinned upstream bytes.

Network access is build-time only. Sign the bridge BEFORE invoking this script:
--bridge is the final publisher-signed executable (or explicitly unsigned dev build).
"""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import urllib.request
import zipfile

VERSION = '0.28.2'
BUNDLE_ID = 'cua-driver-rs-v' + VERSION
ARCHIVE_SHA256 = '1f4bfceeab64cb7f56be7aad774c3dc2d2910d1427e4be1d79939c706e8029ba'
URL = f'https://github.com/trycua/cua/releases/download/{BUNDLE_ID}/cua-driver-rs-{VERSION}-windows-x86_64-binary.zip'
ROOT = Path(__file__).resolve().parent.parent

def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--bridge', type=Path, required=True)
    parser.add_argument('--archive', type=Path, help='Use an already downloaded upstream ZIP')
    parser.add_argument('--bundle-only', action='store_true', help='Development guest payload only')
    parser.add_argument('--bin-dir', type=Path, help='Built local rdpilot, rdpilot-daemon, rdpilot-mcp directory')
    parser.add_argument('--output', type=Path, required=True, help='New distribution directory')
    args = parser.parse_args()
    if not args.bundle_only and args.bin_dir is None:
        parser.error('--bin-dir is required for a complete distribution')
    if args.output.exists():
        parser.error('--output must be a new directory')
    args.output.mkdir(parents=True)
    try:
        bundle = args.output / 'bundle'
        bundle.mkdir()
        archive = bundle / 'cua.zip'
        if args.archive:
            shutil.copyfile(args.archive, archive)
        else:
            urllib.request.urlretrieve(URL, archive)
        if digest(archive) != ARCHIVE_SHA256:
            raise ValueError('Pinned upstream archive SHA256 mismatch')
        with zipfile.ZipFile(archive) as z:
            files = {}
            for item in z.infolist():
                if item.is_dir():
                    continue
                if '/' in item.filename or '\\' in item.filename or ':' in item.filename or item.filename in ('.', '..'):
                    raise ValueError('Unexpected archive layout')
                files[item.filename] = hashlib.sha256(z.read(item)).hexdigest()
            if 'cua-driver.exe' not in files:
                raise ValueError('Missing upstream executable')
        shutil.copyfile(args.bridge, bundle / 'rdpilot-bridge.exe')
        manifest = dict(bundle_id=BUNDLE_ID, archive_sha256=ARCHIVE_SHA256,
                        bridge_sha256=digest(bundle / 'rdpilot-bridge.exe'), files=files)
        (bundle / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
        shutil.copyfile(ROOT / 'packaging/bootstrap.ps1', bundle / 'bootstrap.ps1')
        shutil.copytree(ROOT / 'packaging/notices', args.output / 'notices')
        shutil.copyfile(ROOT / 'LICENSE.md', args.output / 'notices' / 'rdpilot-LICENSE')
        if args.bundle_only:
            print(args.output.resolve())
            return
        binaries = args.output / 'bin'
        binaries.mkdir()
        for name in ('rdpilot', 'rdpilot-daemon', 'rdpilot-mcp'):
            candidates = [args.bin_dir / name, args.bin_dir / (name + '.exe')]
            source = next((p for p in candidates if p.is_file()), None)
            if source is None:
                raise ValueError(f'Missing local binary: {name}')
            shutil.copy2(source, binaries / source.name)
        print(args.output.resolve())
    except BaseException:
        shutil.rmtree(args.output)
        raise

if __name__ == '__main__':
    main()
