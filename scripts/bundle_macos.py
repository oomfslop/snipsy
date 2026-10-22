import pathlib
import plistlib
import shutil
import subprocess
import sys
import tempfile

IDENTITY = "snipsy-dev"

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
        "CFBundleIdentifier": "desu.tei.snipsy",
        "CFBundleName": "Snipsy",
        "CFBundleDisplayName": "Snipsy",
        "CFBundlePackageType": "APPL",
        "CFBundleShortVersionString": "0.1.0",
        "CFBundleVersion": "1",
        "LSMinimumSystemVersion": "14.0",
        "NSHighResolutionCapable": True,
        "LSUIElement": True,
        "NSMicrophoneUsageDescription": "Snipsy records your microphone when you enable it for a screen recording.",
    }, file)

identities = subprocess.run(["security", "find-identity", "-p", "codesigning"], capture_output=True, text=True, check=True).stdout
if f'"{IDENTITY}"' not in identities:
    with tempfile.TemporaryDirectory() as tmp:
        tmp = pathlib.Path(tmp)
        (tmp / "req.cnf").write_text(
            "[req]\ndistinguished_name = dn\nx509_extensions = ext\nprompt = no\n"
            f"[dn]\nCN = {IDENTITY}\n"
            "[ext]\nbasicConstraints = critical,CA:false\nkeyUsage = critical,digitalSignature\nextendedKeyUsage = critical,codeSigning\n"
        )
        # LibreSSL: OpenSSL 3 defaults to AES-encrypted PKCS#12, which `security import` rejects
        openssl = "/usr/bin/openssl"
        subprocess.run([openssl, "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "3650", "-keyout", tmp / "key.pem", "-out", tmp / "cert.pem", "-config", tmp / "req.cnf"], check=True, capture_output=True)
        # empty passphrase trips a MAC mismatch on import
        subprocess.run([openssl, "pkcs12", "-export", "-passout", "pass:snipsy", "-name", IDENTITY, "-inkey", tmp / "key.pem", "-in", tmp / "cert.pem", "-out", tmp / "id.p12"], check=True, capture_output=True)
        subprocess.run(["security", "import", tmp / "id.p12", "-k", pathlib.Path.home() / "Library/Keychains/login.keychain-db", "-P", "snipsy", "-T", "/usr/bin/codesign"], check=True)
subprocess.run(["codesign", "--force", "--timestamp=none", "--sign", IDENTITY, bundle], check=True)
print(bundle)
