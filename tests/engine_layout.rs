//! Data-directory layout of the embedded engine.
//!
//! Pins the locations that outlive a run: a later invocation finds an instance
//! it did not start through the port and pid files, so moving them orphans it.

use std::path::{Path, PathBuf};

use hodis::engine::Paths;

fn paths() -> Paths {
    Paths::new(PathBuf::from("/data/hodis"))
}

#[test]
fn everything_lives_under_the_data_directory() {
    let paths = paths();
    let root = paths.root();

    for path in [
        paths.uv(),
        paths.uv_cache(),
        paths.engine(),
        paths.python_install_dir(),
        paths.venv(),
        paths.venv_python(),
        paths.settings(),
        paths.pidfile(),
        paths.portfile(),
        paths.revision(),
        paths.log(),
    ] {
        assert!(
            path.starts_with(root),
            "{} escapes the data directory {}",
            path.display(),
            root.display()
        );
    }
}

#[test]
fn runtime_state_sits_beside_the_installation() {
    // A pid file that drifts away from the venv it describes would let a stale
    // record survive a wiped installation.
    let paths = paths();
    let engine = paths.engine();

    assert_eq!(paths.settings(), engine.join("settings.yml"));
    assert_eq!(paths.pidfile(), engine.join("searxng.pid"));
    assert_eq!(paths.portfile(), engine.join("port"));
    assert_eq!(paths.revision(), engine.join("revision"));
    assert_eq!(paths.log(), engine.join("searxng.log"));
}

#[test]
fn the_interpreter_is_inside_the_virtualenv() {
    let paths = paths();
    assert!(
        paths.venv_python().starts_with(paths.venv()),
        "the interpreter hodis runs must be the one it provisioned"
    );
    assert!(
        paths
            .venv_python()
            .file_stem()
            .is_some_and(|name| name == "python"),
        "expected a python executable, got {}",
        paths.venv_python().display()
    );
}

#[test]
fn distinct_roots_stay_isolated() {
    // The engine must be relocatable, since tests and packaged installs point
    // it somewhere other than the platform data directory.
    let a = Paths::new(PathBuf::from("/one"));
    let b = Paths::new(PathBuf::from("/two"));

    assert_ne!(a.settings(), b.settings());
    assert!(a.settings().starts_with(Path::new("/one")));
    assert!(b.settings().starts_with(Path::new("/two")));
}

#[test]
fn the_default_location_is_a_hodis_directory() {
    if std::env::var_os(hodis::engine::DATA_DIR_ENV).is_some() {
        // An override is in effect, so resolve() is not reporting the default.
        return;
    }

    let paths = Paths::resolve().expect("the test user has a data directory");
    assert!(
        paths.root().ends_with("hodis"),
        "expected a hodis-owned directory, got {}",
        paths.root().display()
    );
}
