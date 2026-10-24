use atuin_client::settings::Settings;
use atuin_common::out::outln;

pub fn run() {
    outln!("{}", Settings::example_config());
}
