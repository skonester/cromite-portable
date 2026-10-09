fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../app.ico");

    // Embed the Cromite icon and version metadata into the .exe (as Nomad's
    // launchers do). No-op on non-Windows targets.
    if std::env::var_os("CARGO_CFG_WINDOWS").is_some() {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("../app.ico");
        let ver = format!("{}.0", env!("CARGO_PKG_VERSION"));
        res.set("FileDescription", "Cromite Portable Updater");
        res.set("ProductName", "Cromite Portable");
        res.set("FileVersion", &ver);
        res.set("ProductVersion", &ver);
        res.set("OriginalFilename", "Cromite-Updater.exe");
        res.compile().expect("failed to compile Windows resources");
    }
}
