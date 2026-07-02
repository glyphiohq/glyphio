fn main() {
    // The `screencapturekit` crate ships a Swift bridge and depends on the Swift runtime
    // (`@rpath/libswift*.dylib`). On Command Line Tools-only machines the linker needs the
    // toolchain's Swift lib dirs to resolve those symbols at LINK time, while at RUN time the
    // Swift runtime must come from the OS location (served by the dyld shared cache) so we don't
    // load duplicate Swift class implementations. All of this is harmless when full Xcode is
    // installed (extra, possibly-unused search paths).
    if let Ok(out) = std::process::Command::new("xcrun").args(["-f", "swiftc"]).output() {
        if out.status.success() {
            let swiftc = String::from_utf8_lossy(&out.stdout);
            let p = std::path::Path::new(swiftc.trim());
            // .../usr/bin/swiftc  ->  .../usr
            if let Some(usr) = p.parent().and_then(|bin| bin.parent()) {
                for rel in ["lib/swift-5.5/macosx", "lib/swift/macosx"] {
                    let dir = usr.join(rel);
                    if dir.exists() {
                        // Link-time symbol resolution only (no rpath to these — see below).
                        println!("cargo:rustc-link-search=native={}", dir.display());
                    }
                }
            }
        }
    }
    // Run-time: resolve `@rpath/libswift*.dylib` from the OS Swift runtime, avoiding duplicate
    // class implementations that mixing in the toolchain's back-deploy copies would cause.
    println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");

    tauri_build::build();
}
