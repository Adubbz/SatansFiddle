use std::{env, process::Command};

fn main() {
    // The `lldb` crate is built with `no_link_args`, so it emits no linker
    // arguments of its own: it is up to us to supply liblldb and the C++
    // runtime that its cpp! glue code was compiled against.
    let libdir = env::var("LLDB_LIB_DIR").ok().unwrap_or_else(|| {
        Command::new("llvm-config")
            .arg("--libdir")
            .output()
            .ok()
            .filter(|out| out.status.success())
            .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_owned())
            .unwrap_or_else(|| "/usr/lib".to_owned())
    });

    println!("cargo:rerun-if-env-changed=LLDB_LIB_DIR");
    println!("cargo:rustc-link-search=native={libdir}");
    println!("cargo:rustc-link-lib=dylib=lldb");
    println!("cargo:rustc-link-lib=dylib=stdc++");
    // liblldb lives outside the default loader path on some installs.
    println!("cargo:rustc-link-arg=-Wl,-rpath,{libdir}");
}
