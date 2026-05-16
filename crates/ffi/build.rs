fn main() {
    cc::Build::new()
        .file("c/vendor_ticks.c")
        .warnings(true)
        .compile("vendor_ticks");
}
