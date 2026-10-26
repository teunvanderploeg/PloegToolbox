#!/usr/bin/env python3
"""Package the Rust Toolbox and verified local Swift app builds for review."""
import argparse
import hashlib
import json
import os
import platform
import plistlib
import shutil
import subprocess
import tempfile
import time
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def run(*args):
    subprocess.run([str(a) for a in args], check=True)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--binary', type=Path, default=ROOT / 'target/debug/ploegtoolbox')
    parser.add_argument('--sound', type=Path, required=True)
    parser.add_argument('--voice', type=Path, required=True)
    parser.add_argument('--shot', type=Path, required=True)
    parser.add_argument('--icon', type=Path, default=ROOT / 'assets/PloegToolbox.icns')
    args = parser.parse_args()
    if platform.system() != 'Darwin':
        parser.error('This package command creates a macOS development bundle.')
    for path in [args.binary, args.sound, args.voice, args.shot, args.icon]:
        if not path.exists():
            parser.error('Missing build input: ' + str(path))
    dist = ROOT / 'dist'
    dist.mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='package-', dir=dist) as scratch:
        stage = Path(scratch)
        bundle = stage / 'PloegToolbox.app'
        macos = bundle / 'Contents/MacOS'
        resources = bundle / 'Contents/Resources'
        macos.mkdir(parents=True)
        (resources / 'packages').mkdir(parents=True)
        shutil.copy2(args.binary, macos / 'ploegtoolbox')
        shutil.copy2(args.icon, resources / 'PloegToolbox.icns')
        shutil.copy2(ROOT / 'assets/THIRD_PARTY_NOTICES.txt', resources / 'THIRD_PARTY_NOTICES.txt')
        info = {'CFBundleIdentifier': 'nl.ploeg.PloegToolbox', 'CFBundleName': 'PloegToolbox',
                'CFBundleDisplayName': 'PloegToolbox', 'CFBundleExecutable': 'ploegtoolbox',
                'CFBundlePackageType': 'APPL', 'CFBundleShortVersionString': '0.1.0',
                'CFBundleVersion': '1', 'CFBundleIconFile': 'PloegToolbox.icns',
                'LSMinimumSystemVersion': '13.0', 'NSHighResolutionCapable': True,
                'NSSupportsAutomaticGraphicsSwitching': True}
        (bundle / 'Contents/Info.plist').write_bytes(plistlib.dumps(info))
        architecture = {'arm64': 'aarch64', 'x86_64': 'x86_64'}[platform.machine()]
        releases = []
        for app_id, name, source in [('sound', 'SoundPloeg', args.sound), ('voice', 'VoicePloeg', args.voice), ('shot', 'PloegShot', args.shot)]:
            local_app = stage / (name + '.app')
            shutil.copytree(source, local_app, symlinks=True)
            run('/usr/bin/codesign', '--force', '--deep', '--sign', '-', '--preserve-metadata=identifier,entitlements,flags,runtime', local_app)
            run('/usr/bin/codesign', '--verify', '--deep', '--strict', local_app)
            metadata = plistlib.loads((local_app / 'Contents/Info.plist').read_bytes())
            if metadata.get('PloegDesignPreview'):
                raise RuntimeError('Refusing to package an inert design-preview build as a real app')
            if metadata.get('PloegToolboxOpenVersion', 0) < 1:
                raise RuntimeError('App build is missing the visible Open handler: ' + name)
            version = metadata['CFBundleShortVersionString']
            archive = resources / 'packages' / (app_id + '-' + version + '-macos-' + architecture + '.zip')
            with zipfile.ZipFile(archive, 'w', zipfile.ZIP_DEFLATED, compresslevel=6) as output:
                for file in sorted(local_app.rglob('*')):
                    if file.is_symlink():
                        resolved = file.resolve(strict=True)
                        if not resolved.is_relative_to(local_app.resolve()):
                            raise RuntimeError('Bundle link points outside the app: ' + str(file))
                        entry = zipfile.ZipInfo(file.relative_to(stage).as_posix())
                        entry.create_system = 3
                        entry.external_attr = 0o120777 << 16
                        entry.compress_type = zipfile.ZIP_DEFLATED
                        output.writestr(entry, os.readlink(file))
                    elif file.is_file():
                        output.write(file, file.relative_to(stage))
            releases.append({'app': app_id, 'version': version, 'os': 'macos', 'arch': architecture,
                             'source': 'packages/' + archive.name,
                             'sha256': hashlib.sha256(archive.read_bytes()).hexdigest(),
                             'toolbox_open_version': metadata.get('PloegToolboxOpenVersion', 0),
                             'notes': 'Local design-foundation build. Shared PloegKit styling and approved generated icon. Existing app workflows and data identities are preserved. This is not a public signed release.'})
        (resources / 'catalog.json').write_text(json.dumps({'schema': 1, 'releases': releases}, indent=2) + '\n')
        shutil.copy2(ROOT / 'README.txt', resources / 'README.txt')
        run('/usr/bin/codesign', '--force', '--deep', '--sign', '-', bundle)
        run('/usr/bin/codesign', '--verify', '--deep', '--strict', bundle)
        destination = dist / 'PloegToolbox.app'
        if destination.exists():
            destination.rename(dist / ('PloegToolbox.previous-' + str(time.time_ns()) + '.app'))
        shutil.move(bundle, destination)
        print(destination)
        print('Bundled local release catalog includes SoundPloeg, VoicePloeg and PloegShot.')
        print('Existing installations have not been replaced.')


if __name__ == '__main__':
    main()
