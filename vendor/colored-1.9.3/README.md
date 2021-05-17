# Colored

[![Build
Status](https://travis-ci.org/mackwic/colored.svg?branch=master)](https://travis-ci.org/mackwic/colored) [![Crates.io](https://img.shields.io/crates/v/colored.svg?maxAge=2592000)](https://crates.io/crates/colored) [![Crates.io](https://img.shields.io/crates/l/colored.svg?maxAge=2592000)](https://github.com/mackwic/colored/blob/master/LICENSE)

Coloring terminal so simple, you already know how to do it!

```rust
    "this is blue".blue();
    "this is red".red();
    "this is red on blue".red().on_blue();
    "this is also red on blue".on_blue().red();
    "bright colors are welcome as well".on_bright_blue().bright_red();
    "you can also make bold comments".bold();
    println!("{} {} {}", "or use".cyan(), "any".italic().yellow(), "string type".cyan());
    "or change advice. This is red".yellow().blue().red();
    "or clear things up. This is default color and style".red().bold().clear();
    "purple and magenta are the same".purple().magenta();
    "and so are normal and clear".normal().clear();
    "you can specify color by string".color("blue").on_color("red");
    String::from("this also works!").green().bold();
    format!("{:30}", "format works as expected. This will be padded".blue());
    format!("{:.3}", "and this will be green but truncated to 3 chars".green());
```

## How to use

Add this in your `Cargo.toml`:

```toml
[dependencies]
colored = "1.9"
```

and add this to your `lib.rs` or `main.rs`:

```rust
    extern crate colored; // not needed in Rust 2018
    
    use colored::*;

    // test the example with `cargo run --example most_simple`
    fn main() {
        // TADAA!
        println!("{} {} !", "it".green(), "works".blue().bold());
    }
```

## Features

- Safe rust, easy to use, minimal dependencies, complete test suite
- Respect the `CLICOLOR`/`CLICOLOR_FORCE` behavior (see [the specs](http://bixense.com/clicolors/))
- Respect the `NO_COLOR` behavior (see [the specs](https://no-color.org/))
- Works on Linux, MacOS, and Windows (Powershell)

#### Colors:

- black
- red
- green
- yellow
- blue
- magenta (or purple)
- cyan
- white

Bright colors: prepend the color by `bright_`. So easy.
Background colors: prepend the color by `on_`. Simple as that.
Bright Background colors: prepend the color by `on_bright_`. Not hard at all.

#### Styles:

- bold
- underline
- italic
- dimmed
- reversed
- blink
- hidden
- strikethrough

You can clear color _and_ style anytime by using `normal()` or `clear()`

#### Advanced Control:

##### Dynamic color from str

As `Color` implements `FromStr`, `From<&str>`, and `From<String>`, you can easily cast a string into a color like that:

```rust
// the easy way
"blue string yo".color("blue");

// this will default to white
"white string".color("zorglub");

// the safer way via a Result
let color_res : Result<Color, ()> = "zorglub".parse();
"red string".color(color_res.unwrap_or(Color::Red));
```


##### Colorization control

If you want to disable any coloring at compile time, you can simply do so by
using the `no-color` feature.

For example, you can do this in your `Cargo.toml` to disable color in tests:

```toml
[features]
# this effectively enable the feature `no-color` of colored when testing with
# `cargo test --feature dumb_terminal`
dumb_terminal = ["colored/no-color"]
```

You can use have even finer control by using the
`colored::control::set_override` method.

## Build with Docker

### Install Docker

Use the install instructions located [here](https://docs.docker.com/v17.12/install/)

### Build the Docker image

```docker build -t colored_image .```

### Build the library

```docker run --rm -it -v "$PWD":/src -u `id -u`:`id -g` colored_image /bin/bash -c "cargo build"```

### Test the library

```docker run --rm -it -v "$PWD":/src -u `id -u`:`id -g` colored_image /bin/bash -c "cargo test"```


## Todo

- **More tests ?**: We always welcome more tests! Please contribute!

## Credits

This library wouldn't have been the same without the marvelous ruby gem [colored](https://github.com/defunkt/colored).

Thanks for the [ansi\_term crate](https://github.com/ogham/rust-ansi-term) for
providing a reference implementation, which greatly helped making this crate
output correct strings.

## License

[Mozilla Public License 2.0](https://www.mozilla.org/en-US/MPL/2.0/). See the
[LICENSE](https://github.com/mackwic/colored/blob/master/LICENSE) file at the
root of the repository.

In non legal terms it means that:
- if you fix a bug, you MUST give me the code of the fix (it's only fair)
- if you change/extend the API, you MUST give me the code you changed in the
  files under MPL2.
- you CAN'T sue me for anything about this code
- apart from that, you can do almost whatever you want. See the LICENSE file
  for details.

## Contributors

- Thomas Wickham: [@mackwic](https://github.com/mackwic)
- Corey "See More" Richardson: [@cmr](https://github.com/cmr)
- Iban Eguia: [@Razican](https://github.com/Razican)
- Alexis "Horgix" Chotard: [@horgix](https://github.com/horgix)
- Keith Yeung: [@KiChjang](https://github.com/KiChjang)
- Kyle Galloway: [@kylegalloway](https://github.com/kylegalloway)
- Luke Hsiao: [@lukehsiao](https://github.com/lukehsiao)
- kurtlawrence: [@kurtlawrence](https://github.com/kurtlawrence)

