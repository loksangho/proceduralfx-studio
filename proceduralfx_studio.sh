#!/usr/bin/env bash
set -euo pipefail

# Helper script for running or packaging the app on macOS.
# Usage:
#   ./proceduralfx_studio.sh            # run in debug via `cargo run`
#   ./proceduralfx_studio.sh package-macos  # build release and create .app with bundled MoonRay if present
#   ./proceduralfx_studio.sh preview-exr <file.exr> [out.png]
#   ./proceduralfx_studio.sh render-moonray <scene.rdla> <out.exr> [width] [height]

# Dependencies: the macOS 14 build from scripts/build_macos_deps.sh when it's there
# (PFX_DEPS, PFX_BUILD), else Homebrew and the local USD and MoonRay builds.
PFX_DEPS="${PFX_DEPS:-$HOME/pfx-deps/macos14.0}"
PFX_BUILD="${PFX_BUILD:-$HOME/pfx-deps/build}"
PFX_MOONRAY="$PFX_BUILD/moonray/moonray/moonray/cmd/raas_cmd/moonray/moonray"
if [[ -f "$PFX_DEPS/include/pxr/pxr.h" && -x "$PFX_MOONRAY" ]]; then
  OPENVDB_ROOT="$PFX_DEPS"
  PKG_CONFIG_PATH="$PFX_DEPS/lib/pkgconfig:$PFX_DEPS/share/pkgconfig:${PKG_CONFIG_PATH-}"
  export USD_ROOT="${USD_ROOT:-$PFX_DEPS}"
  export RDL2_DSO_PATH="${RDL2_DSO_PATH:-$PFX_BUILD/moonray/rdl2dso}"
  export MACOSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET:-14.0}"
  MOONRAY_SRC_BIN="$PFX_MOONRAY"
  # Alembic (.abc import and export), when the prefix has it (scripts/build_macos_deps.sh).
  if [[ -f "$PFX_DEPS/include/Alembic/AbcGeom/All.h" ]]; then
    export ALEMBIC_ROOT="$PFX_DEPS"
    APP_FEATURES="native-openvdb native-usd native-alembic"
  fi
  # Hydra: hd_render and hdMoonray's USD plugins, when MoonRay was built with them (WITH_HYDRA=1).
  PFX_HYDRA="$PFX_BUILD/moonray/moonray/hydra/hdMoonray"
  if [[ -x "$PFX_HYDRA/cmd/hd_cmd/hd_render/hd_render" ]]; then
    HD_RENDER_BIN="$PFX_HYDRA/cmd/hd_cmd/hd_render/hd_render"
  fi
else
  OPENVDB_ROOT="$(brew --prefix openvdb)"
  PKG_CONFIG_PATH="$(brew --prefix tbb)/lib/pkgconfig:${PKG_CONFIG_PATH-}"
  MOONRAY_SRC_BIN="native/moonray/macos/moonray"
fi

preview_exr() {
  local input_exr="$1"
  local output_png="${2-}"
  if [[ -z "$output_png" ]]; then
    output_png="${input_exr%.exr}_preview.png"
  fi
  python3 scripts/exr_preview.py "$input_exr" -o "$output_png"
}

find_moonray() {
  if [[ -x dist/ProceduralFxStudio.app/Contents/Resources/moonray ]]; then
    echo "dist/ProceduralFxStudio.app/Contents/Resources/moonray"
    return 0
  fi
  if [[ -x "$MOONRAY_SRC_BIN" ]]; then
    echo "$MOONRAY_SRC_BIN"
    return 0
  fi
  if command -v moonray >/dev/null 2>&1; then
    command -v moonray
    return 0
  fi
  return 1
}

if [[ ${1-} == "preview-exr" ]]; then
  if [[ $# -lt 2 ]]; then
    echo "Usage: ./proceduralfx_studio.sh preview-exr <file.exr> [out.png]"
    exit 2
  fi
  preview_exr "$2" "${3-}"
  exit 0
fi

if [[ ${1-} == "render-moonray" ]]; then
  if [[ $# -lt 3 ]]; then
    echo "Usage: ./proceduralfx_studio.sh render-moonray <scene.rdla> <out.exr> [width] [height]"
    exit 2
  fi
  SCENE="$2"
  OUT_EXR="$3"
  WIDTH="${4-256}"
  HEIGHT="${5-256}"

  MOONRAY_BIN="$(find_moonray || true)"
  if [[ -z "$MOONRAY_BIN" ]]; then
    echo "No MoonRay binary found (checked dist bundle, native/moonray/macos, PATH)"
    exit 2
  fi

  export RDL2_DSO_PATH="${RDL2_DSO_PATH:-/Users/loksangho/moonray/build/rdl2dso}"
  echo "MoonRay: $MOONRAY_BIN"
  echo "RDL2_DSO_PATH: $RDL2_DSO_PATH"
  "$MOONRAY_BIN" -in "$SCENE" -out "$OUT_EXR" -size "$WIDTH" "$HEIGHT"
  preview_exr "$OUT_EXR"
  exit 0
fi

if [[ ${1-} == "package-macos" ]]; then
  echo "Building release and packaging macOS .app..."
  # Lowest macOS the Rust/C++ code itself targets. The bundle's real minimum
  # is the highest of this and every bundled library; the bundler reports it.
  export MACOSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET:-12.0}"
  # Ensure build.rs can find OpenVDB and tbb when building release
  OPENVDB_ROOT="$OPENVDB_ROOT" PKG_CONFIG_PATH="$PKG_CONFIG_PATH" \
  cargo build --release --features "${APP_FEATURES:-native-openvdb native-usd}"
  APP_NAME="ProceduralFxStudio"
  APP_DIR="dist/${APP_NAME}.app"
  BIN_DIR="${APP_DIR}/Contents/MacOS"
  RES_DIR="${APP_DIR}/Contents/Resources"
  # Start from an empty bundle so nothing stale is left inside the signature.
  rm -rf "$APP_DIR"
  mkdir -p "$BIN_DIR" "$RES_DIR"

  # Copy the compiled binary
  cp target/release/proceduralfx_studio "$BIN_DIR/${APP_NAME}"
  chmod +x "$BIN_DIR/${APP_NAME}"

  # Bundle the MoonRay command line renderer into Resources when there is one.
  if [[ -x "$MOONRAY_SRC_BIN" ]]; then
    echo "Bundling MoonRay from $MOONRAY_SRC_BIN into app Resources"
    mkdir -p "$RES_DIR"
    cp "$MOONRAY_SRC_BIN" "$RES_DIR/moonray"
    chmod +x "$RES_DIR/moonray"
  else
    echo "No MoonRay binary at $MOONRAY_SRC_BIN; skipping MoonRay bundle"
  fi

  # Bundle RDL2 DSOs so constructors like RdlMeshGeometry are available at runtime.
  # Source priority:
  #  1) RDL2_DSO_PATH env var if it points to a directory
  #  2) default local MoonRay build path used in development
  DSO_SRC=""
  if [[ -n "${RDL2_DSO_PATH-}" && -d "${RDL2_DSO_PATH}" ]]; then
    DSO_SRC="${RDL2_DSO_PATH}"
  elif [[ -d /Users/loksangho/moonray/build/rdl2dso ]]; then
    DSO_SRC="/Users/loksangho/moonray/build/rdl2dso"
  fi

  if [[ -n "$DSO_SRC" ]]; then
    echo "Bundling RDL2 DSOs from $DSO_SRC into app Resources/rdl2dso"
    mkdir -p "$RES_DIR/rdl2dso"
    rm -rf "$RES_DIR/rdl2dso"/*
    cp -RL "$DSO_SRC"/. "$RES_DIR/rdl2dso/"
  else
    echo "No RDL2 DSO directory found (set RDL2_DSO_PATH to bundle DSOs)"
  fi

  # Copy every non-system dylib the app, MoonRay and the DSOs need into
  # Contents/Frameworks and point their load paths there.
  echo "Bundling shared libraries into the app"
  rm -rf "$APP_DIR/Contents/Frameworks"
  # OpenUSD needs its plugin registry (file formats, schemas). It searches
  # <libraries>/../plugin/usd, and signed bundles only allow code in Frameworks,
  # so it goes in Contents/plugin/usd with library paths pointing back into Frameworks.
  USD_ROOT="${USD_ROOT:-/Users/loksangho/openusd-build}"
  if [[ -d "$USD_ROOT/lib/usd" ]]; then
    echo "Bundling USD plugin registry from $USD_ROOT/lib/usd"
    rm -rf "$RES_DIR/usd" "$APP_DIR/Contents/plugin"
    mkdir -p "$APP_DIR/Contents/plugin"
    cp -R "$USD_ROOT/lib/usd" "$APP_DIR/Contents/plugin/usd"
    find "$APP_DIR/Contents/plugin/usd" -name plugInfo.json -exec sed -i '' 's|"LibraryPath": "\.\./\.\./|"LibraryPath": "../../../Frameworks/|' {} +
  else
    echo "No USD plugin registry found (set USD_ROOT); USD files will not open in the app"
  fi

  # Hydra (STAGE renders through hdMoonray): hd_render in Resources, and hdMoonray's
  # plugins next to USD's, with their libraries in Frameworks.
  if [[ -n "${HD_RENDER_BIN-}" && -d "$APP_DIR/Contents/plugin/usd" ]]; then
    echo "Bundling hd_render and hdMoonray's USD plugins"
    cp "$HD_RENDER_BIN" "$RES_DIR/hd_render"
    chmod +x "$RES_DIR/hd_render"
    python3 scripts/hydra_plugins.py "$PFX_HYDRA" "$APP_DIR/Contents/plugin/usd" --frameworks "$APP_DIR/Contents/Frameworks"
  else
    echo "No hd_render found; STAGE renders through Hydra won't be available in the app"
  fi

  # Also writes Info.plist and signs the bundle (SIGN_IDENTITY, default ad-hoc).
  # The app's version (Info.plist and the updater) comes from Cargo.toml.
  APP_VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)" \
    python3 scripts/bundle_app_libs.py "$APP_DIR"

  # Copy the app to other Macs via this zip: ditto keeps symlinks and code
  # signatures intact, which Finder copies or plain zip/cp can break.
  ZIP="dist/${APP_NAME}.zip"
  rm -f "$ZIP"
  ditto -c -k --keepParent "$APP_DIR" "$ZIP"

  # Notarization (needs SIGN_IDENTITY=Developer ID and a notarytool keychain profile,
  # e.g. NOTARY_PROFILE=pfx-notary): notarize and staple the app, then wrap it in a
  # signed, notarized DMG with an Applications shortcut.
  if [[ -n "${NOTARY_PROFILE-}" && -n "${SIGN_IDENTITY-}" && "${SIGN_IDENTITY}" != "-" ]]; then
    echo "Notarizing the app (this usually takes a few minutes)..."
    xcrun notarytool submit "$ZIP" --keychain-profile "$NOTARY_PROFILE" --wait
    xcrun stapler staple "$APP_DIR"
    rm -f "$ZIP"
    ditto -c -k --keepParent "$APP_DIR" "$ZIP"

    DMG="dist/${APP_NAME}.dmg"
    STAGE="$(mktemp -d)"
    ditto "$APP_DIR" "$STAGE/${APP_NAME}.app"
    ln -s /Applications "$STAGE/Applications"
    rm -f "$DMG"
    hdiutil create -volname "$APP_NAME" -srcfolder "$STAGE" -ov -format UDZO "$DMG" >/dev/null
    rm -rf "$STAGE"
    codesign --force --timestamp --sign "$SIGN_IDENTITY" "$DMG"
    echo "Notarizing the disk image..."
    xcrun notarytool submit "$DMG" --keychain-profile "$NOTARY_PROFILE" --wait
    xcrun stapler staple "$DMG"
    spctl --assess --type open --context context:primary-signature -v "$DMG"
    echo "Notarized disk image: $DMG"
  fi

  echo "Packaged app in $APP_DIR"
  echo "Distributable archive: $ZIP"
  exit 0
fi

echo "Running (debug) via cargo run..."
if [[ -n "${HD_RENDER_BIN-}" ]]; then
  python3 scripts/hydra_plugins.py "$PFX_HYDRA" "$PFX_BUILD/hydra-plugins" >/dev/null
  export PFX_HD_RENDER="$HD_RENDER_BIN" PFX_HYDRA_PLUGIN_PATH="$PFX_BUILD/hydra-plugins"
fi
OPENVDB_ROOT="$OPENVDB_ROOT" PKG_CONFIG_PATH="$PKG_CONFIG_PATH" cargo run --features "${APP_FEATURES:-native-openvdb native-usd}"
