<p align="center"><img src="packaging/logo/icon-256.png" width="128" alt="ProceduralFX Studio logo"></p>

# ProceduralFX Studio

<a href="https://buymeacoffee.com/sangstari"><img src="https://img.buymeacoffee.com/button-api/?text=Buy%20me%20a%20coffee&emoji=%E2%98%95&slug=sangstari&button_colour=FFDD00&font_colour=000000&font_family=Inter&outline_colour=000000&coffee_colour=ffffff" height="40" alt="Buy me a coffee"></a>

A node-based procedural 3D tool for macOS, Windows and Linux. You build geometry
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
| macOS | [ProceduralFxStudio.dmg](https://github.com/loksangho/proceduralfx-studio/releases/latest/download/ProceduralFxStudio.dmg) | Apple Silicon Mac, macOS 14 or later |
| Linux | [ProceduralFxStudio-x86_64.flatpak](https://github.com/loksangho/proceduralfx-studio/releases/latest/download/ProceduralFxStudio-x86_64.flatpak) | x86_64 CPU with AVX2 (2013 or newer), Flatpak |
| Windows | [ProceduralFxStudio-windows-x64.zip](https://github.com/loksangho/proceduralfx-studio/releases/latest/download/ProceduralFxStudio-windows-x64.zip) | Windows 11 (x64) with virtualization turned on |

Once installed, the app checks for new versions at startup and offers to update
itself (**Help → Check for Updates…**). Windows 0.12.0 links to the new download
instead; from 0.13.0 Windows updates itself too.

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

## Windows

1. Download [ProceduralFxStudio-windows-x64.zip](https://github.com/loksangho/proceduralfx-studio/releases/latest/download/ProceduralFxStudio-windows-x64.zip) and unzip it anywhere.
2. Run **ProceduralFX Studio.exe**. Keep the files in the folder together.
3. To render, choose **Render → Set Up Rendering on Windows…** once.

MoonRay is a Linux program, so on Windows it runs in a small Linux container
through WSL. The set-up window checks for WSL, then downloads MoonRay (about
1.3 GB, once). If WSL isn't installed, the window installs it: Windows asks an
administrator to approve that once, and a restart may be needed. Downloading
and rendering never need administrator rights.

## Support

ProceduralFX Studio is free. If it's useful to you, you can support its
development with a coffee: [buymeacoffee.com/sangstari](https://buymeacoffee.com/sangstari).

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

## License

ProceduralFX Studio is **source-available** under the
[PolyForm Noncommercial License 1.0.0](https://polyformproject.org/licenses/noncommercial/1.0.0).
See [LICENSE](LICENSE).

**Free for noncommercial use.** Learning, personal projects, hobby games,
research, and use by schools, charities and other noncommercial organizations
are all free. You can read, build, modify and share the code for those purposes.

**Commercial use needs a commercial license.** If you use ProceduralFX Studio
to make something you sell or earn money from (for example, effects for a
commercial game), please get a commercial license: email [support@build3dstudio.com](mailto:support@build3dstudio.com).

**What stays true whatever license you have:**

- Everything you make with the tool belongs to you.
- Your files use an open, documented format.
- Your work is never used to train AI models.

---

Familiar if you've used Houdini. Houdini is a trademark of Side Effects Software Inc.
ProceduralFX Studio is not affiliated with or endorsed by SideFX.
