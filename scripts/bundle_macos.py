import pathlib
import plistlib
import shutil
import sys

root = pathlib.Path(__file__).resolve().parents[1]
profile = sys.argv[1] if len(sys.argv) > 1 else "debug"
binary = root / "target" / profile / "snipsy"
bundle = root / "target" / profile / "Snipsy.app"
destination = bundle / "Contents" / "MacOS"
destination.mkdir(parents=True, exist_ok=True)
shutil.copy2(binary, destination / "snipsy")
with (bundle / "Contents" / "Info.plist").open("wb") as file:
    plistlib.dump({
        "CFBundleExecutable": "snipsy",
        "CFBundleIdentifier": "app.snipsy.Snipsy",
        "CFBundleName": "Snipsy",
        "CFBundleDisplayName": "Snipsy",
        "CFBundlePackageType": "APPL",
        "CFBundleShortVersionString": "0.1.0",
        "CFBundleVersion": "1",
        "LSMinimumSystemVersion": "14.0",
        "NSHighResolutionCapable": True,
        "LSUIElement": True,
    }, file)
print(bundle)
