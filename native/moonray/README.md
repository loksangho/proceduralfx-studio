Place MoonRay binaries for macOS here before packaging.

Supported layout:

  native/moonray/macos/moonray

Make the binary executable (`chmod +x native/moonray/macos/moonray`).

When packaging for macOS, run:

```bash
./proceduralfx_studio.sh package-macos
```

The script will build a release binary and create `dist/ProceduralFxStudio.app` with:

- `Contents/MacOS/ProceduralFxStudio` (the app binary)
- `Contents/Resources/moonray` (bundled MoonRay executable, if present)

At runtime the app looks for MoonRay in a list of candidate locations, preferring:

- `./Contents/Resources/moonray` (app bundle Resources)
- `./Contents/MacOS/moonray` (app bundle MacOS)
- `native/moonray/macos/moonray` (development folder)
- system paths like `/usr/local/bin/moonray` or `moonray` on PATH

If you intend to redistribute MoonRay with the app, place the appropriate macOS
binary at `native/moonray/macos/moonray` before running the packaging step.
