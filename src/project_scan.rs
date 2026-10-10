use crate::backend::{Classification, Project};
use anyhow::{Context, Result, bail};
use regex::Regex;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeSet, VecDeque},
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::OnceLock,
    time::UNIX_EPOCH,
};

pub fn read_json(path: &Path) -> Result<Value> {
    let file = fs::File::open(path)?;
    let mut bytes = Vec::new();
    file.take(2 * 1024 * 1024).read_to_end(&mut bytes)?;
    Ok(serde_json::from_slice(&bytes)?)
}

pub fn key(path: &str) -> String {
    path.replace('/', "\\")
        .trim_end_matches('\\')
        .to_lowercase()
}

pub fn roots(values: &[String]) -> Result<Vec<String>> {
    if values.len() > 64 {
        bail!("项目目录必须为列表，最多支持 64 个目录");
    }
    let mut seen = BTreeSet::new();
    let mut result = Vec::new();
    for value in values {
        let path = PathBuf::from(value.trim().trim_matches('"'));
        if !path.is_absolute() {
            bail!("项目目录必须使用绝对路径");
        }
        let path = std::path::absolute(path).context("无法读取项目目录路径")?;
        let value = crate::storage::display_path(&path);
        if seen.insert(key(&value)) {
            result.push(value);
        }
    }
    Ok(result)
}

pub fn dependencies_present(directory: &Path, pkg: &Value) -> bool {
    if ["dependencies", "devDependencies", "optionalDependencies"]
        .iter()
        .all(|name| pkg[*name].as_object().is_none_or(|items| items.is_empty()))
    {
        return true;
    }
    for current in directory.ancestors() {
        let manifest = if current == directory {
            pkg.clone()
        } else {
            read_json(&current.join("package.json")).unwrap_or_default()
        };
        if current == directory
            || !manifest["workspaces"].is_null()
            || current.join("pnpm-workspace.yaml").is_file()
        {
            if ["node_modules", ".pnp.cjs", ".pnp.js"]
                .iter()
                .any(|name| current.join(name).exists())
            {
                return true;
            }
        }
    }
    false
}

pub fn package_manager(directory: &Path, pkg: &Value) -> Result<&'static str> {
    if let Some(specifier) = pkg["packageManager"].as_str() {
        let manager = specifier.split('@').next().unwrap_or("");
        return match manager {
            "npm" => Ok("npm"),
            "pnpm" => Ok("pnpm"),
            "yarn" => Ok("yarn"),
            "bun" => Ok("bun"),
            _ => bail!("package.json 中的包管理器暂不支持，请在项目目录手动安装依赖。"),
        };
    }
    Ok(if directory.join("pnpm-lock.yaml").is_file() {
        "pnpm"
    } else if directory.join("yarn.lock").is_file() {
        "yarn"
    } else if directory.join("bun.lock").is_file() || directory.join("bun.lockb").is_file() {
        "bun"
    } else {
        "npm"
    })
}

pub fn development_script(pkg: &Value) -> Option<&'static str> {
    if pkg["scripts"]["dev:web"].is_string() {
        Some("dev:web")
    } else if pkg["scripts"]["dev"].is_string() {
        Some("dev")
    } else {
        None
    }
}

struct Rule {
    name: &'static str,
    game: bool,
    package: Regex,
    source: Option<Regex>,
    file: Option<Regex>,
    script: Option<Regex>,
    extension: Option<&'static str>,
}
fn rules() -> &'static [Rule] {
    static RULES: OnceLock<Vec<Rule>> = OnceLock::new();
    RULES.get_or_init(|| {
        let definitions = [
            (
                "Phaser",
                true,
                r"^(phaser|phaser-ce)(/|$)",
                r"\bPhaser\s*\.\s*(Game|Scene|AUTO|CANVAS|WEBGL)\b",
                "",
                r"(?i)(^|/)phaser([.@/-]|$)",
                "",
            ),
            (
                "Babylon.js",
                true,
                r"^(@babylonjs/core|babylonjs)(/|$)",
                r"\bBABYLON\s*\.\s*(Engine|Scene|MeshBuilder|ArcRotateCamera)\b",
                "",
                r"(?i)(^|/)babylon(\.max|\.min)?\.js($|\?)",
                "",
            ),
            (
                "Cocos",
                true,
                r"^(cc|cocos2d|cocos2d-js|@cocos/engine)(/|$)",
                r"\bcc\s*\.\s*(game|director|Class|_decorator)\b",
                "",
                r"(?i)(^|/)(cocos2d(-js)?|cocos-creator)([.@/-]|$)",
                "",
            ),
            (
                "LayaAir",
                true,
                r"^(layaair|layaair-js|layaair2-cmd|layaair3-cmd)(/|$)",
                r"\bLaya\s*\.\s*(init|stage|Scene|Sprite|Laya3D)\b",
                "",
                r"(?i)(^|/)(laya\.(core|webgl|d3)|layaair)([.@/-]|$)",
                "",
            ),
            (
                "PixiJS",
                true,
                r"^(pixi\.js(-legacy)?|@pixi/app|@pixi/core|@pixi/react)(/|$)",
                r"\bPIXI\s*\.\s*(Application|Container|Sprite|Renderer)\b",
                "",
                r"(?i)(^|/)pixi([.@/-]|$)",
                "",
            ),
            (
                "Three.js",
                true,
                r"^(three|@react-three/fiber)(/|$)",
                r"\bTHREE\s*\.\s*(Scene|WebGLRenderer|PerspectiveCamera|Mesh)\b",
                "",
                r"(?i)(^|/)three([.@/-]|$)",
                "",
            ),
            (
                "Galacean",
                true,
                r"^(@galacean/engine(-core)?|oasis-engine)(/|$)",
                r"\b(Galacean|galacean|oasis)\s*\.\s*(WebGLEngine|Engine)\b",
                "",
                r"(?i)(^|/)(galacean|oasis-engine)([.@/-]|$)",
                "",
            ),
            (
                "Next.js",
                false,
                r"^next(/|$)",
                "",
                r"^next\.config\.[cm]?[jt]s$",
                "",
                "",
            ),
            (
                "Nuxt",
                false,
                r"^(nuxt|nuxt3)(/|$)",
                "",
                r"^nuxt\.config\.[cm]?[jt]s$",
                "",
                "",
            ),
            ("SvelteKit", false, r"^@sveltejs/kit(/|$)", "", "", "", ""),
            (
                "Astro",
                false,
                r"^astro(/|$)",
                "",
                r"^astro\.config\.[cm]?[jt]s$",
                "",
                "",
            ),
            (
                "Angular",
                false,
                r"^@angular/core(/|$)",
                "",
                r"^angular\.json$",
                "",
                "",
            ),
            (
                "React",
                false,
                r"^(react|react-dom)(/|$)",
                r"\bReact(DOM)?\s*\.\s*(createElement|createRoot|render|useState)\b",
                "",
                r"(?i)(^|/)react(-dom)?([.@/-]|$)",
                "",
            ),
            (
                "Vue",
                false,
                r"^(vue|@vitejs/plugin-vue)(/|$)",
                r"\bVue\s*\.\s*(createApp|createSSRApp|extend|component)\b",
                "",
                r"(?i)(^|/)vue([.@/-]|$)",
                ".vue",
            ),
            (
                "Svelte",
                false,
                r"^svelte(/|$)",
                "",
                r"^svelte\.config\.[cm]?[jt]s$",
                "",
                ".svelte",
            ),
            ("Solid", false, r"^solid-js(/|$)", "", "", "", ""),
            (
                "Preact",
                false,
                r"^preact(/|$)",
                r"\bpreact\s*\.\s*(h|render)\b",
                "",
                r"(?i)(^|/)preact([.@/-]|$)",
                "",
            ),
        ];
        let optional = |value: &str| (!value.is_empty()).then(|| Regex::new(value).unwrap());
        definitions
            .into_iter()
            .map(
                |(name, game, package, source, file, script, extension)| Rule {
                    name,
                    game,
                    package: Regex::new(package).unwrap(),
                    source: optional(source),
                    file: optional(file),
                    script: optional(script),
                    extension: (!extension.is_empty()).then_some(extension),
                },
            )
            .collect()
    })
}

fn source_file(name: &str) -> bool {
    static SOURCE: OnceLock<Regex> = OnceLock::new();
    SOURCE
        .get_or_init(|| Regex::new(r"(?i)\.(html|[cm]?[jt]sx?|vue|svelte|astro)$").unwrap())
        .is_match(name)
}

fn samples(directory: &Path, files: &[String]) -> Vec<(String, String)> {
    let mut candidates: Vec<_> = files
        .iter()
        .filter(|name| source_file(name))
        .cloned()
        .collect();
    candidates.sort_by_key(|name| (name != "index.html", name.clone()));
    candidates.truncate(12);
    let mut folders: VecDeque<_> = ["src", "app", "pages", "scripts", "assets/scripts"]
        .into_iter()
        .map(|p| (PathBuf::from(p), 0))
        .collect();
    for _ in 0..24 {
        let Some((relative, depth)) = folders.pop_front() else {
            break;
        };
        if candidates.len() >= 32 {
            break;
        }
        let target = directory.join(&relative);
        if !fs::symlink_metadata(&target).is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink()) {
            continue;
        }
        let Ok(entries) = fs::read_dir(&target) else {
            continue;
        };
        for entry in entries.take(128).flatten() {
            if candidates.len() >= 32 {
                break;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.')
                || matches!(
                    name.as_str(),
                    "node_modules" | "dist" | "build" | "vendor" | "coverage" | "__tests__"
                )
            {
                continue;
            }
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() && depth < 2 {
                folders.push_back((relative.join(name), depth + 1));
            } else if kind.is_file()
                && source_file(&name)
                && ![".min.", ".test.", ".spec."]
                    .iter()
                    .any(|suffix| name.contains(suffix))
            {
                candidates.push(relative.join(name).to_string_lossy().into_owned());
            }
        }
    }
    let mut result = Vec::new();
    let mut remaining = 512 * 1024;
    for name in candidates {
        if remaining == 0 {
            break;
        }
        let target = directory.join(&name);
        if !fs::symlink_metadata(&target).is_ok_and(|m| m.is_file() && !m.file_type().is_symlink())
        {
            continue;
        }
        if let Ok(file) = fs::File::open(target) {
            let mut bytes = Vec::new();
            if file
                .take(remaining.min(64 * 1024) as u64)
                .read_to_end(&mut bytes)
                .is_ok()
            {
                remaining -= bytes.len();
                result.push((name, String::from_utf8_lossy(&bytes).into_owned()));
            }
        }
    }
    result
}

fn package_from(specifier: &str) -> String {
    static VERSION: OnceLock<Regex> = OnceLock::new();
    let mut value = specifier.split(['?', '#']).next().unwrap_or("").to_owned();
    if value.starts_with("http:") || value.starts_with("https:") || value.starts_with("//") {
        if value.starts_with("//") {
            value = format!("https:{value}");
        }
        let Ok(url) = url::Url::parse(&value) else {
            return String::new();
        };
        value = url
            .path()
            .trim_start_matches('/')
            .trim_start_matches("npm/")
            .to_owned();
    }
    VERSION
        .get_or_init(|| Regex::new(r"(@[^/@]+/[^/@]+|^[^/@]+)@[^/]+").unwrap())
        .replace(&value, "$1")
        .into_owned()
}

fn classify(directory: &Path, pkg: &Value, files: &[String]) -> Classification {
    let mut packages = BTreeSet::new();
    for section in [
        "dependencies",
        "devDependencies",
        "peerDependencies",
        "optionalDependencies",
    ] {
        if let Some(values) = pkg[section].as_object() {
            for (name, value) in values {
                packages.insert(name.clone());
                if let Some(value) = value.as_str().and_then(|v| v.strip_prefix("npm:")) {
                    packages.insert(package_from(value));
                }
            }
        }
    }
    static IMPORT: OnceLock<Regex> = OnceLock::new();
    static SCRIPT: OnceLock<Regex> = OnceLock::new();
    static IMPORT_MAP: OnceLock<Regex> = OnceLock::new();
    let imports = IMPORT.get_or_init(|| {
        Regex::new(r#"(?:\bfrom\s*|\bimport\s*(?:\(\s*)?|\brequire\s*\(\s*)['"]([^'"\r\n]+)['"]"#)
            .unwrap()
    });
    let scripts = SCRIPT
        .get_or_init(|| Regex::new(r#"(?i)<script\b[^>]*\bsrc\s*=\s*['"]([^'"]+)['"]"#).unwrap());
    let maps = IMPORT_MAP.get_or_init(|| {
        Regex::new(r#"(?is)<script\b[^>]*\btype\s*=\s*['"]importmap['"][^>]*>(.*?)</script>"#)
            .unwrap()
    });
    let samples = samples(directory, files);
    let mut urls = Vec::new();
    for (_, source) in &samples {
        for item in imports.captures_iter(source) {
            packages.insert(package_from(&item[1]));
        }
        for item in scripts.captures_iter(source) {
            urls.push(item[1].to_owned());
            packages.insert(package_from(&item[1]));
        }
        for item in maps.captures_iter(source) {
            if let Ok(map) = serde_json::from_str::<Value>(&item[1]) {
                if let Some(values) = map["imports"].as_object() {
                    for (name, value) in values {
                        packages.insert(name.clone());
                        if let Some(value) = value.as_str() {
                            packages.insert(package_from(value));
                        }
                    }
                }
            }
        }
    }
    let cocos = !pkg["creator"]["version"].is_null()
        || ["project.json", "assets", "settings"]
            .iter()
            .all(|name| files.iter().any(|file| file == name));
    let laya = files
        .iter()
        .any(|name| name == ".laya" || name.to_lowercase().ends_with(".laya"));
    let mut classification = Classification::default();
    for rule in rules() {
        if packages.iter().any(|name| rule.package.is_match(name))
            || rule
                .file
                .as_ref()
                .is_some_and(|r| files.iter().any(|name| r.is_match(name)))
            || rule
                .source
                .as_ref()
                .is_some_and(|r| samples.iter().any(|(_, source)| r.is_match(source)))
            || rule
                .script
                .as_ref()
                .is_some_and(|r| urls.iter().any(|url| r.is_match(url)))
            || rule
                .extension
                .is_some_and(|ext| samples.iter().any(|(name, _)| name.ends_with(ext)))
            || rule.name == "Cocos" && cocos
            || rule.name == "LayaAir" && laya
        {
            if rule.game {
                classification.game_engines.push(rule.name.into());
            } else {
                classification.frameworks.push(rule.name.into());
            }
        }
    }
    classification
}

pub fn scan(directories: &[String]) -> Result<Vec<Project>> {
    let mut projects = Vec::new();
    for root in directories {
        let entries = fs::read_dir(root).with_context(|| format!("无法扫描项目目录：{root}"))?;
        for entry in entries.flatten() {
            if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let directory = entry.path();
            let pkg = read_json(&directory.join("package.json")).unwrap_or_default();
            let Ok(entries) = fs::read_dir(&directory) else {
                continue;
            };
            let files: Vec<_> = entries
                .flatten()
                .map(|item| item.file_name().to_string_lossy().into_owned())
                .collect();
            if !files.iter().any(|name| name == "index.html") && development_script(&pkg).is_none()
            {
                continue;
            }
            let metadata = fs::metadata(&directory)?;
            let timestamp = |time: std::io::Result<std::time::SystemTime>| {
                time.ok()
                    .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                    .map(|duration| duration.as_secs_f64() * 1000.)
            };
            let mut modified = timestamp(metadata.modified()).unwrap_or_default();
            for file in ["src", "index.html", "package.json"] {
                if let Ok(metadata) = fs::metadata(directory.join(file)) {
                    modified = modified.max(timestamp(metadata.modified()).unwrap_or_default());
                }
            }
            let path = crate::storage::display_path(&directory);
            let hash = Sha256::digest(path.to_lowercase().as_bytes());
            let id = hash[..8]
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            projects.push(Project {
                id,
                name,
                path,
                root: root.clone(),
                classification: Some(classify(&directory, &pkg, &files)),
                modified,
                created_at: timestamp(metadata.created()),
                dependencies_installed: dependencies_present(&directory, &pkg),
                status: "idle".into(),
                capture: "idle".into(),
                install_state: "idle".into(),
                ..Project::default()
            });
        }
    }
    projects.sort_by(|a, b| b.modified.total_cmp(&a.modified));
    Ok(projects)
}
