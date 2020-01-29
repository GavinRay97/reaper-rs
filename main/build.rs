use cc;

fn main() {
    #[cfg(not(windows))]
        generate_bindings();
    compile_glue();
}

fn compile_glue() {
    cc::Build::new()
        .cpp(true)
        .file("src/low_level/control_surface.cpp")
        .compile("glue");
}

fn generate_bindings() {
    // Tell cargo to invalidate the built crate whenever the wrapper changes
    println!("cargo:rerun-if-changed=src/low_level/bindgen.h");

    // The bindgen::Builder is the main entry point
    // to bindgen, and lets you build up options for
    // the resulting bindings.
    let bindings = bindgen::Builder::default()
        // The input header we would like to generate
        // bindings for.
        .header("src/low_level/bindgen.h")
        .opaque_type("timex")
        .clang_arg("-xc++")
        .enable_cxx_namespaces()
        .raw_line("#![allow(non_upper_case_globals)]")
        .raw_line("#![allow(non_camel_case_types)]")
        .raw_line("#![allow(non_snake_case)]")
        .whitelist_var("EnumProjects")
        .whitelist_var("GetTrack")
        .whitelist_var("ValidatePtr2")
        .whitelist_var("GetSetMediaTrackInfo")
        .whitelist_var("ShowConsoleMsg")
        .whitelist_var("REAPER_PLUGIN_VERSION")
        .whitelist_var("plugin_register")
        .whitelist_type("HINSTANCE")
        .whitelist_type("reaper_plugin_info_t")
        .whitelist_type("gaccel_register_t")
        .whitelist_function("reaper_rs_control_surface::.*")
        // Tell cargo to invalidate the built crate whenever any of the
        // included header files changed. TODO Do as soon as available
//        .parse_callbacks(Box::new(bindgen::CargoCallbacks))
        // Finish the builder and generate the bindings.
        .generate()
        // Unwrap the Result and panic on failure.
        .expect("Unable to generate bindings");

    // Write the bindings to the bindings.rs file.
    let out_path = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    bindings
        .write_to_file(out_path.join("src/low_level/bindings.rs"))
        .expect("Couldn't write bindings!");
}