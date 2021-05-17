/*!
Using `env_logger::Logger` and the `log::Log` trait directly.

This example doesn't rely on environment variables, or having a static logger installed.
*/

fn record() -> log::Record<'static> {
    let error_metadata = log::MetadataBuilder::new()
        .target("myApp")
        .level(log::Level::Error)
        .build();

    log::Record::builder()
        .metadata(error_metadata)
        .args(format_args!("Error!"))
        .line(Some(433))
        .file(Some("app.rs"))
        .module_path(Some("server"))
        .build()
}

fn main() {
    use log::Log;

    let stylish_logger = env_logger::Builder::new()
        .filter(None, log::LevelFilter::Error)
        .write_style(env_logger::WriteStyle::Always)
        .build();

    let unstylish_logger = env_logger::Builder::new()
        .filter(None, log::LevelFilter::Error)
        .write_style(env_logger::WriteStyle::Never)
        .build();

    stylish_logger.log(&record());
    unstylish_logger.log(&record());
}
