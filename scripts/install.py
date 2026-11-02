#!/usr/bin/env python3
"""Developer convenience: install a source build for the current user. End users use the release .deb."""
import argparse
import os
from pathlib import Path
import shutil

APP_ID = "com.github.duskriver.snaptranslate"

def desktop_quote(value):
    # Desktop Exec has its own quoting rules, independent of shell quoting.
    return '"' + str(value).replace('\\', '\\\\').replace('"', '\\"').replace('`', '\\`').replace('$', '\\$').replace('%', '%%') + '"'

def install(binary, data, executable):
    if not binary.is_file():
        raise SystemExit(f"Binary missing: {binary}; run cargo build --release first")
    executable.parent.mkdir(parents=True, exist_ok=True)
    # Atomic replace also works when an older executable is running.
    temporary = executable.with_name(executable.name + '.new')
    shutil.copy2(binary, temporary)
    temporary.chmod(0o755)
    temporary.replace(executable)
    icons = data / 'icons/hicolor/scalable/apps'
    icons.mkdir(parents=True, exist_ok=True)
    shutil.copy2(Path(__file__).resolve().parents[1] / 'resources' / (APP_ID + '.svg'), icons / (APP_ID + '.svg'))
    applications = data / 'applications'
    applications.mkdir(parents=True, exist_ok=True)
    command = desktop_quote(executable.resolve())
    desktop = applications / (APP_ID + '.desktop')
    desktop.write_text(f'''[Desktop Entry]
Type=Application
Name=SnapTranslate · 轻译
Comment=Select a screen region and translate its text
Exec={command}
Icon={APP_ID}
Terminal=false
Categories=Utility;
StartupWMClass={APP_ID}
Actions=Capture;

[Desktop Action Capture]
Name=截屏翻译
Exec={command} capture
''')
    print(f'Installed: {executable}\nLauncher: {desktop}')

if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=Path(__file__).resolve().parents[1] / 'target/release/snaptranslate-linux')
    parser.add_argument('--prefix', type=Path, default=Path.home() / '.local')
    args = parser.parse_args()
    data = Path(os.environ.get('XDG_DATA_HOME', str(args.prefix / 'share')))
    install(args.binary, data, args.prefix / 'libexec/snaptranslate-linux')
