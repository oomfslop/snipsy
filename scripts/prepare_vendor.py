import fcntl
import hashlib
import io
import os
import pathlib
import shutil
import subprocess
import sys
import tarfile
import tempfile
import urllib.request

root = pathlib.Path(__file__).resolve().parents[1]
version = "0.2.2"
checksum = "979b45cfa6ec723b6f42330915a1b3769b930d02b2d505f9697f8ca602bee707"
patch = root / "patches" / "gpui-frame-resize.patch"
destination = root / "vendor" / "gpui"
destination.parent.mkdir(parents=True, exist_ok=True)
stamp = destination / ".snipsy-patch"
fingerprint = hashlib.sha256(checksum.encode() + patch.read_bytes()).hexdigest()

with (destination.parent / ".lock").open("a") as lock:
    fcntl.flock(lock, fcntl.LOCK_EX)
    if not stamp.exists() or stamp.read_text() != fingerprint:
        cargo_home = pathlib.Path(os.environ.get("CARGO_HOME", pathlib.Path.home() / ".cargo"))
        archive = next((cargo_home / "registry" / "cache").glob(f"*/gpui-{version}.crate"), None)
        if archive:
            data = archive.read_bytes()
        else:
            with urllib.request.urlopen(f"https://static.crates.io/crates/gpui/gpui-{version}.crate", timeout=60) as response:
                data = response.read()
        if hashlib.sha256(data).hexdigest() != checksum:
            sys.exit("GPUI archive checksum mismatch")
        with tempfile.TemporaryDirectory(dir=destination.parent) as temporary:
            prepared = pathlib.Path(temporary) / "gpui"
            prepared.mkdir()
            with tarfile.open(fileobj=io.BytesIO(data), mode="r:gz") as archive:
                for member in archive.getmembers():
                    path = pathlib.PurePosixPath(member.name)
                    if not path.parts or path.parts[0] != f"gpui-{version}" or ".." in path.parts:
                        sys.exit(f"Invalid GPUI archive path: {member.name}")
                    target = prepared.joinpath(*path.parts[1:])
                    if member.isdir():
                        target.mkdir(parents=True, exist_ok=True)
                    elif member.isfile():
                        target.parent.mkdir(parents=True, exist_ok=True)
                        with archive.extractfile(member) as source, target.open("wb") as output:
                            shutil.copyfileobj(source, output)
                    else:
                        sys.exit(f"Unsupported GPUI archive entry: {member.name}")
            subprocess.run(["patch", "-p1", "--batch", "-i", str(patch)], cwd=prepared, check=True)
            (prepared / stamp.name).write_text(fingerprint)
            if destination.exists():
                shutil.rmtree(destination)
            prepared.rename(destination)
    print(f"Patched GPUI ready: {destination}")
