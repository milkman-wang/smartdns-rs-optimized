#![allow(dead_code)]

use std::fs::File;
use std::io::Write;

use std::{env, path::Path};

fn download<P: AsRef<Path> + Copy>(url: &str, file_path: P) -> bool {
    use reqwest::blocking as http;
    if Path::exists(file_path.as_ref()) {
        return false;
    }

    let rest = http::get(url).unwrap_or_else(|_| panic!("URL: {url} download failed!!!"));

    let bytes = rest.bytes().expect("read bytes from server failed!!!");

    write_all_to_file(file_path, bytes);

    true
}

fn write_all_to_file<P: AsRef<Path> + Copy, T: AsRef<[u8]>>(file_path: P, text: T) {
    let mut file = File::create(file_path)
        .unwrap_or_else(|_| panic!("Create file {:?} failed", file_path.as_ref()));
    file.write_all(text.as_ref()).unwrap();
}

fn append_text_to_file<P: AsRef<Path> + Copy, T: AsRef<[u8]>>(file_path: P, text: T) {
    let mut file = File::options()
        .append(true)
        .create(true)
        .open(file_path)
        .unwrap_or_else(|_| panic!("Create file {:?} failed", file_path.as_ref()));
    file.write_all(text.as_ref()).unwrap();
}

fn download_resources() -> anyhow::Result<()> {
    if download(
        "https://cdn.jsdelivr.net/gh/pymumu/smartdns/etc/smartdns/smartdns.conf",
        "etc/smartdns/smartdns.conf",
    ) {
        // append_text_to_file("./etc/smartdns/smartdns.conf", "\nconf-file custom.conf\n");
    }

    download(
        "https://cdn.jsdelivr.net/gh/pymumu/smartdns/package/openwrt/files/etc/init.d/smartdns",
        "src/service/linux/initd/openwrt/files/etc/init.d/smartdns-rs",
    );

    download(
        "https://cdn.jsdelivr.net/gh/mullvad/windows-service-rs/src/shell_escape.rs",
        "src/service/windows/shell_escape.rs",
    );
    Ok(())
}

fn create_build_time_vars() -> anyhow::Result<()> {
    let target_dir = env::var_os("OUT_DIR").unwrap();
    let target_dir = Path::new(&target_dir);
    let build_file = target_dir.join("build_time_vars.rs");
    let mut file = File::create(build_file)?;
    let build_timestamp = chrono::Utc::now().timestamp_millis();
    writeln!(
        file,
        r#"pub const BUILD_DATE: chrono::DateTime<chrono::Utc> = chrono::DateTime::from_timestamp_millis({build_timestamp}).unwrap();"#
    )?;

    writeln!(
        file,
        r#"pub const BUILD_TARGET: &str = "{}";"#,
        env::var("TARGET").unwrap()
    )?;

    writeln!(
        file,
        r#"pub const BUILD_VERSION: &str = "{}";"#,
        env::var("CARGO_PKG_VERSION").unwrap()
    )?;
    Ok(())
}

fn main() -> anyhow::Result<()> {
    std::fs::create_dir_all("./logs")?;

    download_resources()?;

    create_build_time_vars()?;
    Ok(())
}
