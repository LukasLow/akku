//! The `akku` command line: a small project scaffolder.

use std::env;
use std::fs;
use std::path::Path;
use std::process::ExitCode;

fn main() -> ExitCode {
    match run(env::args().skip(1)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("akku: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: impl Iterator<Item = String>) -> Result<(), String> {
    let args: Vec<String> = args.collect();
    match args.as_slice() {
        [] => {
            print_help();
            Ok(())
        }
        [flag] if flag == "--help" || flag == "-h" || flag == "help" => {
            print_help();
            Ok(())
        }
        [flag] if flag == "--version" || flag == "-V" => {
            println!("akku {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        [command, name] if command == "new" => new_project(name),
        [command, ..] if command == "new" => Err("usage: akku new <name>".to_string()),
        [command, ..] => Err(format!("unknown command `{command}`; try `akku --help`")),
    }
}

fn new_project(name: &str) -> Result<(), String> {
    if !valid_name(name) {
        return Err(format!(
            "`{name}` is not a valid project name; use letters, digits, `_` or `-`"
        ));
    }
    let root = Path::new(name);
    if root.exists() {
        return Err(format!("`{name}` already exists"));
    }
    fs::create_dir_all(root.join("src"))
        .map_err(|error| format!("cannot create `{name}`: {error}"))?;
    let manifest = format!(
        r#"[package]
name = "{name}"
version = "0.1.0"
edition = "2021"

[dependencies]
akku = "{version}"
"#,
        version = env!("CARGO_PKG_VERSION")
    );
    let main = r#"use akku::html::Html;
use akku::http::{App, Result};

#[akku::http::get("/")]
async fn home() -> Html {
    Html::new("<h1>Hello from Akku</h1>")
}

#[akku::http::main]
async fn main() -> Result<()> {
    App::new().mount(home).serve("127.0.0.1:3000").await
}
"#;
    fs::write(root.join("Cargo.toml"), manifest)
        .map_err(|error| format!("cannot write Cargo.toml: {error}"))?;
    fs::write(root.join("src/main.rs"), main)
        .map_err(|error| format!("cannot write src/main.rs: {error}"))?;
    println!("created `{name}`");
    println!("  cd {name}");
    println!("  cargo run");
    Ok(())
}

fn valid_name(name: &str) -> bool {
    let mut characters = name.chars();
    let Some(first) = characters.next() else {
        return false;
    };
    first.is_ascii_alphabetic()
        && characters.all(|character| {
            character.is_ascii_alphanumeric() || character == '_' || character == '-'
        })
}

fn print_help() {
    println!("akku {}", env!("CARGO_PKG_VERSION"));
    println!();
    println!("usage:");
    println!("  akku new <name>   create a project");
    println!("  akku --version    print the version");
    println!("  akku --help       show this help");
}

#[cfg(test)]
mod tests {
    use super::valid_name;

    #[test]
    fn accepts_package_names() {
        assert!(valid_name("hello"));
        assert!(valid_name("hello_world"));
        assert!(valid_name("hello-world"));
    }

    #[test]
    fn rejects_empty_and_path_like_names() {
        assert!(!valid_name(""));
        assert!(!valid_name("../hello"));
        assert!(!valid_name("1hello"));
        assert!(!valid_name("hello/world"));
    }
}
