ProceduralFX Studio for Windows
===============================

Run "ProceduralFX Studio.exe". Keep the three files in this folder together:

  ProceduralFX Studio.exe   the app
  pfx-render.exe            runs MoonRay for the app (see Rendering)
  wslcsdk.dll               Microsoft's WSL Containers SDK, which pfx-render.exe uses

Rendering
---------
MoonRay is a Linux program, so on Windows it runs in a small Linux container through
WSL (the Windows Subsystem for Linux). The first time, choose
Render > Set Up Rendering on Windows... in the app. It installs WSL containers if
they're missing (turning on Virtual Machine Platform needs a restart) and downloads
MoonRay, a few GB, once.

Needs Windows 11 with a recent WSL, and virtualization turned on in the BIOS or UEFI
settings. Rendering uses up to three quarters of your memory and all your CPU cores.

The WSL Containers SDK is a preview from Microsoft, so a WSL update can occasionally
break rendering until the app catches up.

Not yet on Windows: binary USD (.usdc), OpenVDB (.vdb) and Alembic (.abc) files.
Rendering through Hydra is untested on Windows so far.

Licences
--------
MoonRay (OpenMoonRay) is Apache-2.0, by DreamWorks Animation. The WSL Containers SDK is
MIT, by Microsoft (its notices are in licenses\). pfx-render.exe is based on
Microsoft's WSLC MoonRay sample (MIT). The test geometry's sources are in
licenses\test-geometry.md.

Houdini is a trademark of Side Effects Software Inc. ProceduralFX Studio is not
affiliated with or endorsed by SideFX.
