fn main() {
    // Embed the app icon into guardian-agent.exe on Windows.
    #[cfg(windows)]
    {
        let mut res = winres::WindowsResource::new();
        res.set_icon("../assets/guardian.ico");
        res.compile().expect("failed to compile Windows resources");
    }
}
