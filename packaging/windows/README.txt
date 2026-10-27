ProceduralFX Studio for Windows
===============================

Run "ProceduralFX Studio.exe". Keep everything in this folder together:

  ProceduralFX Studio.exe   the app
  pfx-render.exe            runs MoonRay for the app (see Rendering)
  wslcsdk.dll               Microsoft's WSL Containers SDK, which pfx-render.exe uses
  the other .dll files, usd OpenUSD, OpenVDB and Alembic, for USD, .vdb and .abc files

Rendering
---------
MoonRay is a Linux program, so on Windows it runs in a small Linux container through
WSL (the Windows Subsystem for Linux). The first time, choose
Render > Set Up Rendering on Windows... in the app. It checks for WSL, then downloads
MoonRay (about 1.3 GB, once). Downloading and rendering never need administrator
rights.

If WSL isn't installed yet, installing it needs an administrator once (Windows asks),
and turning on Virtual Machine Platform needs a restart. On a PC where you can't
approve that, an administrator can run this once in a Terminal opened as
administrator, then restart:

  wsl --install --no-distribution

After that, choose Check again in the set-up window and it carries on without them.

Needs Windows 11 with a recent WSL, and virtualization turned on in the BIOS or UEFI
settings. Rendering uses up to three quarters of your memory and all your CPU cores.

The WSL Containers SDK is a preview from Microsoft, so a WSL update can occasionally
break rendering until the app catches up.

Rendering through Hydra is untested on Windows so far.

Licences
--------
MoonRay (OpenMoonRay) is Apache-2.0, by DreamWorks Animation. The WSL Containers SDK is
MIT, by Microsoft (its notices are in licenses\). OpenUSD (Tomorrow Open Source
Technology License, by Pixar), OpenVDB (Apache-2.0), Alembic (BSD-3-Clause) and the
libraries they use are built with vcpkg; their licences are in licenses\libraries\.
pfx-render.exe is based on
Microsoft's WSLC MoonRay sample (MIT). The test geometry's sources are in
licenses\test-geometry.md.

Houdini is a trademark of Side Effects Software Inc. ProceduralFX Studio is not
affiliated with or endorsed by SideFX.
