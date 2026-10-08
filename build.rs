use std::env;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn compile_with(tool: &OsStr, args: &[OsString], directory: &Path) -> std::io::Result<Output> {
    Command::new(tool)
        .args(args)
        .current_dir(directory)
        .output()
}

fn run_resource_compiler(
    primary: OsString,
    fallback: Option<OsString>,
    args: &[OsString],
    directory: &Path,
) {
    let (tool, output) = match compile_with(&primary, args, directory) {
        Ok(output) => (primary, output),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let fallback = fallback.unwrap_or_else(|| {
                panic!(
                    "resource compiler {} was not found; set WINDRES or RC to its executable path",
                    primary.to_string_lossy()
                )
            });
            let output = compile_with(&fallback, args, directory).unwrap_or_else(|error| {
                panic!(
                    "resource compiler {} could not start: {error}",
                    fallback.to_string_lossy()
                )
            });
            (fallback, output)
        }
        Err(error) => panic!(
            "resource compiler {} could not start: {error}",
            primary.to_string_lossy()
        ),
    };
    if !output.status.success() {
        panic!(
            "resource compiler {} failed ({}):\n{}{}",
            tool.to_string_lossy(),
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

fn main() {
    println!("cargo:rerun-if-changed=resources/app.rc");
    println!("cargo:rerun-if-changed=resources/app.manifest");
    println!("cargo:rerun-if-changed=resources/app.ico");
    println!("cargo:rerun-if-env-changed=WINDRES");
    println!("cargo:rerun-if-env-changed=RC");

    let directory =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("Cargo manifest directory missing"));
    for name in [
        "resources/app.rc",
        "resources/app.manifest",
        "resources/app.ico",
    ] {
        assert!(
            directory.join(name).is_file(),
            "required Windows resource is missing: {name}"
        );
    }
    assert_eq!(
        env::var("CARGO_CFG_TARGET_OS").as_deref(),
        Ok("windows"),
        "Dot Calendar only supports Windows targets"
    );

    let resources = directory.join("resources");
    let output = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo output directory missing"));
    let artifact = match env::var("CARGO_CFG_TARGET_ENV").as_deref() {
        Ok("gnu") => {
            let arch = match env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
                Ok("x86") => "i686",
                Ok("x86_64") => "x86_64",
                Ok("aarch64") => "aarch64",
                _ => panic!("unsupported GNU Windows target architecture"),
            };
            let artifact = output.join("app-resources.o");
            let args = vec![
                OsString::from("--input=app.rc"),
                OsString::from(format!("--output={}", artifact.display())),
                OsString::from("--output-format=coff"),
            ];
            let primary = env::var_os("WINDRES")
                .unwrap_or_else(|| format!("{arch}-w64-mingw32-windres").into());
            run_resource_compiler(primary, None, &args, &resources);
            artifact
        }
        Ok("msvc") => {
            let artifact = output.join("app-resources.res");
            let args = vec![
                OsString::from("/nologo"),
                OsString::from(format!("/fo{}", artifact.display())),
                OsString::from("app.rc"),
            ];
            let configured = env::var_os("RC");
            let primary = configured.clone().unwrap_or_else(|| "rc.exe".into());
            let fallback = if configured.is_none() {
                Some("llvm-rc.exe".into())
            } else {
                None
            };
            run_resource_compiler(primary, fallback, &args, &resources);
            // Link the resource we supplied rather than a second linker-generated manifest.
            println!("cargo:rustc-link-arg=/MANIFEST:NO");
            artifact
        }
        _ => panic!("Dot Calendar requires a GNU or MSVC Windows target"),
    };
    assert!(
        artifact.is_file(),
        "resource compiler did not produce {}",
        artifact.display()
    );
    // The same embedded manifest is needed in the GUI executable and test binary.
    println!("cargo:rustc-link-arg={}", artifact.display());
}
