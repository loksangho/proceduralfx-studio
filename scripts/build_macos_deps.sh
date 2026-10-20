#!/bin/bash
# Builds MoonRay, OpenUSD and their dependencies for macOS 14 (Sonoma) and later,
# into PREFIX, so the app bundle runs on macOS 14+. Homebrew's libraries are built
# for the macOS they were installed on, so every library the app bundles is built
# here from source instead (the versions Homebrew has, which the MoonRay checkout
# already builds against). Homebrew provides only build tools: cmake, ninja,
# pkg-config, ispc and python3.
#
# Each step is skipped once it has finished (stamps in PREFIX/.stamps), so the
# script can be re-run after fixing a failure. Usage:
#   scripts/build_macos_deps.sh            # everything
#   scripts/build_macos_deps.sh moonray    # from one step on (forgets its stamp)
set -euo pipefail

TARGET=14.0
PREFIX="${PREFIX:-$HOME/pfx-deps/macos$TARGET}"
SRC="${SRC:-$HOME/pfx-deps/src}"
BUILD="${BUILD:-$HOME/pfx-deps/build}"
MOONRAY_SRC="${MOONRAY_SRC:-$HOME/moonray}"
USD_SRC="${USD_SRC:-$HOME/OpenUSD}"
RANDOM123_INCLUDE="${RANDOM123_INCLUDE:-$HOME/random123-src/include}"
# WITH_HYDRA=1: also USD's imaging libraries (Hydra), Ptex, and MoonRay's Hydra delegate (hdMoonray).
WITH_HYDRA="${WITH_HYDRA:-0}"
JOBS="${JOBS:-$(sysctl -n hw.ncpu)}"
mkdir -p "$PREFIX/.stamps" "$SRC" "$BUILD" "$PREFIX/lib" "$PREFIX/include"

export MACOSX_DEPLOYMENT_TARGET=$TARGET
export CFLAGS="-mmacosx-version-min=$TARGET -O2"
export CXXFLAGS="-mmacosx-version-min=$TARGET -O2"
# Header room so install names can be rewritten later (here, and when the app is bundled).
export LDFLAGS="-mmacosx-version-min=$TARGET -L$PREFIX/lib -Wl,-rpath,$PREFIX/lib -Wl,-headerpad_max_install_names"
export CPPFLAGS="-I$PREFIX/include"
# Only our own .pc files.
export PKG_CONFIG_LIBDIR="$PREFIX/lib/pkgconfig:$PREFIX/share/pkgconfig"
export PKG_CONFIG_PATH=""

CMAKE_COMMON=(
  -G Ninja
  -DCMAKE_BUILD_TYPE=Release
  -DCMAKE_INSTALL_PREFIX="$PREFIX"
  -DCMAKE_PREFIX_PATH="$PREFIX"
  -DCMAKE_OSX_DEPLOYMENT_TARGET=$TARGET
  -DCMAKE_OSX_ARCHITECTURES=arm64
  -DCMAKE_INSTALL_NAME_DIR="$PREFIX/lib"
  -DCMAKE_INSTALL_RPATH="$PREFIX/lib"
  -DCMAKE_IGNORE_PREFIX_PATH=/opt/homebrew
  "-DCMAKE_IGNORE_PATH=/opt/homebrew/include;/opt/homebrew/lib"
  -DCMAKE_FIND_FRAMEWORK=LAST
  -DCMAKE_POLICY_VERSION_MINIMUM=3.5
  -DBUILD_SHARED_LIBS=ON
  -DCMAKE_SHARED_LINKER_FLAGS=-Wl,-headerpad_max_install_names
  -DCMAKE_MODULE_LINKER_FLAGS=-Wl,-headerpad_max_install_names
  -DCMAKE_EXE_LINKER_FLAGS=-Wl,-headerpad_max_install_names
)

FROM="${1:-}"
done_step() { [[ -f "$PREFIX/.stamps/$1" ]]; }
mark() { touch "$PREFIX/.stamps/$1"; }
if [[ -n "$FROM" ]]; then rm -f "$PREFIX/.stamps/$FROM"; fi

# fetch NAME URL SHA256 -> unpacks into $SRC/NAME
fetch() {
  local name=$1 url=$2 sha=$3
  local file="$SRC/$(basename "$url")"
  [[ -f "$file" ]] || curl -fL --retry 3 -o "$file" "$url"
  [[ "$sha" == SKIP ]] || echo "$sha  $file" | shasum -a 256 -c - >/dev/null
  rm -rf "$SRC/$name"
  mkdir -p "$SRC/$name"
  case "$file" in
    *.zip) (cd "$SRC/$name" && unzip -q "$file") ;;
    *) tar -xf "$file" -C "$SRC/$name" ;;
  esac
  # Use the single top-level folder as the source root.
  local top
  top=$(find "$SRC/$name" -mindepth 1 -maxdepth 1 -type d | head -1)
  if [[ -n "$top" && $(find "$SRC/$name" -mindepth 1 -maxdepth 1 | wc -l) -eq 1 ]]; then
    mv "$top"/* "$top"/.[!.]* "$SRC/$name/" 2>/dev/null || true
    rmdir "$top" 2>/dev/null || true
  fi
}

cmake_build() {
  local name=$1 src=$2
  shift 2
  rm -rf "$BUILD/$name"
  cmake -S "$src" -B "$BUILD/$name" "${CMAKE_COMMON[@]}" "$@"
  cmake --build "$BUILD/$name" -j "$JOBS"
  cmake --install "$BUILD/$name"
}

autotools_build() {
  local name=$1
  shift
  (cd "$SRC/$name" && ./configure --prefix="$PREFIX" "$@" && make -j "$JOBS" && make install)
}

step() {
  local name=$1
  shift
  if done_step "$name"; then
    echo "== $name: done"
    return
  fi
  echo "== $name"
  "$@"
  mark "$name"
}

# --- Libraries ------------------------------------------------------------------

do_boost() {
  fetch boost https://github.com/boostorg/boost/releases/download/boost-1.92.0/boost-1.92.0-b2-nodocs.tar.xz ea7b982002cc9dfbe59b0b217b206f470dc75f3de0bb2973d844118934d82411
  (cd "$SRC/boost" &&
    ./bootstrap.sh --prefix="$PREFIX" --with-toolset=clang \
      --with-libraries=atomic,chrono,container,date_time,filesystem,iostreams,program_options,random,regex,system,thread &&
    ./b2 -j "$JOBS" install variant=release link=shared threading=multi toolset=clang \
      cxxflags="-mmacosx-version-min=$TARGET" linkflags="-mmacosx-version-min=$TARGET -Wl,-headerpad_max_install_names" \
      -sNO_LZMA=1 -sNO_ZSTD=1)
  # b2 gives dylibs bare install names; give them full paths so dependants load them.
  for lib in "$PREFIX"/lib/libboost_*.dylib; do
    install_name_tool -id "$lib" "$lib"
    for dep in $(otool -L "$lib" | awk '/^\t(@rpath\/)?libboost_/ {print $1}'); do
      install_name_tool -change "$dep" "$PREFIX/lib/$(basename "$dep")" "$lib"
    done
  done
}

do_tbb() {
  fetch tbb https://github.com/uxlfoundation/oneTBB/archive/refs/tags/v2023.1.0.tar.gz 191288b52e1e6b17198000b64d77d194bb65e791be46ebc606e9b091781e2070
  cmake_build tbb "$SRC/tbb" -DTBB_TEST=OFF -DTBB_STRICT=OFF
}

do_blosc() {
  fetch blosc https://github.com/Blosc/c-blosc/archive/refs/tags/v1.21.6.tar.gz 9fcd60301aae28f97f1301b735f966cc19e7c49b6b4321b839b4579a0c156f38
  cmake_build blosc "$SRC/blosc" -DBUILD_TESTS=OFF -DBUILD_BENCHMARKS=OFF -DBUILD_FUZZERS=OFF -DBUILD_STATIC=OFF -DPREFER_EXTERNAL_ZLIB=ON -DDEACTIVATE_ZSTD=ON
}

do_libdeflate() {
  fetch libdeflate https://github.com/ebiggers/libdeflate/archive/refs/tags/v1.26.tar.gz bba03fffc5538576213675ce6968fcff6ce2e67d82e4d5febea2d05f9f13cf85
  cmake_build libdeflate "$SRC/libdeflate" -DLIBDEFLATE_BUILD_STATIC_LIB=OFF -DLIBDEFLATE_BUILD_GZIP=OFF
}

do_imath() {
  fetch imath https://github.com/AcademySoftwareFoundation/Imath/archive/refs/tags/v3.2.3.tar.gz e10c12b3f21f45bf08e09d4215d9c7691368d747beebd840de0b6fefed2df9f8
  cmake_build imath "$SRC/imath" -DBUILD_TESTING=OFF -DPYTHON=OFF
}

do_openexr() {
  fetch openexr https://github.com/AcademySoftwareFoundation/openexr/archive/refs/tags/v3.5.2.tar.gz 85a291b9b8563fabef1aa3f6e912d744ce1d4b9fd5a8f978eda194cd51b15ae9
  cmake_build openexr "$SRC/openexr" -DBUILD_TESTING=OFF -DOPENEXR_BUILD_TOOLS=OFF -DOPENEXR_BUILD_EXAMPLES=OFF -DOPENEXR_INSTALL_EXAMPLES=OFF -DOPENEXR_FORCE_INTERNAL_DEFLATE=OFF -DOPENEXR_FORCE_INTERNAL_OPENJPH=ON
}

do_openvdb() {
  fetch openvdb https://github.com/AcademySoftwareFoundation/openvdb/archive/refs/tags/v13.1.0.tar.gz 21659ef2330a06805519dd8d4369375f181a0dfee205b0180da69b4edd3329ae
  cmake_build openvdb "$SRC/openvdb" -DCONCURRENT_MALLOC=None -DOPENVDB_BUILD_BINARIES=OFF -DOPENVDB_BUILD_UNITTESTS=OFF \
    -DOPENVDB_BUILD_PYTHON_MODULE=OFF -DOPENVDB_USE_DELAYED_LOADING=OFF -DUSE_BLOSC=ON -DUSE_ZLIB=ON -DUSE_IMATH_HALF=ON -DUSE_EXPLICIT_INSTANTIATION=OFF
}

do_jsoncpp() {
  fetch jsoncpp https://github.com/open-source-parsers/jsoncpp/archive/refs/tags/1.9.8.tar.gz 51828cf3574281d2b79ec2a1c56a9e4c20cc1103711321ea96384cffb8d2d904
  cmake_build jsoncpp "$SRC/jsoncpp" -DBUILD_STATIC_LIBS=OFF -DBUILD_OBJECT_LIBS=OFF -DJSONCPP_WITH_TESTS=OFF -DJSONCPP_WITH_POST_BUILD_UNITTEST=OFF
}

do_log4cplus() {
  fetch log4cplus https://downloads.sourceforge.net/project/log4cplus/log4cplus-stable/2.2.0/log4cplus-2.2.0.1.tar.xz 6fc6b1b392921b048dcb0a71b5a1b46e9956bff710d288bdc9c4fc689f1a5f0b
  autotools_build log4cplus --disable-static --without-tests
}

do_cppunit() {
  fetch cppunit https://dev-www.libreoffice.org/src/cppunit-1.15.1.tar.gz 89c5c6665337f56fd2db36bc3805a5619709d51fb136e51937072f63fcc717a7
  autotools_build cppunit --disable-static --disable-doxygen
}

do_microhttpd() {
  fetch microhttpd https://ftpmirror.gnu.org/libmicrohttpd/libmicrohttpd-1.0.10.tar.gz 04bfe8ef75db7d629a33de767599765cecadc56274a39822d5d081030d577685
  autotools_build microhttpd --disable-static --disable-doc --disable-examples --disable-https
}

do_lua() {
  fetch lua https://www.lua.org/ftp/lua-5.5.1.tar.gz 1c4b4068d67061f2a2231ad2b5422e77acea1487ea9890f6320af614f4373dce
  # A shared liblua (MoonRay links it), built like Homebrew's.
  (cd "$SRC/lua/src" &&
    make -j "$JOBS" macosx MYCFLAGS="-fPIC $CFLAGS" MYLDFLAGS="$LDFLAGS" &&
    cc -dynamiclib -mmacosx-version-min=$TARGET -o liblua.5.5.dylib -install_name "$PREFIX/lib/liblua.5.5.dylib" \
      -compatibility_version 5.5 -current_version 5.5.1 $(ls *.o | grep -v -E '^(lua|luac)\.o$') &&
    cd .. && make install INSTALL_TOP="$PREFIX" &&
    cp src/liblua.5.5.dylib "$PREFIX/lib/" && ln -sf liblua.5.5.dylib "$PREFIX/lib/liblua.dylib")
  mkdir -p "$PREFIX/lib/pkgconfig"
  cat > "$PREFIX/lib/pkgconfig/lua.pc" <<EOF
prefix=$PREFIX
libdir=\${prefix}/lib
includedir=\${prefix}/include
Name: Lua
Description: Lua language engine
Version: 5.5.1
Libs: -L\${libdir} -llua
Cflags: -I\${includedir}
EOF
}

do_embree() {
  fetch embree https://github.com/RenderKit/embree/archive/refs/tags/v4.4.1.tar.gz dcf338cc61b636c871ccf370e673bfd380c5ecb71ce49ad50f28e1d4ec9995dc
  cmake_build embree "$SRC/embree" -DEMBREE_ISPC_SUPPORT=ON -DEMBREE_ISPC_EXECUTABLE="$(command -v ispc)" -DEMBREE_IGNORE_INVALID_RAYS=ON \
    -DEMBREE_RAY_MASK=ON -DEMBREE_TUTORIALS=OFF -DEMBREE_TESTING_INSTALL_TESTS=OFF -DEMBREE_TASKING_SYSTEM=TBB
}

do_yamlcpp() {
  fetch yamlcpp https://github.com/jbeder/yaml-cpp/archive/refs/tags/yaml-cpp-0.9.0.tar.gz 25cb043240f828a8c51beb830569634bc7ac603978e0f69d6b63558dadefd49a
  cmake_build yamlcpp "$SRC/yamlcpp" -DYAML_BUILD_SHARED_LIBS=ON -DYAML_CPP_BUILD_TESTS=OFF -DYAML_CPP_BUILD_TOOLS=OFF -DYAML_CPP_BUILD_CONTRIB=OFF
}

do_ocio() {
  fetch ocio https://github.com/AcademySoftwareFoundation/OpenColorIO/archive/refs/tags/v2.6.0.tar.gz 784ac37bde5b9c6e5dc15f23d1235bd1452d376cdc03627d6d6dd310f375a735
  cmake_build ocio "$SRC/ocio" -DOCIO_BUILD_APPS=OFF -DOCIO_BUILD_TESTS=OFF -DOCIO_BUILD_GPU_TESTS=OFF -DOCIO_BUILD_PYTHON=OFF -DOCIO_BUILD_DOCS=OFF \
    -DOCIO_INSTALL_EXT_PACKAGES=MISSING
}

do_jpeg() {
  fetch jpeg https://github.com/libjpeg-turbo/libjpeg-turbo/releases/download/3.2.0/libjpeg-turbo-3.2.0.tar.gz 6f30092cef9fb839779646608f4ee14ae3cbac989c47fa05e841b0841f09878e
  cmake_build jpeg "$SRC/jpeg" -DENABLE_STATIC=OFF -DWITH_TURBOJPEG=OFF
}

do_png() {
  fetch png https://downloads.sourceforge.net/project/libpng/libpng16/1.6.59/libpng-1.6.59.tar.xz d80dd2a38a37f803cb9b6ac7b14bd6e74ddc3b654780a8380bdf93523fdb4389
  cmake_build png "$SRC/png" -DPNG_STATIC=OFF -DPNG_TESTS=OFF -DPNG_TOOLS=OFF -DPNG_FRAMEWORK=OFF
}

do_tiff() {
  fetch tiff https://download.osgeo.org/libtiff/tiff-4.7.2.tar.gz 672bd7d10aee4606171afb864f3570b83340f6a33e2c186dc0512f7145ffdf6a
  cmake_build tiff "$SRC/tiff" -Dtiff-tools=OFF -Dtiff-tests=OFF -Dtiff-contrib=OFF -Dtiff-docs=OFF -Dwebp=OFF -Dlzma=OFF -Dzstd=OFF -Djbig=OFF -Dlerc=OFF -Dlibdeflate=OFF -Djpeg=ON -Dzlib=ON
}

do_oiio() {
  fetch oiio https://github.com/AcademySoftwareFoundation/OpenImageIO/archive/refs/tags/v3.2.1.2.tar.gz 3f4ccfbd9ef40098bcc81bbdcd7f9d4ed97724bb2f4de92daf2310885704404e
  cmake_build oiio "$SRC/oiio" "-DOpenImageIO_BUILD_MISSING_DEPS=fmt;Robinmap" -DOIIO_BUILD_TESTS=OFF -DOIIO_BUILD_TOOLS=OFF -DOIIO_BUILD_DOCS=OFF \
    -DUSE_PYTHON=OFF -DUSE_QT=OFF -DENABLE_FFmpeg=OFF -DENABLE_libuhdr=OFF -DENABLE_JXL=OFF -DENABLE_OpenCV=OFF -DENABLE_DCMTK=OFF \
    -DENABLE_GIF=OFF -DENABLE_Libheif=OFF -DENABLE_LibRaw=OFF -DENABLE_OpenJPEG=OFF -DENABLE_openjph=OFF -DENABLE_OpenVDB=OFF -DENABLE_Ptex=OFF \
    -DENABLE_WebP=OFF -DENABLE_R3DSDK=OFF -DENABLE_Nuke=OFF -DENABLE_Qt5=OFF -DENABLE_Qt6=OFF -DENABLE_Freetype=OFF -DENABLE_TBB=ON
}

do_oidn() {
  fetch oidn https://github.com/RenderKit/oidn/releases/download/v2.5.1/oidn-2.5.1.src.tar.gz e71fd043a70f1cc80e301d1b90df6c1f536098c4dd94baa612742f6db3369c36
  cmake_build oidn "$SRC/oidn" -DISPC_EXECUTABLE="$(command -v ispc)" -DOIDN_APPS=OFF -DOIDN_DEVICE_CPU=ON -DOIDN_DEVICE_SYCL=OFF \
    -DOIDN_DEVICE_CUDA=OFF -DOIDN_DEVICE_HIP=OFF -DOIDN_DEVICE_METAL=OFF
}

do_opensubdiv() {
  fetch opensubdiv https://github.com/PixarAnimationStudios/OpenSubdiv/archive/refs/tags/v3_6_1.tar.gz e9d99a480d80e999667643747c76eb0138d36c418fc154fd71b4bd65be103d52
  cmake_build opensubdiv "$SRC/opensubdiv" -DNO_PTEX=ON -DNO_OMP=ON -DNO_TBB=ON -DNO_CUDA=ON -DNO_OPENCL=ON -DNO_CLEW=ON -DNO_GLFW=ON \
    -DNO_DOC=ON -DNO_EXAMPLES=ON -DNO_TUTORIALS=ON -DNO_REGRESSION=ON -DNO_TESTS=ON -DNO_GLTESTS=ON
}

do_ptex() {
  fetch ptex https://github.com/wdas/ptex/archive/refs/tags/v2.4.3.tar.gz 435aa2ee1781ff24859bd282b7616bfaeb86ca10604b13d085ada8aa7602ad27
  cmake_build ptex "$SRC/ptex" -DPTEX_BUILD_STATIC_LIBS=OFF -DPTEX_BUILD_DOCS=OFF
}

do_usd() {
  # The local checkout, the same commit the app and MoonRay already build against. No Python.
  local imaging=OFF
  [[ "$WITH_HYDRA" == 1 ]] && imaging=ON
  cmake_build usd "$USD_SRC" -DPXR_ENABLE_PYTHON_SUPPORT=OFF -DPXR_BUILD_IMAGING=$imaging -DPXR_BUILD_USD_IMAGING=$imaging -DPXR_BUILD_USDVIEW=OFF \
    -DPXR_BUILD_TESTS=OFF -DPXR_BUILD_EXAMPLES=OFF -DPXR_BUILD_TUTORIALS=OFF -DPXR_BUILD_USD_TOOLS=OFF -DPXR_BUILD_DOCUMENTATION=OFF \
    -DPXR_ENABLE_MATERIALX_SUPPORT=OFF -DPXR_ENABLE_OPENVDB_SUPPORT=OFF -DPXR_ENABLE_PTEX_SUPPORT=$imaging -DPXR_BUILD_EXEC=OFF -DTBB_ROOT="$PREFIX" \
    -DPXR_BUILD_OPENIMAGEIO_PLUGIN=OFF -DPXR_BUILD_OPENCOLORIO_PLUGIN=OFF -DPXR_BUILD_EMBREE_PLUGIN=OFF -DPXR_ENABLE_GL_SUPPORT=$imaging
}

# OpenSSL and FreeType are static, so they end up inside MoonRay's own libraries.
do_openssl() {
  fetch openssl https://github.com/openssl/openssl/releases/download/openssl-4.0.3/openssl-4.0.3.tar.gz 325b5c806167c13b40b1ffeadfe0248197c00eccc4cf123ec1e28d2d2fd216d9
  (cd "$SRC/openssl" && ./Configure darwin64-arm64-cc no-shared no-tests no-docs no-apps -fPIC --prefix="$PREFIX" --libdir=lib "-mmacosx-version-min=$TARGET" \
    && make -j "$JOBS" && make install_sw)
}

do_freetype() {
  fetch freetype https://download.savannah.gnu.org/releases/freetype/freetype-2.14.3.tar.xz 36bc4f1cc413335368ee656c42afca65c5a3987e8768cc28cf11ba775e785a5f
  cmake_build freetype "$SRC/freetype" -DBUILD_SHARED_LIBS=OFF -DCMAKE_POSITION_INDEPENDENT_CODE=ON \
    -DFT_DISABLE_ZLIB=ON -DFT_DISABLE_BZIP2=ON -DFT_DISABLE_PNG=ON -DFT_DISABLE_HARFBUZZ=ON -DFT_DISABLE_BROTLI=ON
}

# Alembic (Ogawa, no HDF5), static: linked into the app for .abc import and export.
do_alembic() {
  fetch alembic https://github.com/alembic/alembic/archive/refs/tags/1.8.12.tar.gz b6d916c40446e8c502c84273092ab3d98f3f7f6094f8a2b8203d23e2f1d2a4a0
  cmake_build alembic "$SRC/alembic" -DALEMBIC_SHARED_LIBS=OFF -DCMAKE_POSITION_INDEPENDENT_CODE=ON \
    -DUSE_BINARIES=OFF -DUSE_TESTS=OFF -DUSE_HDF5=OFF -DUSE_PYALEMBIC=OFF -DUSE_ARNOLD=OFF -DUSE_PRMAN=OFF -DUSE_MAYA=OFF
}

do_moonray() {
  # Same source and options as the current build, against PREFIX. Makefiles, not Ninja (duplicate outputs).
  # Boost.Python is only needed with USD's Python bindings, which this USD doesn't have.
  for f in "$MOONRAY_SRC/moonray/moonshine_usd/CMakeLists.txt" "$MOONRAY_SRC/moonray/hydra/hdMoonray/CMakeLists.txt"; do
    sed -i '' 's/find_package(Boost REQUIRED COMPONENTS "${BOOST_PYTHON_COMPONENT_NAME}")/find_package(Boost REQUIRED)/' "$f"
  done
  # Nor the Python module of scene_rdl2, or Boost.Python in the Hydra command line tools.
  sed -i '' 's/^add_subdirectory(python)/# add_subdirectory(python)/' "$MOONRAY_SRC/moonray/scene_rdl2/mod/CMakeLists.txt"
  for f in "$MOONRAY_SRC"/moonray/hydra/hdMoonray/cmd/hd_cmd/{hd_render,hd_usd2rdl}/CMakeLists.txt; do
    sed -i '' '/Boost::\${BOOST_PYTHON_COMPONENT_NAME}/d' "$f"
  done
  # They start Python for USD's plugin loader, which only USD built with Python needs.
  sed -i '' '/Py_Initialize();/d' "$MOONRAY_SRC"/moonray/hydra/hdMoonray/cmd/hd_cmd/{hd_render/hd_render.cc,hd_usd2rdl/hd_usd2rdl.cc}
  # The Sdr shader-registry plugins (MoonRay shaders listed in other apps) use Ndr, which
  # USD 0.26 removed; hdMoonray maps material ids to MoonRay shaders without them.
  sed -i '' 's/^add_subdirectory(moonray_sdr_plugins)/# add_subdirectory(moonray_sdr_plugins)/' "$MOONRAY_SRC/moonray/hydra/CMakeLists.txt"
  # USD 0.27 passes the renderer's create arguments to HdRendererPlugin::IsSupported.
  sed -i '' 's/bool IsSupported(bool gpuEnabled = true) const override {/bool IsSupported(const pxr::HdRendererCreateArgsSchema \&, std::string * = nullptr) const override {/' \
    "$MOONRAY_SRC"/moonray/hydra/hdMoonray/plugin/{hd_moonray,hd_moonray_debug}/RendererPlugin.cc
  # hdMoonray asks for Vulkan with USD 25.05+, but only USD built with Vulkan needs it (this one isn't).
  sed -i '' 's/    find_package(Vulkan REQUIRED)/    find_package(Vulkan)/' "$MOONRAY_SRC/moonray/hydra/hdMoonray/CMakeLists.txt"
  # The Hydra delegate builds only with WITH_HYDRA (it needs USD imaging).
  if [[ "$WITH_HYDRA" == 1 ]]; then
    sed -i '' 's/^# add_subdirectory(hydra)/add_subdirectory(hydra)/' "$MOONRAY_SRC/moonray/CMakeLists.txt"
  else
    sed -i '' 's/^add_subdirectory(hydra)/# add_subdirectory(hydra)/' "$MOONRAY_SRC/moonray/CMakeLists.txt"
  fi
  # uuid is part of libSystem on macOS, and curl comes with the SDK.
  local SDK
  SDK=$(xcrun --show-sdk-path)
  cat > "$BUILD/moonray_usd_fix.cmake" <<EOF
include_directories(SYSTEM "$PREFIX/include")
EOF
  rm -rf "$BUILD/moonray"
  cmake -S "$MOONRAY_SRC" -B "$BUILD/moonray" -G "Unix Makefiles" "${CMAKE_COMMON[@]:2}" \
    -DCMAKE_PROJECT_INCLUDE_BEFORE="$BUILD/moonray_usd_fix.cmake" \
    -Dpxr_DIR="$PREFIX" -DPXR_USD_LOCATION="$PREFIX" -DPXR_INCLUDE_DIRS="$PREFIX/include" \
    -DLibuuid_INCLUDE_DIRS="$SDK/usr/include" -DLibuuid_LIBRARY="$SDK/usr/lib/libSystem.tbd" \
    -DLibcurl_INCLUDE_DIR="$SDK/usr/include/curl" -DLibcurl_LIBRARY="$SDK/usr/lib/libcurl.tbd" \
    -DOPENSSL_ROOT_DIR="$PREFIX" -DOPENSSL_USE_STATIC_LIBS=ON \
    -DRandom123_INCLUDE_DIR="$RANDOM123_INCLUDE/Random123" -DCMAKE_ISPC_COMPILER="$(command -v ispc)" \
    -DPython_EXECUTABLE=/usr/bin/python3 -DABI_VERSION=0 -DBUILD_QT_APPS=OFF -DMOONRAY_USE_OPTIX=OFF -DMOONRAY_USE_METAL=OFF \
    -DBUILD_TESTING=OFF -DARRASCORE_BUILD_TESTING=OFF -DMCRTCOMPUTATION_BUILD_TESTING=OFF -DMCRTDATAIO_BUILD_TESTING=OFF \
    -DMCRTDENOISE_BUILD_TESTING=OFF -DMCRTMESSAGES_BUILD_TESTING=OFF -DMOONRAY_BUILD_TESTING=OFF -DMOONSHINE_BUILD_TESTING=OFF \
    -DMOONSHINEUSD_BUILD_TESTING=OFF -DSCENERDL2_BUILD_TESTING=OFF \
    -DCMAKE_CXX_FLAGS="-mmacosx-version-min=$TARGET -D_LIBCPP_ENABLE_CXX17_REMOVED_UNARY_BINARY_FUNCTION"
  cmake --build "$BUILD/moonray" -j "$JOBS"
}

step boost do_boost
step tbb do_tbb
step blosc do_blosc
step libdeflate do_libdeflate
step imath do_imath
step openexr do_openexr
step openvdb do_openvdb
step jsoncpp do_jsoncpp
step log4cplus do_log4cplus
step cppunit do_cppunit
step microhttpd do_microhttpd
step lua do_lua
step embree do_embree
step yamlcpp do_yamlcpp
step ocio do_ocio
step jpeg do_jpeg
step png do_png
step tiff do_tiff
step oiio do_oiio
step oidn do_oidn
step opensubdiv do_opensubdiv
[[ "$WITH_HYDRA" == 1 ]] && step ptex do_ptex
step usd do_usd
step openssl do_openssl
step freetype do_freetype
step moonray do_moonray
step alembic do_alembic

# Everything in PREFIX should now need macOS 14 or older.
echo "== check"
newer=0
for f in "$PREFIX"/lib/*.dylib; do
  minos=$(otool -l "$f" | awk '/LC_BUILD_VERSION/{f=1} f&&/minos/{print $2; exit}')
  if [[ -n "$minos" && "${minos%%.*}" -gt 14 ]]; then
    echo "  $f needs macOS $minos"
    newer=1
  fi
done
[[ $newer == 0 ]] && echo "All libraries in $PREFIX run on macOS $TARGET."
