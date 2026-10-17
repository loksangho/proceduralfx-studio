# ProceduralFX Studio

A node-based procedural 3D tool in the spirit of Houdini. You build geometry
from networks of nodes, and the app includes:

- modelling nodes and VEX wrangles;
- volumes and OpenVDB;
- Pyro, FLIP, RBD and SOP solvers;
- a USD stage context;
- keyframe animation with a graph editor;
- rendering with [MoonRay](https://openmoonray.org).

Downloads are on the [Releases page](https://github.com/loksangho/proceduralfx-studio/releases/latest).

| Platform | Download | Requirements |
|---|---|---|
| macOS | [ProceduralFxStudio.dmg](https://github.com/loksangho/proceduralfx-studio/releases/latest/download/ProceduralFxStudio.dmg) | Apple Silicon Mac, macOS 27 or later |
| Linux | [ProceduralFxStudio-x86_64.flatpak](https://github.com/loksangho/proceduralfx-studio/releases/latest/download/ProceduralFxStudio-x86_64.flatpak) | x86_64 CPU with AVX2 (2013 or newer), Flatpak |
| Windows | Use the Linux build under WSL 2 (see below) | Windows 11, or Windows 10 22H2 with the Store version of WSL |

Once installed, the app checks for new versions at startup and offers to update
itself (**Help → Check for Updates…**).

## macOS

1. Download [ProceduralFxStudio.dmg](https://github.com/loksangho/proceduralfx-studio/releases/latest/download/ProceduralFxStudio.dmg) and open it.
2. Drag **ProceduralFX Studio** into **Applications**.
3. Open it from Applications.

The app is signed and notarized by Apple. The first time you open it, macOS
may ask you to confirm opening an app downloaded from the internet; click
**Open**. Run it from Applications rather than from the disk image, or it can't
update itself.

## Linux

The Linux build is a Flatpak bundle and runs on most distributions.

1. Install Flatpak if you don't have it ([setup guide for each distribution](https://flatpak.org/setup/)), then add Flathub:
   ```sh
   flatpak remote-add --user --if-not-exists flathub https://dl.flathub.org/repo/flathub.flatpakrepo
   ```
2. Download and install the app. The first install also downloads the GNOME runtime from Flathub, about 1 GB.
   ```sh
   curl -LO https://github.com/loksangho/proceduralfx-studio/releases/latest/download/ProceduralFxStudio-x86_64.flatpak
   flatpak install --user ./ProceduralFxStudio-x86_64.flatpak
   ```
3. Start it from your applications menu, or with:
   ```sh
   flatpak run com.loksangho.proceduralfxstudio
   ```

To uninstall: `flatpak uninstall --user com.loksangho.proceduralfxstudio`.

## Windows (WSL)

On Windows, the Linux build runs inside WSL 2, and its window appears on your
desktop.

1. In **PowerShell as Administrator**, install Ubuntu, then restart if asked:
   ```powershell
   wsl --install -d Ubuntu-24.04
   wsl --update
   ```
2. Open **Ubuntu** from the Start menu, create a user, then install Flatpak and the app:
   ```sh
   sudo apt update && sudo apt install -y flatpak
   flatpak remote-add --user --if-not-exists flathub https://dl.flathub.org/repo/flathub.flatpakrepo
   curl -LO https://github.com/loksangho/proceduralfx-studio/releases/latest/download/ProceduralFxStudio-x86_64.flatpak
   flatpak install --user ./ProceduralFxStudio-x86_64.flatpak
   ```
3. WSL needs two display settings. Set them once:
   ```sh
   flatpak override --user --nosocket=wayland --socket=x11 --env=GALLIUM_DRIVER=llvmpipe com.loksangho.proceduralfxstudio
   ```
4. Run it:
   ```sh
   flatpak run com.loksangho.proceduralfxstudio
   ```

The app can open files in your Linux home folder. To also reach your Windows
drive (`C:`), run `flatpak override --user --filesystem=/mnt/c com.loksangho.proceduralfxstudio`.
Files kept on the Linux side are much faster to work with.

### WSL troubleshooting

| Symptom | Fix |
|---|---|
| `MESA: error: ZINK: failed to choose pdev` | Harmless if the window opens. Otherwise make sure step 3 was run. |
| `IO error: Broken pipe` at startup | WSLg's Wayland connection dropped. Step 3 switches the app to X11. |
| Slow viewport | Step 3 uses software OpenGL, because WSL's GPU isn't reachable from Flatpak. MoonRay renders run on the CPU either way, so they're unaffected. |

## Building from source

The app is written in Rust ([egui](https://github.com/emilk/egui)). A plain
build, without the OpenVDB, USD and MoonRay integrations, needs only a Rust
toolchain:

```sh
cargo run --release
```

The full builds include native OpenVDB and USD, plus a bundled MoonRay:

- **macOS:** `./proceduralfx_studio.sh package-macos` builds the app bundle in `dist/`.
- **Linux:** [packaging/linux](packaging/linux) contains the Flatpak manifest.
  [.github/workflows/flatpak.yml](.github/workflows/flatpak.yml) builds it on
  GitHub Actions, and publishes a release when a `v*` tag is pushed.
