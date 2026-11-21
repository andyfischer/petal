//! `embed_package!` and `Env::register_embedded_package`: a package directory
//! compiled into the binary and registered from it.

use petal::env::Env;
use petal::package::EmbeddedPackage;

static DEMO: EmbeddedPackage = petal::embed_package!("../fixtures/embedded-demo");

#[test]
fn embeds_the_manifest_and_modules_but_not_tests() {
    assert_eq!(DEMO.name, "embedded_demo");
    let paths: Vec<&str> = DEMO.files.iter().map(|(p, _)| *p).collect();
    assert_eq!(paths, ["embedded_demo.ptl", "helpers.ptl", "petal.toml"]);
    assert!(DEMO.manifest().contains("version = \"0.3.0\""));
}

#[test]
fn a_registered_package_is_importable() {
    let mut env = Env::new();
    let info = env.register_embedded_package(&DEMO).unwrap();
    assert_eq!(info.name, "embedded_demo");
    let pid = env.load_program("import embedded_demo\ngreeting()\n").unwrap();
    let stack = env.create_stack(pid).unwrap();
    let v = env.run(stack).unwrap();
    assert_eq!(petal::value::value_to_json(&v, env.heap()).unwrap(), serde_json::json!("hello abab"));
}

#[test]
fn a_provided_package_is_announced_at_its_version() {
    let mut env = Env::new();
    env.provide_embedded_package(&DEMO).unwrap();
    assert_eq!(env.provided_version("embedded_demo").as_deref(), Some("0.3.0"));
}
