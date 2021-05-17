mod common;

use std::env;
use dotenv::*;

use crate::common::*;

#[test]
fn test_variable_substitutions() {
    std::env::set_var("KEY", "value");
    std::env::set_var("KEY1", "value1");

    let substitutions_to_test = [
        "$ZZZ",
        "$KEY",
        "$KEY1",
        "${KEY}1",
        "$KEY_U",
        "${KEY_U}",
        "\\$KEY"
    ];

    let common_string = substitutions_to_test.join(">>");
    let dir = tempdir_with_dotenv(&format!(r#"
KEY1=new_value1
KEY_U=$KEY+valueU

SUBSTITUTION_FOR_STRONG_QUOTES='{}'
SUBSTITUTION_FOR_WEAK_QUOTES="{}"
SUBSTITUTION_WITHOUT_QUOTES={}
"#, common_string, common_string, common_string)).unwrap();

    assert_eq!(var("KEY").unwrap(), "value");
    assert_eq!(var("KEY1").unwrap(), "value1");
    assert_eq!(var("KEY_U").unwrap(), "value+valueU");
    assert_eq!(var("SUBSTITUTION_FOR_STRONG_QUOTES").unwrap(), common_string);
    assert_eq!(var("SUBSTITUTION_FOR_WEAK_QUOTES").unwrap(), [
        "",
        "value",
        "value1",
        "value1",
        "value_U",
        "value+valueU",
        "$KEY"
    ].join(">>"));
    assert_eq!(var("SUBSTITUTION_WITHOUT_QUOTES").unwrap(), [
        "",
        "value",
        "value1",
        "value1",
        "value_U",
        "value+valueU",
        "$KEY"
    ].join(">>"));

    env::set_current_dir(dir.path().parent().unwrap()).unwrap();
    dir.close().unwrap();
}