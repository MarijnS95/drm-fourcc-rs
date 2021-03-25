#![feature(with_options)]

#[cfg(not(feature = "build_bindings"))]
fn main() {
    println!("cargo:rerun-if-changed=build.rs"); // never rerun
}

#[cfg(feature = "build_bindings")]
fn main() {
    println!("cargo:rerun-if-changed=build.rs"); // avoids double-build when we output into src
    generate::generate().unwrap();
}

#[cfg(feature = "build_bindings")]
mod generate {
    use std::error::Error;
    use std::io::Write;
    use std::process::{Command, Stdio};

    use regex::Regex;
    use std::env;
    use std::fs::File;
    use std::path::Path;

    pub fn generate() -> Result<(), Box<dyn Error + Sync + Send>> {
        let out_dir = env::var("OUT_DIR").unwrap();
        let wrapper_path = Path::new(&out_dir).join("wrapper.h");

        // First get all the macros in drm_fourcc.h

        let mut cmd = Command::new("clang")
            .arg("-E") // run pre-processor only
            .arg("-dM") // output all macros defined
            .arg("-") // take input from stdin
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()?;

        {
            let stdin = cmd.stdin.as_mut().expect("failed to open stdin");
            stdin.write_all(b"#include <drm/drm_fourcc.h>\n")?;
        }

        let result = cmd.wait_with_output()?;
        let stdout = String::from_utf8(result.stdout)?;
        if !result.status.success() {
            panic!("Clang failed with output: {}", stdout)
        }

        // Then get the names of the format macros

        let re = Regex::new(r"^\s*#define (?P<full>DRM_FORMAT_(?P<short>[A-Z0-9]+)) ")?;
        let names: Vec<(&str, &str)> = stdout
            .lines()
            .filter_map(|line| {
                if line.contains("DRM_FORMAT_RESERVED") || line.contains("INVALID") {
                    return None;
                }

                re.captures(line).map(|caps| {
                    let full = caps.name("full").unwrap().as_str();
                    let short = caps.name("short").unwrap().as_str();

                    (full, short)
                })
            })
            .collect();

        // Then create a file with a variable defined for every format macro

        let mut wrapper = File::create(&wrapper_path)?;

        wrapper.write_all(b"#include <stdint.h>\n")?;
        wrapper.write_all(b"#include <drm/drm_fourcc.h>\n")?;

        let const_prefix = "DRM_FOURCC_";

        for (full, short) in &names {
            writeln!(wrapper, "uint32_t {}{} = {};\n", const_prefix, short, full)?;
        }

        wrapper.flush()?;

        // Then generate bindings from that file
        bindgen::builder()
            .header(wrapper_path.as_os_str().to_str().unwrap())
            .whitelist_var("DRM_FOURCC_.*")
            .generate()
            .unwrap()
            .write_to_file("src/consts.rs")?;

        // Then generate an enum
        let as_enum_path = "src/as_enum.rs";
        {
            let mut as_enum = File::create(as_enum_path)?;

            as_enum.write_all(b"// Automatically generated by build.rs\n")?;
            as_enum.write_all(b"use crate::consts;")?;
            as_enum.write_all(b"#[derive(Copy, Clone, Eq, PartialEq)]")?;
            as_enum.write_all(
                b"#[cfg_attr(feature = \"serde\", derive(serde::Serialize, serde::Deserialize))]",
            )?;
            as_enum.write_all(b"#[repr(u32)]")?;
            as_enum.write_all(b"pub enum DrmFormat {\n")?;

            let members: Vec<(String, String)> = names
                .iter()
                .map(|(_, short)| {
                    (
                        enum_member_case(short),
                        format!("consts::{}{}", const_prefix, short),
                    )
                })
                .collect();

            for (member, value) in &members {
                writeln!(as_enum, "{} = {},", member, value)?;
            }

            as_enum.write_all(b"}\n")?;

            as_enum.write_all(b"impl DrmFormat {\n")?;
            as_enum.write_all(b"pub(crate) fn from_u32(n: u32) -> Option<Self> {\n")?;
            as_enum.write_all(b"match n {\n")?;

            for (member, value) in &members {
                writeln!(as_enum, "{} => Some(Self::{}),", value, member)?;
            }

            writeln!(as_enum, "_ => None")?;
            as_enum.write_all(b"}}}")?;
        }

        Command::new("rustfmt").arg(as_enum_path).spawn()?.wait()?;

        Ok(())
    }

    fn enum_member_case(s: &str) -> String {
        let (first, rest) = s.split_at(1);
        format!("{}{}", first, rest.to_ascii_lowercase())
    }
}
