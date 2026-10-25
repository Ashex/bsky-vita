use std::{env, path::PathBuf, process::Command};

const QUICKJS_REV: &str = "ba5bdd0dc013518768e76cd9e05cd30ed53dd35b";

fn git(path: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .output()
        .expect("git is required to verify the pinned QuickJS source")
}

fn main() {
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let vendor = manifest.join("../../vendor/pocketjs");
    let cargo_home = env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".cargo")))
        .expect("CARGO_HOME or HOME must be set to locate pinned QuickJS source");
    let cache = cargo_home.join("git/db/quickjs-rs-eb07d6c367fd641c");
    let pinned = Command::new("git")
        .arg("--git-dir")
        .arg(&cache)
        .args(["rev-parse", &format!("{QUICKJS_REV}^{{commit}}")])
        .output()
        .expect("git is required to verify the pinned QuickJS source");
    assert!(
        pinned.status.success(),
        "pinned QuickJS commit {QUICKJS_REV} absent; run `cargo fetch` first"
    );
    let checkout_root = cargo_home.join("git/checkouts/quickjs-rs-eb07d6c367fd641c");
    let checkout = std::fs::read_dir(&checkout_root)
        .expect("QuickJS checkout absent; run `cargo fetch`")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| {
            let head = git(path, &["rev-parse", "HEAD"]);
            let tree = git(path, &["rev-parse", "HEAD^{tree}"]);
            let expected = Command::new("git")
                .arg("--git-dir")
                .arg(&cache)
                .args(["rev-parse", &format!("{QUICKJS_REV}^{{tree}}")])
                .output()
                .expect("git is required to verify pinned QuickJS tree contents");
            head.status.success()
                && tree.status.success()
                && expected.status.success()
                && String::from_utf8_lossy(&head.stdout).trim() == QUICKJS_REV
                && tree.stdout == expected.stdout
        })
        .expect("clean checkout for pinned QuickJS revision missing; run `cargo fetch`");
    let quickjs = checkout.join("libquickjs-sys/embed/quickjs");
    assert!(
        quickjs.join("quickjs.h").is_file(),
        "pinned QuickJS source is incomplete"
    );
    println!("cargo:rerun-if-changed=bridge.c");
    for name in [
        "quickjs.c",
        "dtoa.c",
        "libregexp.c",
        "libunicode.c",
        "cutils.c",
        "quickjs.h",
    ] {
        println!("cargo:rerun-if-changed={}", quickjs.join(name).display());
    }
    for path in [
        vendor.join("engine/guest-quickjs/src/guest.c"),
        vendor.join("engine/guest-quickjs/src/generated_ops.h"),
        vendor.join("engine/guest-quickjs/include/pocket_guest_quickjs.h"),
        vendor.join("engine/runtime-cabi/src/runtime.rs"),
        vendor.join("engine/runtime-cabi/src/adapters.rs"),
        vendor.join("contracts/generated/pocket_runtime.h"),
        vendor.join("contracts/generated/pocket_drivers.h"),
        vendor.join("contracts/generated/pocket_spec.h"),
    ] {
        println!("cargo:rerun-if-changed={}", path.display());
    }
    let mut build = cc::Build::new();
    build
        .file("bridge.c")
        .include(vendor.join("contracts/generated"))
        .file(vendor.join("engine/guest-quickjs/src/guest.c"))
        .include(vendor.join("engine/guest-quickjs/include"))
        .include(vendor.join("contracts/generated"))
        .include(&quickjs)
        .define("POCKET_QJS_BELLARD", None)
        .define("JS_NO_NAN_BOXING", None)
        .define("CONFIG_VERSION", "\"2026-06-04\"")
        .define("_GNU_SOURCE", None)
        .flag_if_supported("-std=gnu11");
    for name in ["quickjs", "dtoa", "libregexp", "libunicode", "cutils"] {
        build.file(quickjs.join(format!("{name}.c")));
    }
    build.compile("pocketjs_quickjs");
}
