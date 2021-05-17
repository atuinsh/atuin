extern crate config;

use config::*;
use std::env;

#[test]
fn test_default() {
    env::set_var("A_B_C", "abc");

    let environment = Environment::new();

    assert!(environment.collect().unwrap().contains_key("a_b_c"));

    env::remove_var("A_B_C");
}

#[test]
fn test_prefix_is_removed_from_key() {
    env::set_var("B_A_C", "abc");

    let environment = Environment::with_prefix("B");

    assert!(environment.collect().unwrap().contains_key("a_c"));

    env::remove_var("B_A_C");
}

#[test]
fn test_prefix_with_variant_forms_of_spelling() {
    env::set_var("a_A_C", "abc");

    let environment = Environment::with_prefix("a");

    assert!(environment.collect().unwrap().contains_key("a_c"));

    env::remove_var("a_A_C");
    env::set_var("aB_A_C", "abc");

    let environment = Environment::with_prefix("aB");

    assert!(environment.collect().unwrap().contains_key("a_c"));

    env::remove_var("aB_A_C");
    env::set_var("Ab_A_C", "abc");

    let environment = Environment::with_prefix("ab");

    assert!(environment.collect().unwrap().contains_key("a_c"));

    env::remove_var("Ab_A_C");
}

#[test]
fn test_separator_behavior() {
    env::set_var("C_B_A", "abc");

    let environment = Environment::with_prefix("C").separator("_");

    assert!(environment.collect().unwrap().contains_key("b.a"));

    env::remove_var("C_B_A");
}

#[test]
fn test_empty_value_is_ignored() {
    env::set_var("C_A_B", "");

    let environment = Environment::new().ignore_empty(true);

    assert!(!environment.collect().unwrap().contains_key("c_a_b"));

    env::remove_var("C_A_B");
}
