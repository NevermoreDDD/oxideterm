// Copyright (C) 2026 AnalyseDeCircuit
// SPDX-License-Identifier: GPL-3.0-only

use std::{collections::HashSet, error::Error, fs, path::PathBuf, time::Duration};

use oxideterm_plugin_registry::{
    NATIVE_PLUGIN_PACKAGE_MAX_BYTES, NativePluginRegistry, validate_native_plugin_id,
};
use serde_json::{Value, json};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 3 {
        return Err(
            "usage: package_plugins <windows-target> <selection.json> <empty-output-dir>".into(),
        );
    }
    let target = &args[0];
    if !matches!(
        target.as_str(),
        "x86_64-pc-windows-msvc" | "aarch64-pc-windows-msvc"
    ) {
        return Err("a Windows package target is required".into());
    }
    let selection: Value = serde_json::from_slice(&fs::read(&args[1])?)?;
    let plugins = selection
        .get("plugins")
        .and_then(Value::as_array)
        .ok_or("selection must contain a plugins array")?;
    if plugins.is_empty() {
        return Err("plugin selection is empty".into());
    }
    let output = PathBuf::from(&args[2]);
    if output.exists() && fs::read_dir(&output)?.next().is_some() {
        return Err("plugin output directory must be empty".into());
    }
    let settings_path = output.join("data/settings.json");
    let packages_dir = output.join("resources/plugin-packages");
    fs::create_dir_all(&packages_dir)?;
    let catalog = NativePluginRegistry::fetch_official_plugin_registry().await?;
    NativePluginRegistry::cache_official_catalog(&settings_path, &catalog)?;
    let client = oxideterm_network_proxy::application_http_client()?;
    let mut seen = HashSet::new();
    let mut bundled = Vec::new();
    for plugin in plugins {
        let id = plugin
            .get("id")
            .and_then(Value::as_str)
            .ok_or("plugin ID missing")?;
        validate_native_plugin_id(id)?;
        if !seen.insert(id) {
            return Err(format!("duplicate plugin ID: {id}").into());
        }
        let minimum = semver::Version::parse(
            plugin
                .get("minimumVersion")
                .and_then(Value::as_str)
                .ok_or("minimumVersion missing")?,
        )?;
        let summary = catalog
            .plugins
            .iter()
            .find(|entry| entry.id == id)
            .ok_or_else(|| format!("Selected plugin is absent from the official catalog: {id}"))?;
        let history = NativePluginRegistry::fetch_registry_history(&settings_path, summary).await?;
        let (release, package) =
            NativePluginRegistry::resolve_registry_release_for_target(&history, target)?;
        if semver::Version::parse(&release.version)?
            .cmp_precedence(&minimum)
            .is_lt()
        {
            return Err(
                format!("Compatible {target} release of {id} is older than {minimum}").into(),
            );
        }
        let mut response = client
            .get(&package.download_url)
            .timeout(Duration::from_secs(90))
            .send()
            .await
            .map_err(|_| format!("Cannot download plugin package: {id}"))?;
        if !response.status().is_success() {
            return Err(format!("Plugin {id} returned HTTP {}", response.status()).into());
        }
        if response
            .content_length()
            .is_some_and(|size| size > NATIVE_PLUGIN_PACKAGE_MAX_BYTES)
        {
            return Err(format!("Plugin {id} exceeds the package size limit").into());
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| format!("Cannot read plugin package: {id}"))?
        {
            if bytes.len() as u64 + chunk.len() as u64 > NATIVE_PLUGIN_PACKAGE_MAX_BYTES {
                return Err(format!("Plugin {id} exceeds the package size limit").into());
            }
            bytes.extend_from_slice(&chunk);
        }
        if package.size.is_some_and(|size| size != bytes.len() as u64) {
            return Err(format!("Plugin {id} size differs from the catalog").into());
        }
        let installed = NativePluginRegistry::install_managed_plugin_package(
            &settings_path,
            id,
            Some(&package.checksum),
            &bytes,
            false,
        )?;
        if installed.manifest.version != release.version {
            return Err(format!("Plugin {id} version differs from the catalog").into());
        }
        let filename = format!("{id}-{}-{}.zip", release.version, package.target);
        fs::write(packages_dir.join(&filename), &bytes)?;
        bundled.push(json!({
            "id": id,
            "name": installed.manifest.name,
            "version": release.version,
            "target": package.target,
            "engines": release.engines,
            "file": filename,
            "checksum": format!("sha256:{}", installed.checksum),
            "size": bytes.len(),
            "downloadUrl": package.download_url,
        }));
        println!("Bundled {id} {} ({})", release.version, package.target);
    }
    // Only public catalog metadata and verified packages enter this fresh profile.
    // Config defaults keep permission review in the host instead of exporting approvals.
    let index = json!({
        "formatVersion": 1,
        "hostVersion": env!("CARGO_PKG_VERSION"),
        "target": target,
        "plugins": bundled,
    });
    fs::write(
        packages_dir.join("index.json"),
        serde_json::to_vec_pretty(&index)?,
    )?;
    Ok(())
}
