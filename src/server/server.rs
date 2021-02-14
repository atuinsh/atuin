#[get("/")]
fn index() -> &'static str {
    "Hello, world!"
}

pub fn launch() {
    rocket::ignite().mount("/", routes![index]).launch();
}
