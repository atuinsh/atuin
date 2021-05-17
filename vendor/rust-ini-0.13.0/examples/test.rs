extern crate ini;

use std::io::stdout;

use ini::Ini;

const CONF_FILE_NAME: &'static str = "test.ini";

fn main() {
    let mut conf = Ini::new();
    conf.with_section(None::<String>).set("encoding", "utf-8");
    conf.with_section(Some("User")).set("name", "Raspberry树莓")
        .set("value", "Pi");
    conf.with_section(Some("Library")).set("name", "Sun Yat-sen U")
        .set("location", "Guangzhou=world\x0ahahaha");

    conf.section_mut(Some("Library")).unwrap()
        .insert("seats".into(), "42".into());

    println!("---------------------------------------");
    println!("Writing to file {:?}\n", CONF_FILE_NAME);
    conf.write_to(&mut stdout()).unwrap();

    conf.write_to_file(CONF_FILE_NAME).unwrap();

    println!("----------------------------------------");
    println!("Reading from file {:?}", CONF_FILE_NAME);
    let i = Ini::load_from_file(CONF_FILE_NAME).unwrap();

    println!("Iterating");
    let general_section_name = "__General__".into();
    for (sec, prop) in i.iter() {
        let section_name = sec.as_ref().unwrap_or(&general_section_name);
        println!("-- Section: {:?} begins", section_name);
        for (k, v) in prop.iter() {
            println!("{}: {:?}", *k, *v);
        }
    }
    println!("");

    let section = i.section(Some("User")).unwrap();
    println!("name={}", section.get("name").unwrap());
    println!("conf[{}][{}]={}", "User", "name", i["User"]["name"]);
    println!("General Section: {:?}", i.general_section());
}
