#!/usr/bin/env python3
"""Make an .app self-contained: copy every non-system dylib that the app
executable, the bundled MoonRay binary and the RDL2 DSOs depend on into
Contents/Frameworks and rewrite their load paths to @rpath."""

import os
import plistlib
import shutil
import subprocess
from concurrent.futures import ThreadPoolExecutor
import sys
from pathlib import Path

SYSTEM_PREFIXES = ("/usr/lib/", "/System/", "/Library/Apple/")


def run(*cmd):
    return subprocess.run(cmd, capture_output=True, text=True)


def is_macho(path: Path) -> bool:
    with open(path, "rb") as f:
        return f.read(4) in (
            b"\xcf\xfa\xed\xfe",  # 64-bit little endian
            b"\xca\xfe\xba\xbe",  # universal
        )


def deps_of(path: Path):
    out = run("otool", "-L", str(path)).stdout.splitlines()[1:]
    return [line.strip().split(" (")[0] for line in out if line.strip()]


def install_id(path: Path):
    out = run("otool", "-D", str(path)).stdout.splitlines()
    return out[1].strip() if len(out) > 1 else None


def minos_of(path: Path):
    lines = run("otool", "-l", str(path)).stdout.splitlines()
    for i, line in enumerate(lines):
        if line.strip() == "cmd LC_BUILD_VERSION":
            for follow in lines[i + 1:i + 6]:
                parts = follow.split()
                if parts and parts[0] == "minos":
                    return tuple(int(x) for x in parts[1].split("."))
        if line.strip() == "cmd LC_VERSION_MIN_MACOSX":
            return tuple(int(x) for x in lines[i + 2].split()[1].split("."))
    return None


def rpaths_of(path: Path):
    lines = run("otool", "-l", str(path)).stdout.splitlines()
    found = []
    for i, line in enumerate(lines):
        if line.strip() == "cmd LC_RPATH":
            found.append(lines[i + 2].strip().split("path ", 1)[1].split(" (offset")[0])
    return found


class Bundler:
    def __init__(self, app: Path):
        self.app = app
        self.frameworks = app / "Contents" / "Frameworks"
        self.frameworks.mkdir(parents=True, exist_ok=True)
        # bundled name -> original real path
        self.libs = {}
        # (original path used for resolution, bundled copy, rpath to install)
        self.work = []
        self.signed = []
        # bundled file -> minimum macOS version it was built for
        self.minos = {}

    def expand(self, entry: str, origin: Path, exe_dir: Path) -> Path:
        entry = entry.replace("@loader_path", str(origin.parent))
        entry = entry.replace("@executable_path", str(exe_dir))
        return Path(entry)

    def resolve(self, dep: str, origin: Path, rpaths, exe_dir: Path):
        if dep.startswith(SYSTEM_PREFIXES):
            return None
        if dep.startswith("@rpath/"):
            name = dep[len("@rpath/"):]
            for rp in rpaths:
                candidate = self.expand(rp, origin, exe_dir) / name
                if candidate.exists():
                    return candidate.resolve()
            return None
        if dep.startswith("@"):
            candidate = self.expand(dep, origin, exe_dir)
            return candidate.resolve() if candidate.exists() else None
        candidate = Path(dep)
        return candidate.resolve() if candidate.exists() else None

    def add_root(self, original: Path, bundled: Path, rpath: str, exe_dir: Path):
        self.work.append((original, bundled, rpath, exe_dir, False))

    def process(self):
        index = 0
        while index < len(self.work):
            original, bundled, rpath, exe_dir, is_lib = self.work[index]
            index += 1
            own_id = install_id(original) if is_lib or original.suffix in (".so", ".proxy") else None
            rpaths = rpaths_of(original)
            for dep in deps_of(original):
                if dep == own_id or dep.startswith(SYSTEM_PREFIXES):
                    continue
                source = self.resolve(dep, original, rpaths, exe_dir)
                name = os.path.basename(dep)
                if source is None:
                    print(f"  warning: cannot resolve {dep} (needed by {original.name})")
                    continue
                if name not in self.libs:
                    self.libs[name] = source
                    target = self.frameworks / name
                    shutil.copy2(source, target)
                    os.chmod(target, 0o755)
                    self.work.append((source, target, "@loader_path", exe_dir, True))
                new = f"@rpath/{name}"
                if dep != new:
                    self.patch(bundled, "-change", dep, new)
            for old in rpaths:
                self.patch(bundled, "-delete_rpath", old)
            self.patch(bundled, "-add_rpath", rpath)
            if is_lib:
                self.patch(bundled, "-id", f"@rpath/{bundled.name}")
            self.signed.append(bundled)
            self.minos[bundled] = minos_of(bundled)

    def patch(self, target: Path, *args):
        result = run("install_name_tool", *args, str(target))
        if result.returncode != 0 and "no LC_RPATH" not in result.stderr:
            print(f"  warning: install_name_tool {' '.join(args)} {target.name}: {result.stderr.strip()}")

    def sign(self, identity: str):
        # Inside-out: every nested binary first, then the bundle itself, so the
        # bundle seal matches the final bytes of everything inside it.
        # Distribution identities (Developer ID) get the hardened runtime and a secure
        # timestamp, both required for notarization; ad-hoc signing skips them.
        if identity == "-":
            flags = ["--timestamp=none"]
        else:
            flags = ["--options", "runtime", "--timestamp"]
        entitlements = Path(__file__).with_name("entitlements.plist")
        executables = {self.app / "Contents" / "MacOS", self.app / "Contents" / "Resources"}

        def sign_one(path: Path):
            extra = []
            if identity != "-" and entitlements.exists() and path.parent in executables and is_macho(path):
                extra = ["--entitlements", str(entitlements)]
            result = run("codesign", "--force", *flags, *extra, "--sign", identity, str(path))
            return path, result

        # Nested binaries do not depend on each other's signatures, so sign them in parallel
        # (each Developer ID signature waits on Apple's timestamp server).
        retry = []
        with ThreadPoolExecutor(max_workers=8) as pool:
            for path, result in pool.map(sign_one, self.signed):
                if result.returncode != 0:
                    retry.append(path)
        # The keychain occasionally refuses concurrent access to the private key
        # (errSecInternalComponent); those files are signed again one at a time.
        failures = []
        for path in retry:
            for attempt in range(3):
                _, result = sign_one(path)
                if result.returncode == 0:
                    break
            else:
                failures.append(f"{path.name}: {result.stderr.strip()}")
        if failures:
            sys.exit("codesign failed:\n  " + "\n  ".join(failures[:20]))
        result = run("codesign", "--force", *flags, "--sign", identity, str(self.app))
        if result.returncode != 0:
            sys.exit(f"codesign of {self.app.name} failed: {result.stderr.strip()}")
        result = run("codesign", "--verify", "--strict", "--deep", str(self.app))
        if result.returncode != 0:
            sys.exit(f"signature verification failed: {result.stderr.strip()}")

    def required_macos(self):
        known = {path: v for path, v in self.minos.items() if v}
        if not known:
            return (11, 0), []
        highest = max(known.values())
        return highest, sorted(p.name for p, v in known.items() if v == highest)


def write_info_plist(app: Path, executable: str, min_macos: str):
    plist = {
        "CFBundleName": executable,
        "CFBundleDisplayName": executable,
        "CFBundleExecutable": executable,
        "CFBundleIdentifier": os.environ.get("BUNDLE_ID", "com.loksangho.proceduralfxstudio"),
        "CFBundlePackageType": "APPL",
        "CFBundleShortVersionString": os.environ.get("APP_VERSION", "0.1.0"),
        "CFBundleVersion": os.environ.get("APP_VERSION", "0.1.0"),
        "CFBundleInfoDictionaryVersion": "6.0",
        "LSMinimumSystemVersion": min_macos,
        "NSHighResolutionCapable": True,
    }
    if (app / "Contents" / "Resources" / "ProceduralFxStudio.icns").exists():
        plist["CFBundleIconFile"] = "ProceduralFxStudio"
    with open(app / "Contents" / "Info.plist", "wb") as f:
        plistlib.dump(plist, f)


def main():
    app = Path(sys.argv[1]).resolve()
    resources = app / "Contents" / "Resources"
    macos = app / "Contents" / "MacOS"
    # App icon (packaging/logo, generated from proceduralfx-studio.svg).
    icon = Path(__file__).resolve().parent.parent / "packaging" / "logo" / "ProceduralFxStudio.icns"
    if icon.exists():
        resources.mkdir(parents=True, exist_ok=True)
        shutil.copy2(icon, resources / icon.name)
    bundler = Bundler(app)

    # DSOs are symlinks into the MoonRay build tree; replace them with real copies.
    dso_dir = resources / "rdl2dso"
    if dso_dir.is_dir():
        for entry in sorted(dso_dir.iterdir()):
            if entry.is_symlink():
                real = entry.resolve()
                entry.unlink()
                shutil.copy2(real, entry)
            if entry.is_file() and is_macho(entry):
                # Resolve against the original location so @loader_path-style rpaths work.
                bundler.add_root(entry, entry, "@loader_path/../../Frameworks", macos)

    moonray = resources / "moonray"
    if moonray.is_file():
        bundler.add_root(moonray, moonray, "@executable_path/../Frameworks", macos)

    # hd_render, for STAGE renders through Hydra.
    hd_render = resources / "hd_render"
    if hd_render.is_file():
        bundler.add_root(hd_render, hd_render, "@executable_path/../Frameworks", macos)

    # Libraries already in Frameworks: plugins USD loads at run time (hdMoonray).
    for lib in sorted(bundler.frameworks.glob("*.dylib")):
        bundler.libs[lib.name] = lib
        bundler.work.append((lib, lib, "@loader_path", macos, True))

    for exe in sorted(macos.iterdir()):
        if exe.is_file() and is_macho(exe):
            bundler.add_root(exe, exe, "@executable_path/../Frameworks", macos)

    # Each file's dependencies are read just before that file is patched, so
    # no pristine copies are needed.
    bundler.process()

    highest, culprits = bundler.required_macos()
    min_macos = ".".join(str(x) for x in highest)
    executable = next(e.name for e in sorted(macos.iterdir()) if e.is_file() and is_macho(e))
    write_info_plist(app, executable, min_macos)

    identity = os.environ.get("SIGN_IDENTITY", "-")
    bundler.sign(identity)
    print(f"Bundled {len(bundler.libs)} libraries into {bundler.frameworks}")
    print(f"Signed with {'ad-hoc signature' if identity == '-' else identity}")
    print(f"Minimum macOS for this bundle: {min_macos}")
    if culprits:
        shown = ", ".join(culprits[:8]) + (" ..." if len(culprits) > 8 else "")
        print(f"  {len(culprits)} binaries require {min_macos}, e.g. {shown}")


if __name__ == "__main__":
    main()
