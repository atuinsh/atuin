use miette::SourceCode;

#[test]
fn substr_span() {
    // Make the file
    let contents = String::from("hello !there!");
    let source = axoasset::SourceFile::new("file.md", contents);

    // Do some random parsing operation
    let mut parse = source.contents().split('!');
    let _ = parse.next();
    let there = parse.next().unwrap();

    // Get the span
    let there_span = source.span_for_substr(there).unwrap();

    // Assert the span is correct
    let span_bytes = source.read_span(&there_span, 0, 0).unwrap().data();
    assert_eq!(std::str::from_utf8(span_bytes).unwrap(), "there");
}

#[test]
fn substr_span_invalid() {
    // Make the file
    let contents = String::from("hello !there!");
    let source = axoasset::SourceFile::new("file.md", contents);

    // Get the span for a non-substring (string literal isn't pointing into the String)
    let there_span = source.span_for_substr("there");
    assert_eq!(there_span, None);
}

#[cfg(feature = "json-serde")]
#[test]
fn json_valid() {
    #[derive(serde::Deserialize, PartialEq, Eq, Debug)]
    struct MyType {
        hello: String,
        goodbye: bool,
    }

    // Make the file
    let contents = String::from(r##"{ "hello": "there", "goodbye": true }"##);
    let source = axoasset::SourceFile::new("file.js", contents);

    // Get the span for a non-substring (string literal isn't pointing into the String)
    let val = source.deserialize_json::<MyType>().unwrap();
    assert_eq!(
        val,
        MyType {
            hello: "there".to_string(),
            goodbye: true
        }
    );
}

#[cfg(feature = "json-serde")]
#[test]
fn json_with_bom() {
    #[derive(serde::Deserialize, PartialEq, Eq, Debug)]
    struct MyType {
        hello: String,
        goodbye: bool,
    }

    // Make the file
    let contents =
        String::from("\u{FEFF}") + &String::from(r##"{ "hello": "there", "goodbye": true }"##);
    let source = axoasset::SourceFile::new("file.js", contents);

    // Get the span for a non-substring (string literal isn't pointing into the String)
    let val = source.deserialize_json::<MyType>().unwrap();
    assert_eq!(
        val,
        MyType {
            hello: "there".to_string(),
            goodbye: true
        }
    );
}

#[cfg(feature = "json-serde")]
#[test]
fn json_invalid() {
    use axoasset::AxoassetError;

    #[derive(serde::Deserialize, PartialEq, Eq, Debug)]
    struct MyType {
        hello: String,
        goodbye: bool,
    }

    // Make the file
    let contents = String::from(r##"{ "hello": "there", "goodbye": true, }"##);
    let source = axoasset::SourceFile::new("file.js", contents);

    // Get the span for a non-substring (string literal isn't pointing into the String)
    let res = source.deserialize_json::<MyType>();
    assert!(res.is_err());
    let Err(AxoassetError::Json { span: Some(_), .. }) = res else {
        panic!("span was invalid");
    };
}

#[cfg(feature = "toml-serde")]
#[test]
fn toml_valid() {
    #[derive(serde::Deserialize, PartialEq, Eq, Debug)]
    struct MyType {
        hello: String,
        goodbye: bool,
    }

    // Make the file
    let contents = String::from(
        r##"
hello = "there"
goodbye = true
"##,
    );
    let source = axoasset::SourceFile::new("file.toml", contents);

    // Get the span for a non-substring (string literal isn't pointing into the String)
    let val = source.deserialize_toml::<MyType>().unwrap();
    assert_eq!(
        val,
        MyType {
            hello: "there".to_string(),
            goodbye: true
        }
    );
}

#[cfg(feature = "toml-serde")]
#[test]
fn toml_invalid() {
    use axoasset::AxoassetError;

    #[derive(serde::Deserialize, PartialEq, Eq, Debug)]
    struct MyType {
        hello: String,
        goodbye: bool,
    }

    // Make the file
    let contents = String::from(
        r##"
hello = "there"
goodbye =
"##,
    );
    let source = axoasset::SourceFile::new("file.toml", contents);

    // Get the span for a non-substring (string literal isn't pointing into the String)
    let res = source.deserialize_toml::<MyType>();
    assert!(res.is_err());
    let Err(AxoassetError::Toml { span: Some(_), .. }) = res else {
        panic!("span was invalid");
    };
}

#[cfg(feature = "toml-edit")]
#[test]
fn toml_edit_valid() {
    // Make the file
    let contents = String::from(
        r##"
hello = "there"
goodbye = true
"##,
    );
    let source = axoasset::SourceFile::new("file.toml", contents);

    // Get the span for a non-substring (string literal isn't pointing into the String)
    let val = source.deserialize_toml_edit().unwrap();
    assert_eq!(val["hello"].as_str().unwrap(), "there");
    assert!(val["goodbye"].as_bool().unwrap());
}

#[cfg(feature = "toml-edit")]
#[test]
fn toml_edit_invalid() {
    use axoasset::AxoassetError;

    // Make the file
    let contents = String::from(
        r##"
hello = "there"
goodbye =
"##,
    );
    let source = axoasset::SourceFile::new("file.toml", contents);

    // Get the span for a non-substring (string literal isn't pointing into the String)
    let res = source.deserialize_toml_edit();
    assert!(res.is_err());
    let Err(AxoassetError::TomlEdit { span: Some(_), .. }) = res else {
        panic!("span was invalid");
    };
}

#[test]
#[cfg(feature = "yaml-serde")]
fn yaml_valid() {
    #[derive(serde::Deserialize, PartialEq, Eq, Debug)]
    struct MyType {
        hello: String,
        goodbye: bool,
    }

    // Make the file
    let contents = String::from(
        r##"
hello: "there"
goodbye: true
"##,
    );
    let source = axoasset::SourceFile::new("file.yaml", contents);

    let res = source.deserialize_yaml::<MyType>().unwrap();
    assert_eq!(res.hello, "there");
    assert!(res.goodbye);
}

#[test]
#[cfg(feature = "yaml-serde")]
fn yaml_invalid() {
    #[derive(serde::Deserialize, PartialEq, Eq, Debug)]
    struct MyType {
        hello: String,
        goodbye: bool,
    }

    // Make the file
    let contents = String::from(
        r##"
hello: "there"
goodbye: "this shouldn't be a string"
"##,
    );
    let source = axoasset::SourceFile::new("file.yml", contents);

    let res = source.deserialize_yaml::<MyType>();
    // In a previous version, we had an assertion that the span
    // was filled out here. Unfortunately, the span isn't always
    // available in the yaml library we had to switch to;
    // investigate other options in the future.
    assert!(res.is_err());
}
