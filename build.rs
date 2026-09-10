fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        winresource::WindowsResource::new()
            .set_icon("resources/icons/cowboy_gear_cat_full.ico")
            .compile()
            .expect("failed to embed exe icon");
    }
}
