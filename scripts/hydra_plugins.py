#!/usr/bin/env python3
"""Lays out hdMoonray's USD plugins (the in-process MoonRay renderer and the
USD prim adapters) from a MoonRay build tree, one <Name>/resources/plugInfo.json
per plugin, as USD's own plugins are laid out.

  hydra_plugins.py <hdMoonray build dir> <plugin dir>
      plugInfo files pointing at the libraries where they are (for running from
      the build tree: PXR_PLUGINPATH_NAME=<plugin dir>)
  hydra_plugins.py <hdMoonray build dir> <plugin dir> --frameworks <dir>
      also copies the libraries into <dir> (an app bundle's Contents/Frameworks)
      and points the plugInfo files at them relative to the bundle
"""

import json
import os
import shutil
import sys
from pathlib import Path

PLUGINS = ["hd_moonray_debug", "adapters"]


def main():
    args = sys.argv[1:]
    frameworks = None
    if "--frameworks" in args:
        i = args.index("--frameworks")
        frameworks = Path(args[i + 1]).resolve()
        del args[i : i + 2]
    build, out = Path(args[0]).resolve(), Path(args[1]).resolve()
    # USD reads <plugin dir>/plugInfo.json, which includes each plugin's (USD's own folder has one).
    out.mkdir(parents=True, exist_ok=True)
    if not (out / "plugInfo.json").exists():
        (out / "plugInfo.json").write_text('{\n    "Includes": [ "*/resources/" ]\n}\n')
    for plugin in PLUGINS:
        source = build / "plugin" / plugin / "plugInfo.json"
        info = json.loads(source.read_text())
        for entry in info["Plugins"]:
            name = entry["Name"]
            library = Path(entry["LibraryPath"])
            if not library.is_absolute():
                # Relative paths are for MoonRay's install layout; the library is built next to plugInfo.json.
                library = source.parent / library.name
            if not library.is_file():
                sys.exit(f"hydra_plugins: {library} not found (needed by {source})")
            resources = out / name / "resources"
            resources.mkdir(parents=True, exist_ok=True)
            entry["Root"] = ".."
            entry["ResourcePath"] = "resources"
            if frameworks:
                frameworks.mkdir(parents=True, exist_ok=True)
                shutil.copy2(library, frameworks / library.name)
                os.chmod(frameworks / library.name, 0o755)
                entry["LibraryPath"] = os.path.relpath(frameworks / library.name, out / name)
            else:
                entry["LibraryPath"] = str(library)
            (resources / "plugInfo.json").write_text(json.dumps(info, indent=4) + "\n")
            print(f"  {name}: {entry['LibraryPath']}")


if __name__ == "__main__":
    main()
