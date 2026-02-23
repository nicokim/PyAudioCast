fn main() {
    // On Linux, compile a small C shim for suppressing ALSA error output
    #[cfg(target_os = "linux")]
    {
        cc::Build::new()
            .file("csrc/alsa_suppress.c")
            .compile("alsa_suppress");
    }
}
