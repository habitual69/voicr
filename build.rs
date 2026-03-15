fn main() {
    // Embed icon and version info into Windows executables
    #[cfg(target_os = "windows")]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("icons/voicr.ico");
        res.set("ProductName", "Voicr");
        res.set("FileDescription", "Headless speech-to-text daemon and CLI");
        res.compile().expect("Failed to compile Windows resources");
    }
}
