fn main() {
    windows_resources();
    println!("cargo:rerun-if-changed=native/openvdb_bridge.cpp");
    println!("cargo:rerun-if-changed=native/usd_bridge.cpp");
    println!("cargo:rerun-if-env-changed=USD_ROOT");
    println!("cargo:rerun-if-env-changed=USD_PYTHON_INCLUDE");
    if std::env::var_os("CARGO_FEATURE_NATIVE_USD").is_some() {
        build_usd_bridge();
    }
    println!("cargo:rerun-if-changed=native/abc_bridge.cpp");
    println!("cargo:rerun-if-env-changed=ALEMBIC_ROOT");
    if std::env::var_os("CARGO_FEATURE_NATIVE_ALEMBIC").is_some() {
        build_alembic_bridge();
    }
    if msvc() && ["USD", "ALEMBIC", "OPENVDB"].iter().any(|f| std::env::var_os(format!("CARGO_FEATURE_NATIVE_{f}")).is_some()) {
        link_windows_libraries();
    }
    if std::env::var_os("CARGO_FEATURE_NATIVE_OPENVDB").is_none() {
        return;
    }

    let mut build = cc::Build::new();
    build.cpp(true).file("native/openvdb_bridge.cpp");
    build.flag_if_supported("-std=c++17");
    if msvc() {
        let root = windows_root("OPENVDB_ROOT", "include/openvdb/openvdb.h");
        msvc_flags(&mut build).include(root.join("include")).define("OPENVDB_DLL", None);
        build.compile("proceduralfx_studio_openvdb");
        return;
    }

    let tbb = pkg_config::Config::new().probe("tbb").unwrap_or_else(|error| {
        panic!(
            "native-openvdb requires oneTBB. Install it with `brew install tbb`, \
             or set PKG_CONFIG_PATH to the directory containing tbb.pc. \
             pkg-config error: {error}"
        )
    });
    for path in tbb.include_paths {
        build.include(path);
    }
    for library in tbb.libs {
        println!("cargo:rustc-link-lib={library}");
    }
    for path in tbb.link_paths {
        println!("cargo:rustc-link-search=native={}", path.display());
    }

    if let Some(root) = std::env::var_os("OPENVDB_ROOT") {
        let root = std::path::PathBuf::from(root);
        let include = root.join("include");
        let lib = root.join("lib");
        if !include.exists() || !lib.exists() {
            panic!(
                "OPENVDB_ROOT must contain include/ and lib/ directories; got {}",
                root.display()
            );
        }
        build.include(&include);
        println!("cargo:rustc-link-search=native={}", lib.display());
        println!("cargo:rustc-link-lib=openvdb");
        // OpenVDB's headers reference Imath's half tables directly; ELF linkers
        // don't resolve those through libopenvdb, so link Imath too.
        let _ = pkg_config::Config::new().probe("Imath");
    } else {
        let openvdb = pkg_config::Config::new().probe("openvdb").unwrap_or_else(|error| {
            panic!(
                "native-openvdb could not find OpenVDB. Install it with \
                 `brew install openvdb pkgconf`, then run again; or set \
                 OPENVDB_ROOT=/path/to/openvdb. If openvdb.pc is in a custom \
                 location, set PKG_CONFIG_PATH=/path/to/pkgconfig:$PKG_CONFIG_PATH. \
                 pkg-config error: {error}"
            )
        });
        for path in openvdb.include_paths {
            build.include(path);
        }
        for library in openvdb.libs {
            println!("cargo:rustc-link-lib={library}");
        }
        for path in openvdb.link_paths {
            println!("cargo:rustc-link-search=native={}", path.display());
        }
    }

    build.compile("proceduralfx_studio_openvdb");
}

// OpenUSD for the STAGE context. USD_ROOT is an OpenUSD install (include/, lib/);
// its headers need the Python headers USD was built against (USD_PYTHON_INCLUDE).
fn build_usd_bridge() {
    if msvc() {
        // No Python in the Windows build of USD; its libraries are linked with the rest.
        let root = windows_root("USD_ROOT", "include/pxr/pxr.h");
        msvc_flags(cc::Build::new().cpp(true).file("native/usd_bridge.cpp")).include(root.join("include")).compile("proceduralfx_studio_usd");
        return;
    }
    let root = std::path::PathBuf::from(
        std::env::var_os("USD_ROOT").unwrap_or_else(|| "/Users/loksangho/openusd-build".into()),
    );
    let (include, lib) = (root.join("include"), root.join("lib"));
    if !include.join("pxr/pxr.h").exists() || !lib.exists() {
        panic!(
            "native-usd needs an OpenUSD install: set USD_ROOT to a directory with include/pxr and lib/; got {}",
            root.display()
        );
    }
    // All of python-config's include dirs: on Debian-style layouts pyconfig.h
    // lives in a separate per-arch directory from the other headers.
    let python: Vec<std::path::PathBuf> = match std::env::var_os("USD_PYTHON_INCLUDE") {
        Some(dir) => vec![dir.into()],
        None => ["python3.15-config", "python3-config"]
            .into_iter()
            .find_map(|config| {
                let output = std::process::Command::new(config).arg("--includes").output().ok()?;
                let dirs: Vec<std::path::PathBuf> = String::from_utf8(output.stdout)
                    .ok()?
                    .split_whitespace()
                    .filter_map(|flag| flag.strip_prefix("-I"))
                    .map(std::path::PathBuf::from)
                    .collect();
                dirs.iter().any(|dir| dir.join("pyconfig.h").exists()).then_some(dirs)
            })
            .unwrap_or_default(),
    };
    let mut build = cc::Build::new();
    build
        .cpp(true)
        .file("native/usd_bridge.cpp")
        .include(&include)
        .flag_if_supported("-std=c++17")
        .flag_if_supported("-Wno-deprecated-declarations")
        .flag_if_supported("-Wno-unused-parameter");
    for dir in python {
        build.include(dir);
    }
    if let Ok(tbb) = pkg_config::Config::new().cargo_metadata(false).probe("tbb") {
        for path in tbb.include_paths {
            build.include(path);
        }
    }
    build.compile("proceduralfx_studio_usd");
    println!("cargo:rustc-link-search=native={}", lib.display());
    for library in ["usd_usdUtils", "usd_usdShade", "usd_usdLux", "usd_usdGeom", "usd_usd", "usd_pcp", "usd_sdf", "usd_ar", "usd_vt", "usd_gf", "usd_tf", "usd_python", "usd_arch"] {
        // usd_python only exists in builds with Python support.
        let present = ["so", "dylib"].iter().any(|ext| lib.join(format!("lib{library}.{ext}")).exists());
        if library != "usd_python" || present {
            println!("cargo:rustc-link-lib=dylib={library}");
        }
    }
    // USD libraries use @rpath install names.
    println!("cargo:rustc-link-arg=-Wl,-rpath,{}", lib.display());
}

// Alembic for .abc import and export. ALEMBIC_ROOT is an install (include/Alembic,
// lib/libAlembic) that also has Imath, e.g. the macOS 14 prefix or the Flatpak's /app.
fn build_alembic_bridge() {
    if msvc() {
        let root = windows_root("ALEMBIC_ROOT", "include/Alembic/AbcGeom/All.h");
        let include = root.join("include");
        msvc_flags(cc::Build::new().cpp(true).file("native/abc_bridge.cpp"))
            .include(&include)
            .include(include.join("Imath"))
            .define("ALEMBIC_DLL", None)
            .compile("proceduralfx_studio_abc");
        return;
    }
    let root = std::path::PathBuf::from(std::env::var_os("ALEMBIC_ROOT").or_else(|| std::env::var_os("USD_ROOT")).unwrap_or_else(|| "/usr/local".into()));
    let (include, lib) = (root.join("include"), root.join("lib"));
    if !include.join("Alembic/AbcGeom/All.h").exists() {
        panic!("native-alembic needs an Alembic install: set ALEMBIC_ROOT to a directory with include/Alembic and lib/; got {}", root.display());
    }
    cc::Build::new()
        .cpp(true)
        .file("native/abc_bridge.cpp")
        .include(&include)
        .include(include.join("Imath"))
        .flag_if_supported("-std=c++17")
        .flag_if_supported("-Wno-deprecated-declarations")
        .compile("proceduralfx_studio_abc");
    println!("cargo:rustc-link-search=native={}", lib.display());
    println!("cargo:rustc-link-lib=static=Alembic");
    println!("cargo:rustc-link-lib=dylib=Imath");
    println!("cargo:rustc-link-arg=-Wl,-rpath,{}", lib.display());
}

// Windows (MSVC): USD, OpenVDB and Alembic come from one vcpkg install
// (packaging/windows/deps, built by the "Windows libraries" workflow), as DLLs.
fn msvc() -> bool {
    std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc")
}

// The vcpkg install a feature's *_ROOT names, checked for one of its headers.
fn windows_root(variable: &str, header: &str) -> std::path::PathBuf {
    println!("cargo:rerun-if-env-changed={variable}");
    let root = std::path::PathBuf::from(std::env::var_os(variable).unwrap_or_else(|| panic!("set {variable} to the Windows libraries' folder (pfx-windows-deps.zip, unpacked)")));
    if !root.join(header).is_file() {
        panic!("{variable} has no {header}: {}", root.display());
    }
    root
}

fn msvc_flags(build: &mut cc::Build) -> &mut cc::Build {
    build
        .flag("/std:c++17")
        .flag("/bigobj")
        .flag("/Zc:__cplusplus")
        .flag("/utf-8")
        .flag("/wd4244")
        .flag("/wd4267")
        .flag("/wd4996")
        .define("NOMINMAX", None)
        // Imath is a DLL: its half-float tables are imported data.
        .define("IMATH_DLL", None)
        .define("_USE_MATH_DEFINES", None)
        .define("BOOST_ALL_NO_LIB", None)
        .define("BOOST_ALL_DYN_LINK", None)
}

// Every import library in the install: the linker only keeps the DLLs the bridges
// actually use, so this saves listing each one's (versioned) name.
fn link_windows_libraries() {
    let root = ["USD_ROOT", "OPENVDB_ROOT", "ALEMBIC_ROOT"].iter().find_map(std::env::var_os).map(std::path::PathBuf::from).expect("set USD_ROOT, OPENVDB_ROOT or ALEMBIC_ROOT");
    let lib = root.join("lib");
    println!("cargo:rustc-link-search=native={}", lib.display());
    let mut names: Vec<String> = std::fs::read_dir(&lib)
        .unwrap_or_else(|e| panic!("no lib folder in {}: {e}", root.display()))
        .filter_map(Result::ok)
        .filter_map(|e| e.file_name().to_str()?.strip_suffix(".lib").map(str::to_owned))
        // tbbmalloc_proxy would replace malloc if anything pulled it in.
        .filter(|name| !name.starts_with("tbbmalloc_proxy"))
        .collect();
    names.sort();
    for name in names {
        println!("cargo:rustc-link-lib=dylib={name}");
    }
}

// The Windows app's icon and file details.
#[cfg(windows)]
fn windows_resources() {
    println!("cargo:rerun-if-changed=packaging/windows/app.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut resources = winresource::WindowsResource::new();
    resources.set_icon("packaging/windows/app.ico");
    resources.set("ProductName", "ProceduralFX Studio");
    resources.set("FileDescription", "ProceduralFX Studio");
    if let Err(error) = resources.compile() {
        println!("cargo:warning=no Windows icon: {error}");
    }
}

#[cfg(not(windows))]
fn windows_resources() {}
