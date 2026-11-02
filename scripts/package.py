#!/usr/bin/env python3
"""Build a native .deb from a release executable; developer/CI use only."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import time
import tomllib

ROOT = Path(__file__).resolve().parents[1]
APP_ID = 'com.github.duskriver.snaptranslate'
PACKAGE = 'snaptranslate-linux'


def run(*args, **kwargs):
    return subprocess.run([str(arg) for arg in args], check=True, text=True, **kwargs)


def build(binary, output, version=None):
    binary = binary.resolve()
    if not binary.is_file():
        raise SystemExit(f'Missing binary: {binary}; build with cargo build --locked --release')
    manifest = tomllib.loads((ROOT / 'Cargo.toml').read_text())['package']
    version = version or manifest['version'].replace('-', '~', 1) + '-1'
    if not re.fullmatch(r'[0-9][0-9A-Za-z.+:~\-]*', version):
        raise SystemExit('Invalid Debian package version')
    # Prevent relabeling an executable as a different application release.
    binary_version = run(binary, '--version', capture_output=True).stdout.strip().split()[-1]
    if version.rsplit('-', 1)[0] != binary_version.replace('-', '~', 1):
        raise SystemExit(f'Package version {version} does not match executable {binary_version}')
    machine = run('readelf', '-h', binary, capture_output=True).stdout
    arch = 'amd64' if 'Advanced Micro Devices X86-64' in machine else 'arm64' if 'AArch64' in machine else None
    if arch is None or arch != run('dpkg', '--print-architecture', capture_output=True).stdout.strip():
        raise SystemExit('Build/package natively on the matching amd64 or arm64 host')
    output = output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    epoch = int(os.environ.get('SOURCE_DATE_EPOCH', str(int(time.time()))))
    with tempfile.TemporaryDirectory(prefix='snaptranslate-package-') as temporary:
        work = Path(temporary)
        stage = work / 'root'
        executable = stage / 'usr/bin' / PACKAGE
        executable.parent.mkdir(parents=True)
        shutil.copy2(binary, executable)
        run('strip', '--strip-unneeded', executable)
        executable.chmod(0o755)
        applications = stage / 'usr/share/applications'
        applications.mkdir(parents=True)
        shutil.copy2(ROOT / 'packaging' / (APP_ID + '.desktop'), applications)
        run('desktop-file-validate', applications / (APP_ID + '.desktop'))
        icons = stage / 'usr/share/icons/hicolor/scalable/apps'
        icons.mkdir(parents=True)
        shutil.copy2(ROOT / 'resources' / (APP_ID + '.svg'), icons)
        docs = stage / 'usr/share/doc' / PACKAGE
        docs.mkdir(parents=True)
        shutil.copy2(ROOT / 'packaging/USER_GUIDE.md', docs / 'README.md')
        license_path = ROOT / 'LICENSE'
        if license_path.exists():
            shutil.copy2(license_path, docs / 'copyright')
        # dpkg-shlibdeps derives actual library versions from ELF symbols.
        # Never advertise old-distro compatibility based on the file name alone.
        debian = work / 'debian'
        debian.mkdir()
        (debian / 'control').write_text(f'Source: {PACKAGE}\n\nPackage: {PACKAGE}\nArchitecture: any\nDescription: Screenshot translation application\n')
        deps = run('dpkg-shlibdeps', '-O', '-e' + str(executable), cwd=work, capture_output=True).stdout.strip()
        if not deps.startswith('shlibs:Depends='):
            raise SystemExit('Unable to calculate shared-library dependencies')
        dependencies = deps.removeprefix('shlibs:Depends=')
        installed_size = (sum(p.stat().st_size for p in stage.rglob('*') if p.is_file()) + 1023) // 1024
        control = stage / 'DEBIAN'
        control.mkdir()
        (control / 'control').write_text(f'''Package: {PACKAGE}
Version: {version}
Section: utils
Priority: optional
Architecture: {arch}
Maintainer: SnapTranslate contributors <noreply@users.noreply.github.com>
Installed-Size: {installed_size}
Depends: {dependencies}, ca-certificates, xdg-desktop-portal
Recommends: maim, fonts-noto-cjk, desktop-file-utils, hicolor-icon-theme, xdg-desktop-portal-gtk | xdg-desktop-portal-gnome | xdg-desktop-portal-kde
Suggests: grim, slurp
Description: Native screenshot translation application
 Select a region, translate its text using a configured vision service,
 and read or copy streamed translations in native desktop windows.
 An API key and a graphical desktop are required.
''')
        # No maintainer scripts: never start/kill the user's app, touch home
        # directories, or delete API credentials during installation/removal.
        hashes = []
        for path in sorted(stage.rglob('*')):
            if path.is_file() and control not in path.parents:
                hashes.append(hashlib.md5(path.read_bytes()).hexdigest() + '  ' + path.relative_to(stage).as_posix())
        (control / 'md5sums').write_text('\n'.join(hashes) + '\n')
        for path in stage.rglob('*'):
            path.chmod(0o755 if path.is_dir() or path == executable else 0o644)
            os.utime(path, (epoch, epoch))
        stage.chmod(0o755)
        os.utime(stage, (epoch, epoch))
        artifact = output / f'{PACKAGE}_{version}_{arch}.deb'
        run('dpkg-deb', '--root-owner-group', '-Zxz', '--build', stage, artifact,
            env=dict(os.environ, SOURCE_DATE_EPOCH=str(epoch)))
    digest = hashlib.sha256(artifact.read_bytes()).hexdigest()
    (output / (artifact.name + '.sha256')).write_text(f'{digest}  {artifact.name}\n')
    info = {'package': artifact.name, 'version': version, 'architecture': arch,
            'dependencies': dependencies, 'sha256': digest, 'build_os': Path('/etc/os-release').read_text()}
    (output / (artifact.name + '.build.json')).write_text(json.dumps(info, ensure_ascii=False, indent=2) + '\n')
    print(artifact)
    return artifact


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=ROOT / 'target/release' / PACKAGE)
    parser.add_argument('--output', type=Path, default=ROOT / 'dist')
    parser.add_argument('--version', help='Debian version, defaults to Cargo version plus -1')
    args = parser.parse_args()
    build(args.binary, args.output, args.version)
