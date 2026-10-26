#!/usr/bin/env python3
"""Create verified drag-to-Applications DMGs from the built release ZIPs."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import plistlib
import subprocess
import tempfile


def run(*args):
    result = subprocess.run([str(arg) for arg in args], capture_output=True, text=True)
    if result.returncode:
        raise RuntimeError(result.stderr.strip() or result.stdout.strip())
    return result.stdout


def sha256(path):
    digest = hashlib.sha256()
    with path.open('rb') as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b''):
            digest.update(chunk)
    return digest.hexdigest()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--manifest', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    entries = json.loads(args.manifest.read_text())
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    images = {}
    for name, entry in entries.items():
        archive = Path(entry['archive'])
        if sha256(archive) != entry['sha256']:
            raise RuntimeError(f'Release ZIP checksum mismatch: {name}')
        architectures = set(entry['architecture'].split())
        variants = {frozenset({'arm64', 'x86_64'}): 'Universal',
                    frozenset({'arm64'}): 'AppleSilicon',
                    frozenset({'x86_64'}): 'Intel'}
        if frozenset(architectures) not in variants:
            raise RuntimeError(f'Unsupported architecture: {name}')
        variant = variants[frozenset(architectures)]
        image = output / f'{name}-macOS-{variant}.dmg'
        with tempfile.TemporaryDirectory(prefix=f'{name}-image-', dir=output) as temporary:
            work = Path(temporary)
            source = work / 'source'
            source.mkdir()
            run('/usr/bin/ditto', '-x', '-k', archive, source)
            app = source / (name + '.app')
            run('/usr/bin/codesign', '--verify', '--deep', '--strict', app)
            info = plistlib.loads((app / 'Contents/Info.plist').read_bytes())
            executable = app / 'Contents/MacOS' / info['CFBundleExecutable']
            if set(run('/usr/bin/lipo', '-archs', executable).split()) != architectures:
                raise RuntimeError(f'Unexpected app architecture: {name}')
            if info['CFBundleShortVersionString'] != entry['app_version']:
                raise RuntimeError(f'Unexpected app version: {name}')
            if any(key.startswith('PloegDesignPreview') for key in info):
                raise RuntimeError(f'Preview app cannot be distributed: {name}')
            volume = work / 'volume'
            volume.mkdir()
            run('/usr/bin/ditto', app, volume / app.name)
            os.symlink('/Applications', volume / 'Applications')
            run('/usr/bin/hdiutil', 'create', '-volname', name, '-srcfolder', volume,
                '-fs', 'HFS+', '-format', 'UDZO', '-ov', image)
            run('/usr/bin/hdiutil', 'verify', image)
            mount = work / 'mounted'
            mount.mkdir()
            attached = False
            try:
                run('/usr/bin/hdiutil', 'attach', '-readonly', '-nobrowse',
                    '-mountpoint', mount, image)
                attached = True
                installed = mount / app.name
                run('/usr/bin/codesign', '--verify', '--deep', '--strict', installed)
                mounted_info = plistlib.loads((installed / 'Contents/Info.plist').read_bytes())
                if mounted_info != info:
                    raise RuntimeError(f'DMG app metadata differs from ZIP: {name}')
                if not (mount / 'Applications').is_symlink() or os.readlink(mount / 'Applications') != '/Applications':
                    raise RuntimeError(f'Applications shortcut missing: {name}')
            finally:
                if attached:
                    run('/usr/bin/hdiutil', 'detach', mount)
        checksum = output / (name + '-DMG-SHA256SUMS.txt')
        checksum.write_text(sha256(image) + '  ' + image.name + '\n')
        images[name] = {'image': str(image), 'sha256': sha256(image),
                        'checksum': str(checksum), 'size': image.stat().st_size,
                        'variant': variant, 'tag': entry['tag']}
        print(f'{name}: verified {variant} DMG, {image.stat().st_size:,} bytes', flush=True)
    (output / 'disk-images.json').write_text(json.dumps(images, indent=2) + '\n')


if __name__ == '__main__':
    main()
