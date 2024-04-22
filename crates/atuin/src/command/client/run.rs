use eyre::Result;

use atuin_run::markdown::parse;

pub fn run() -> Result<()> {
    let blocks = parse(
        "
1. do a thing
```sh
echo 'foo'
```

2. do another thing
```sh
echo 'bar'
```
",
    );

    println!("{:?}", blocks);
    Ok(())
}
