#!/usr/bin/env python3
"""Exercise .deb installation/upgrade/removal in a private dpkg root.

Uses a user namespace when unprivileged. Never installs into the host root.
GUI regression is run separately against the executable extracted from the deb.
"""
import argparse
import io
import tarfile
import os
from pathlib import Path, PurePosixPath
import shutil
import subprocess
import tempfile

from importlib.util import spec_from_file_location, module_from_spec

ROOT = Path(__file__).resolve().parents[1]
spec = spec_from_file_location('snap_package', ROOT / 'scripts/package.py')
packager = module_from_spec(spec)
spec.loader.exec_module(packager)


def command(*args, **kwargs):
    return subprocess.run([str(a) for a in args], text=True, check=True, **kwargs)


def verify(package, gui=False):
    package = package.resolve()
    name = command('dpkg-deb', '--field', package, 'Package', capture_output=True).stdout.strip()
    version = command('dpkg-deb', '--field', package, 'Version', capture_output=True).stdout.strip()
    if name != packager.PACKAGE:
        raise SystemExit('Unexpected package name')
    with tarfile.open(fileobj=io.BytesIO(subprocess.check_output(['dpkg-deb', '--fsys-tarfile', str(package)]))) as archive:
        for member in archive:
            assert member.uid == 0 and member.gid == 0, 'Package ownership must be root:root'
            assert not member.mode & 0o022, f'Group/world-writable package entry: {member.name}'
            path = PurePosixPath(member.name)
            assert not path.is_absolute() and '..' not in path.parts and (not path.parts or path.parts[0] == 'usr'), 'Unexpected installation path'

    with tempfile.TemporaryDirectory(prefix='snaptranslate-install-test-') as temporary:
        work = Path(temporary)
        root = work / 'system'
        root.mkdir()
        (root / 'var/lib/dpkg').mkdir(parents=True)
        (root / 'var/lib/dpkg/status').touch()
        home = root / 'home/tester/.config/snaptranslate'
        home.mkdir(parents=True)
        config = home / 'config.toml'
        credential = home / 'credentials.toml'
        config.write_text('target_language = "ja"\n')
        credential.write_text('deepseek_api_key = "synthetic-package-fixture"\n')
        expected = {p: p.read_bytes() for p in [config, credential]}
        controls = work / 'control'
        command('dpkg-deb', '--control', package, controls)
        assert {p.name for p in controls.iterdir()} <= {'control', 'md5sums'}, 'Unexpected installer scripts'
        prefix = []
        if os.geteuid() != 0:
            if not shutil.which('bwrap'):
                raise SystemExit('Install bubblewrap for an isolated root, or run this test as root')
            prefix = ['bwrap', '--unshare-user', '--uid', '0', '--gid', '0', '--ro-bind', '/', '/',
                      '--bind', str(work), str(work), '--proc', '/proc', '--dev', '/dev']

        def dpkg(*args):
            # Dependencies are checked by APT/ELF metadata separately. This empty
            # database deliberately contains no operating-system packages.
            return command(*prefix, '/usr/bin/dpkg', '--root=' + str(root), '--force-depends', *args,
                           stdout=subprocess.PIPE, stderr=subprocess.PIPE)

        dpkg('--install', package)
        executable = root / 'usr/bin' / name
        desktop = root / 'usr/share/applications' / (packager.APP_ID + '.desktop')
        icon = root / 'usr/share/icons/hicolor/scalable/apps' / (packager.APP_ID + '.svg')
        assert executable.is_file() and desktop.is_file() and icon.is_file()
        command('desktop-file-validate', desktop)
        command(executable, '--version')
        assert dpkg('--verify', name).stdout == '', 'Installed payload checksum mismatch'
        first_status = dpkg('--status', name).stdout
        assert 'Status: install ok installed' in first_status and f'Version: {version}' in first_status
        for path in root.rglob('*'):
            if path.is_file() and 'DEBIAN' not in path.parts:
                assert not path.stat().st_mode & 0o002, f'World-writable: {path}'
        if gui:
            command('/usr/bin/python3', ROOT / 'scripts/verify-gui-flow.py', '--binary', executable, '--keep-artifacts')
        # Upgrade the exact tested executable under a higher Debian revision.
        upgrade_version = version + '.1'
        upgrade = packager.build(executable, work / 'upgrade', upgrade_version)
        dpkg('--install', upgrade)
        assert f'Version: {upgrade_version}' in dpkg('--status', name).stdout
        assert all(p.read_bytes() == data for p, data in expected.items())
        dpkg('--remove', name)
        assert not executable.exists() and not desktop.exists() and not icon.exists()
        assert all(p.read_bytes() == data for p, data in expected.items())
        dpkg('--purge', name)
        assert all(p.read_bytes() == data for p, data in expected.items())
        print('PASS actual dpkg install, upgrade, remove and purge in an isolated root; user settings preserved')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('package', type=Path)
    parser.add_argument('--gui', action='store_true', help='Run the complete isolated GUI suite against the installed executable')
    args = parser.parse_args()
    verify(args.package, args.gui)
