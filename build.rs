fn main() {
    println!("cargo:rerun-if-changed=native/openvdb_bridge.cpp");
    println!("cargo:rerun-if-changed=native/usd_bridge.cpp");
    println!("cargo:rerun-if-env-changed=USD_ROOT");
    println!("cargo:rerun-if-env-changed=USD_PYTHON_INCLUDE");
    if std::env::var_os("CARGO_FEATURE_NATIVE_USD").is_some() {
        build_usd_bridge();
    }
    if std::env::var_os("CARGO_FEATURE_NATIVE_OPENVDB").is_none() {
        return;
    }

    let mut build = cc::Build::new();
    build.cpp(true).file("native/openvdb_bridge.cpp");
    build.flag_if_supported("-std=c++17");

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
