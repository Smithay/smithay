fn main() {
    #[cfg(feature = "generate-hwdata")]
    generate_hwdata();
}

#[cfg(feature = "generate-hwdata")]
fn generate_hwdata() {
    use std::{
        fs,
        path::{Path, PathBuf},
    };

    let pkg_path = pkg_config::get_variable("hwdata", "pkgdatadir");
    // Old versions of hwdata don't have .pc file, so let's guess
    let pkg_path = pkg_path.as_deref().unwrap_or("/usr/share/hwdata");

    let pnp_ids_path = PathBuf::from(pkg_path).join("pnp.ids");

    println!(
        "cargo:rerun-if-changed={}",
        pnp_ids_path.as_os_str().to_string_lossy()
    );

    if let Ok(file) = fs::read_to_string(pnp_ids_path) {
        let out_dir = PathBuf::from("src").join("hwdata").join("generated");
        let dest_path = Path::new(&out_dir).join("pnp_ids.rs");

        let i1 = " ".repeat(4);
        let i2 = " ".repeat(8);

        let mut output = Vec::new();

        output.push("#[rustfmt::skip]".into());
        output.push("pub fn pnp_id_to_name(vendor: &[char; 3]) -> Option<&'static str> {".into());
        output.push(i1.clone() + "match vendor {");

        for line in file.lines() {
            let mut segment = line.split('\t');

            let mut code = segment.next().unwrap().chars();
            let n1 = code.next().unwrap();
            let n2 = code.next().unwrap();
            let n3 = code.next().unwrap();

            let name = segment.next().unwrap();

            output.push(format!("{i2}['{n1}', '{n2}', '{n3}'] => Some(\"{name}\"),"));
        }

        output.push(i2 + "_ => None,");

        output.push(i1 + "}");
        output.push("}".into());

        let output = output.join("\n");

        fs::write(dest_path, output).unwrap();
    }
}
