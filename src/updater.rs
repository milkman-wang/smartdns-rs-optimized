use self_update::update::{Release, ReleaseAsset};

fn select_release<'a>(
    releases: &'a [Release],
    flavor: &str,
    target: &str,
    extension: &str,
    current: &str,
    requested: Option<&str>,
) -> Option<(&'a Release, &'a ReleaseAsset, &'a str)> {
    let mut selected: Option<(&Release, &ReleaseAsset, &str)> = None;
    for release in releases {
        let version = if flavor == "webui" {
            let Some(version) = release.version.strip_prefix("webui-v") else {
                continue;
            };
            version
        } else {
            release.version.trim_start_matches('v')
        };
        if let Some(requested) = requested {
            if version != requested {
                continue;
            }
        } else if !self_update::version::bump_is_greater(current, version).unwrap_or(false)
            || (!current.contains('-') && version.contains('-'))
        {
            continue;
        }
        let prefix = if flavor == "webui" {
            "smartdns-webui"
        } else {
            "smartdns"
        };
        let expected = format!("{prefix}-{target}-v{version}.{extension}");
        let Some(asset) = release.assets.iter().find(|asset| asset.name == expected) else {
            continue;
        };
        if selected.is_none_or(|(_, _, previous)| {
            self_update::version::bump_is_greater(previous, version).unwrap_or(false)
        }) {
            selected = Some((release, asset, version));
        }
    }
    selected
}

pub fn update(assume_yes: bool, requested: Option<&str>) -> anyhow::Result<()> {
    use std::io::{self, Write};
    let repository = option_env!("SMARTDNS_RELEASE_REPOSITORY")
        .unwrap_or(env!("CARGO_PKG_REPOSITORY").trim_start_matches("https://github.com/"))
        .trim_end_matches('/')
        .trim_end_matches(".git");
    let (owner, name) = repository
        .split_once('/')
        .ok_or_else(|| anyhow::anyhow!("invalid release repository"))?;
    let flavor = crate::BUILD_FLAVOR;
    let extension = if cfg!(any(windows, target_os = "macos")) {
        "zip"
    } else {
        "tar.gz"
    };
    let requested = requested.map(|value| {
        value
            .strip_prefix("webui-v")
            .unwrap_or(value.trim_start_matches('v'))
    });
    let releases = self_update::backends::github::ReleaseList::configure()
        .repo_owner(owner)
        .repo_name(name)
        .build()?
        .fetch()?;
    let Some((_release, asset, version)) = select_release(
        &releases,
        flavor,
        crate::BUILD_TARGET,
        extension,
        crate::BUILD_VERSION,
        requested,
    ) else {
        if requested.is_some() {
            anyhow::bail!("no matching {flavor} release for the requested version and target");
        }
        println!("No newer {flavor} release is available.");
        return Ok(());
    };
    let tag = if flavor == "webui" {
        format!("webui-v{version}")
    } else {
        format!("v{version}")
    };
    println!(
        "Update {} ({flavor}) to {version}: {}",
        crate::BUILD_VERSION,
        asset.name
    );
    if !assume_yes {
        print!("Do you want to continue? [Y/n] ");
        io::stdout().flush()?;
        let mut answer = String::new();
        io::stdin().read_line(&mut answer)?;
        if matches!(answer.trim().to_ascii_lowercase().as_str(), "n" | "no") {
            return Ok(());
        }
    }
    let temporary = self_update::TempDir::new()?;
    let archive = temporary.path().join(&asset.name);
    let url = format!(
        "https://github.com/{repository}/releases/download/{tag}/{}",
        asset.name
    );
    self_update::Download::from_url(&url)
        .show_progress(true)
        .download_to(std::fs::File::create(&archive)?)?;
    let directory = if flavor == "webui" {
        format!("smartdns-webui-{}", crate::BUILD_TARGET)
    } else {
        format!("smartdns-{}", crate::BUILD_TARGET)
    };
    let executable = format!("{directory}/smartdns{}", std::env::consts::EXE_SUFFIX);
    self_update::Extract::from_source(&archive).extract_file(temporary.path(), &executable)?;
    self_update::self_replace::self_replace(temporary.path().join(executable))?;
    println!("Updated to {version} ({flavor}).");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_updates_stay_on_their_variant_and_choose_archives() {
        let release = |version: &str, asset: &str| Release {
            version: version.into(),
            assets: vec![ReleaseAsset {
                name: asset.into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let releases = vec![
            release(
                "webui-v0.13.4",
                "smartdns-webui-test-v0.13.4.zip-sha256sum.txt",
            ),
            release("webui-v0.13.3", "smartdns-webui-test-v0.13.3.zip"),
            release("0.13.2", "smartdns-test-v0.13.2.zip"),
        ];
        assert_eq!(
            select_release(&releases, "headless", "test", "zip", "0.13.1", None)
                .unwrap()
                .2,
            "0.13.2"
        );
        assert_eq!(
            select_release(&releases, "webui", "test", "zip", "0.13.1", None)
                .unwrap()
                .2,
            "0.13.3"
        );
        assert!(
            select_release(
                &releases,
                "headless",
                "test",
                "zip",
                "0.13.1",
                Some("0.13.3")
            )
            .is_none()
        );
        assert!(select_release(&releases, "webui", "test", "tar.gz", "0.13.1", None).is_none());
    }
}
