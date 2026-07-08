use std::fs;
use std::io::Cursor;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};
use flate2::read::GzDecoder;
use serde_json::{Map, Value};

#[derive(Debug)]
struct PackageSpec {
    name: String,
    requested: Option<String>,
}

pub(crate) fn install_packages(packages: &[String]) -> Result<()> {
    if packages.is_empty() {
        bail!("install requires at least one package");
    }
    let root = std::env::current_dir().context("failed to read current directory")?;
    for package in packages {
        install_package(&root, &parse_package_spec(package)?)?;
    }
    Ok(())
}

fn install_package(root: &Path, spec: &PackageSpec) -> Result<()> {
    let metadata = fetch_package_metadata(&spec.name)?;
    let version = select_version(&metadata, spec.requested.as_deref())?;
    let tarball = metadata
        .get("versions")
        .and_then(Value::as_object)
        .and_then(|versions| versions.get(&version))
        .and_then(|entry| entry.get("dist"))
        .and_then(|dist| dist.get("tarball"))
        .and_then(Value::as_str)
        .with_context(|| format!("package `{}` version `{version}` has no tarball", spec.name))?;
    let bytes = reqwest::blocking::get(tarball)
        .with_context(|| format!("failed to fetch tarball for `{}`", spec.name))?
        .error_for_status()
        .with_context(|| format!("registry returned an error for `{}`", spec.name))?
        .bytes()
        .with_context(|| format!("failed to read tarball for `{}`", spec.name))?;
    let package_dir = package_install_dir(root, &spec.name);
    if package_dir.exists() {
        fs::remove_dir_all(&package_dir)
            .with_context(|| format!("failed to replace '{}'", package_dir.display()))?;
    }
    fs::create_dir_all(package_dir.parent().unwrap_or(root))
        .with_context(|| format!("failed to create parent for '{}'", package_dir.display()))?;
    extract_package(bytes.as_ref(), &package_dir)?;
    update_package_json(root, &spec.name, &format!("^{version}"))?;
    println!("installed {}@{}", spec.name, version);
    Ok(())
}

fn parse_package_spec(raw: &str) -> Result<PackageSpec> {
    if raw.trim().is_empty() {
        bail!("package spec must not be empty");
    }
    let version_split = raw
        .rfind('@')
        .filter(|idx| *idx > 0)
        .filter(|idx| raw[..*idx].contains('/') || !raw.starts_with('@'));
    let (name, requested) = match version_split {
        Some(idx) => (&raw[..idx], Some(raw[idx + 1..].to_string())),
        None => (raw, None),
    };
    if name.is_empty() || requested.as_deref() == Some("") {
        bail!("invalid package spec `{raw}`");
    }
    Ok(PackageSpec {
        name: name.to_string(),
        requested,
    })
}

fn fetch_package_metadata(name: &str) -> Result<Value> {
    let url = format!("https://registry.npmjs.org/{}", registry_name(name));
    let body = reqwest::blocking::get(&url)
        .with_context(|| format!("failed to fetch npm metadata for `{name}`"))?
        .error_for_status()
        .with_context(|| format!("npm registry returned an error for `{name}`"))?
        .text()
        .with_context(|| format!("failed to read npm metadata for `{name}`"))?;
    serde_json::from_str::<Value>(&body)
        .with_context(|| format!("failed to parse npm metadata for `{name}`"))
}

fn select_version(metadata: &Value, requested: Option<&str>) -> Result<String> {
    if let Some(requested) = requested
        && metadata
            .get("versions")
            .and_then(Value::as_object)
            .is_some_and(|versions| versions.contains_key(requested))
    {
        return Ok(requested.to_string());
    }
    if let Some(tag) = requested.or(Some("latest"))
        && let Some(version) = metadata
            .get("dist-tags")
            .and_then(Value::as_object)
            .and_then(|tags| tags.get(tag))
            .and_then(Value::as_str)
    {
        return Ok(version.to_string());
    }
    bail!("unsupported package version selector `{}`", requested.unwrap_or("latest"));
}

fn registry_name(name: &str) -> String {
    if name.starts_with('@') {
        name.replace('/', "%2f")
    } else {
        name.to_string()
    }
}

fn package_install_dir(root: &Path, name: &str) -> PathBuf {
    let mut dir = root.join("node_modules");
    for part in name.split('/') {
        dir.push(part);
    }
    dir
}

fn extract_package(bytes: &[u8], destination: &Path) -> Result<()> {
    fs::create_dir_all(destination)
        .with_context(|| format!("failed to create '{}'", destination.display()))?;
    let gz = GzDecoder::new(Cursor::new(bytes));
    let mut archive = tar::Archive::new(gz);
    for entry in archive.entries().context("failed to read package tarball")? {
        let mut entry = entry.context("failed to read package tarball entry")?;
        let raw_path = entry.path().context("failed to read tarball entry path")?;
        let safe_path = strip_package_prefix(&raw_path)?;
        if safe_path.as_os_str().is_empty() {
            continue;
        }
        let out_path = destination.join(&safe_path);
        if let Some(parent) = out_path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create '{}'", parent.display()))?;
        }
        entry
            .unpack(&out_path)
            .with_context(|| format!("failed to unpack '{}'", out_path.display()))?;
    }
    Ok(())
}

fn strip_package_prefix(path: &Path) -> Result<PathBuf> {
    let mut components = path.components();
    let Some(first) = components.next() else {
        return Ok(PathBuf::new());
    };
    if !matches!(first, Component::Normal(_)) {
        bail!("invalid tarball path '{}'", path.display());
    }
    let mut out = PathBuf::new();
    for component in components {
        match component {
            Component::Normal(part) => out.push(part),
            Component::CurDir => {}
            _ => bail!("invalid tarball path '{}'", path.display()),
        }
    }
    Ok(out)
}

fn update_package_json(root: &Path, package: &str, requirement: &str) -> Result<()> {
    let path = root.join("package.json");
    let mut value = if path.exists() {
        serde_json::from_str::<Value>(
            &fs::read_to_string(&path)
                .with_context(|| format!("failed to read '{}'", path.display()))?,
        )
        .with_context(|| format!("failed to parse '{}'", path.display()))?
    } else {
        Value::Object(Map::new())
    };
    let object = value
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("'{}' must contain a JSON object", path.display()))?;
    let dependencies = object
        .entry("dependencies")
        .or_insert_with(|| Value::Object(Map::new()));
    let dependencies = dependencies
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("package.json dependencies must be an object"))?;
    dependencies.insert(package.to_string(), Value::String(requirement.to_string()));
    fs::write(&path, serde_json::to_string_pretty(&value)? + "\n")
        .with_context(|| format!("failed to write '{}'", path.display()))?;
    Ok(())
}
