# pfx-render.exe: MoonRay for ProceduralFX Studio on Windows, through WSL containers.
#
#   cmake -S packaging/windows/render -B build-render \
#     -DMicrosoft.WSL.Containers_DIR=<the SDK's cmake folder> \
#     -DPFX_MOONRAY_IMAGE=ghcr.io/loksangho/pfx-moonray:<tag>
#   cmake --build build-render --config Release
#
# The SDK is the Microsoft.WSL.Containers NuGet package (MIT); its wslcsdk.dll ships
# next to pfx-render.exe.
cmake_minimum_required(VERSION 3.21)
project(pfx_render CXX)

set(PFX_MOONRAY_IMAGE "ghcr.io/loksangho/pfx-moonray:latest" CACHE STRING "The MoonRay image to download and run")

find_package(Microsoft.WSL.Containers REQUIRED)

add_executable(pfx-render pfx_render.cpp)
target_compile_features(pfx-render PRIVATE cxx_std_17)
target_compile_definitions(pfx-render PRIVATE UNICODE _UNICODE WIN32_LEAN_AND_MEAN NOMINMAX PFX_MOONRAY_IMAGE="${PFX_MOONRAY_IMAGE}")
target_link_libraries(pfx-render PRIVATE Microsoft.WSL.Containers::SDK ole32 shell32)
# The C runtime built in, so the program needs no Visual C++ redistributable.
set_property(TARGET pfx-render PROPERTY MSVC_RUNTIME_LIBRARY "MultiThreaded")
