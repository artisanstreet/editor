fn main() {
    #[cfg(windows)]
    {
        let mut resource = winresource::WindowsResource::new();
        resource
            .set_icon(
                "../frontend/src/lib/assets/barekey/runtime-app-icons/foreground-gradient-symbol.ico",
            )
            .set("CompanyName", "Barekey")
            .set("FileDescription", "Artisan Broker")
            .set("InternalName", "Artisan Broker")
            .set("OriginalFilename", "Artisan Broker.exe")
            .set("ProductName", "Artisan");
        resource
            .compile()
            .expect("compile Artisan Broker resources");
    }
}
