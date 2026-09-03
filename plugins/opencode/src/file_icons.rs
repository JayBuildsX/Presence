//! OpenCode file-icon resolution.
//!
//! Maps a viewed file to the icon and label shown in the presence, modeled
//! after the VS Code / Material Icon Theme resolution order:
//!
//! 1. **Exact file name** — special and compound file names (e.g.
//!    `Dockerfile`, `Makefile`, `package.json`, `tsconfig.json`,
//!    `vite.config.ts`, `.gitignore`) win over any extension rule, so
//!    `package.json` is a Node.js icon, not a generic JSON icon.
//! 2. **Extension** — the file extension maps to its language/tool icon
//!    (e.g. `.rs` → Rust, `.tsx` → React).
//! 3. **Generic fallback** — an unrecognized file resolves to no icon, so the
//!    presence falls back to the OpenCode identity (the small image) and a
//!    stale icon from the previous file is never left behind.
//!
//! # Icon source
//!
//! The icons are the [Material Icon Theme] SVGs pinned to a fixed npm release
//! ([`ICON_SET_VERSION`]) served through jsDelivr, then converted to 512x512
//! PNGs by the weserv image proxy so Discord's media proxy can rehost them as
//! the large image. Every URL is immutable: there are no `@latest` tags or
//! moving branches, and the OpenCode logo is pinned to a specific commit.
//!
//! [Material Icon Theme]: https://github.com/material-extensions/vscode-material-icon-theme

use std::path::Path;

/// A resolved icon for a file.
///
/// [`image_url`](Self::image_url) and [`label`](Self::label) are always set
/// together: either the file is recognized (both are present) or it is not
/// (both are `None`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IconResolution {
    /// External 512x512 PNG URL for the large image, if the file is recognized.
    pub image_url: Option<String>,
    /// Hover text (the file type name) shown over the large image.
    pub label: Option<String>,
}

impl IconResolution {
    /// Build a recognized resolution for an icon name and its display label.
    fn recognized(icon: &'static str, label: &'static str) -> Self {
        Self {
            image_url: Some(icon_url(icon)),
            label: Some(label.to_string()),
        }
    }
}

/// Pinned npm release of the Material Icon Theme whose SVGs back the icons.
///
/// The associations are the `file_stems` / `file_suffixes` tables of the
/// [Zed Material Icon Theme port](https://github.com/PKief/zed-material-icon-theme)
/// at its v5.29.0 sync; this is the exact npm version that port consumed.
const ICON_SET_VERSION: &str = "5.29.0";

/// Resolves the icon for a file name.
///
/// Resolution is deterministic and side-effect free: no caching, no network.
/// The URL for a recognized file is constructed locally from the icon name.
#[derive(Debug, Default)]
pub struct FileIconResolver;

impl FileIconResolver {
    /// Creates a new resolver.
    pub const fn new() -> Self {
        Self
    }

    /// Resolve the icon for a base file name such as `main.rs` or `Dockerfile`.
    ///
    /// Returns a resolution with both fields `Some` for a recognized file and
    /// both fields `None` for an unrecognized one. The caller must replace
    /// (or drop) any previous large image rather than keep it — the plugin
    /// rebuilds its metadata from scratch each poll, so an unrecognized file
    /// yields an activity without a large image.
    pub fn resolve(&self, file_name: &str) -> IconResolution {
        // Exact file name (case-insensitive) first: `Dockerfile`, `Makefile`,
        // `package.json`, `tsconfig.json`, `vite.config.ts`, ...
        if let Some((icon, label)) = FILE_STEMS
            .iter()
            .find(|(stem, _, _)| stem.eq_ignore_ascii_case(file_name))
            .map(|(_, icon, label)| (*icon, *label))
        {
            return IconResolution::recognized(icon, label);
        }

        // Extension (case-insensitive): `.rs`, `.tsx`, `.py`, ...
        let Some(ext) = Path::new(file_name).extension().and_then(|e| e.to_str()) else {
            return IconResolution {
                image_url: None,
                label: None,
            };
        };

        if let Some((icon, label)) = FILE_SUFFIXES
            .iter()
            .find(|(suffix, _, _)| suffix.eq_ignore_ascii_case(ext))
            .map(|(_, icon, label)| (*icon, *label))
        {
            return IconResolution::recognized(icon, label);
        }

        // Unknown: no icon. The presence keeps only the OpenCode identity.
        IconResolution {
            image_url: None,
            label: None,
        }
    }
}

/// Exact file-name associations: `(file name, icon, display label)`.
///
/// Matched case-insensitively against the base file name. Compound/special
/// names (lockfiles, dotfiles, bundler configs) live here so they beat the
/// plain extension rule.
const FILE_STEMS: &[(&str, &str, &str)] = &[
    // Rust
    ("cargo.toml", "rust", "Cargo.toml"),
    ("cargo.lock", "lock", "Cargo.lock"),
    // Python
    ("pyproject.toml", "python", "Python"),
    ("requirements.txt", "document", "Requirements"),
    // Node.js / TypeScript config
    ("package.json", "nodejs", "Node.js"),
    ("package-lock.json", "lock", "package-lock.json"),
    ("tsconfig.json", "tsconfig", "TypeScript Config"),
    ("tsconfig.base.json", "tsconfig", "TypeScript Config"),
    ("jsconfig.json", "jsconfig", "JavaScript Config"),
    ("yarn.lock", "yarn", "Yarn"),
    ("pnpm-lock.yaml", "pnpm", "pnpm"),
    (".npmrc", "npm", "npm"),
    // PHP
    ("composer.json", "php", "PHP (Composer)"),
    ("composer.lock", "lock", "composer.lock"),
    // Build tooling
    ("makefile", "makefile", "Makefile"),
    ("dockerfile", "docker", "Dockerfile"),
    ("dockerfile.dev", "docker", "Dockerfile"),
    ("dockerfile.prod", "docker", "Dockerfile"),
    (".dockerignore", "docker", "Docker"),
    ("cmakelists.txt", "cmake", "CMake"),
    // Container orchestration
    ("docker-compose.yml", "docker", "Docker Compose"),
    ("docker-compose.yaml", "docker", "Docker Compose"),
    // Bundlers / tooling
    ("vite.config.ts", "vite", "Vite"),
    ("vite.config.js", "vite", "Vite"),
    ("vitest.config.ts", "vitest", "Vitest"),
    ("vitest.config.js", "vitest", "Vitest"),
    ("webpack.config.js", "webpack", "Webpack"),
    ("webpack.config.ts", "webpack", "Webpack"),
    // Lint / format
    (".editorconfig", "editorconfig", "EditorConfig"),
    (".eslintrc", "eslint", "ESLint"),
    (".eslintrc.json", "eslint", "ESLint"),
    (".eslintrc.js", "eslint", "ESLint"),
    (".eslintrc.yml", "eslint", "ESLint"),
    (".eslintrc.yaml", "eslint", "ESLint"),
    (".prettierrc", "prettier", "Prettier"),
    (".prettierrc.json", "prettier", "Prettier"),
    ("prettier.config.js", "prettier", "Prettier"),
    // Git
    (".gitignore", "git", "Git"),
    (".gitattributes", "git", "Git"),
    (".gitmodules", "git", "Git"),
    // Environment / misc
    (".env", "tune", "Environment"),
    // Documentation
    ("readme.md", "readme", "Markdown"),
    ("changelog.md", "changelog", "Changelog"),
    ("contributing.md", "contributing", "Contributing"),
    ("license", "license", "License"),
    ("licence", "license", "License"),
];

/// Extension associations: `(extension, icon, display label)`.
///
/// Matched case-insensitively. The list is curated from the Material Icon
/// Theme `file_suffixes` table plus the common suffixes Zed maps to the same
/// icons through its built-in file types (`js`, `ts`, `html`, `yaml`, `php`).
const FILE_SUFFIXES: &[(&str, &str, &str)] = &[
    // Languages
    ("rs", "rust", "Rust"),
    ("py", "python", "Python"),
    ("pyw", "python", "Python"),
    ("js", "javascript", "JavaScript"),
    ("mjs", "javascript", "JavaScript"),
    ("cjs", "javascript", "JavaScript"),
    ("jsx", "react", "React"),
    ("ts", "typescript", "TypeScript"),
    ("mts", "typescript", "TypeScript"),
    ("cts", "typescript", "TypeScript"),
    ("tsx", "react_ts", "React (TypeScript)"),
    ("go", "go", "Go"),
    ("java", "java", "Java"),
    ("c", "c", "C"),
    ("h", "h", "C"),
    ("cpp", "cpp", "C++"),
    ("cc", "cpp", "C++"),
    ("cxx", "cpp", "C++"),
    ("hpp", "hpp", "C++"),
    ("hh", "hpp", "C++"),
    ("cs", "csharp", "C#"),
    ("php", "php", "PHP"),
    ("rb", "ruby", "Ruby"),
    ("dart", "dart", "Dart"),
    ("kt", "kotlin", "Kotlin"),
    ("swift", "swift", "Swift"),
    ("r", "r", "R"),
    ("lua", "lua", "Lua"),
    ("hs", "haskell", "Haskell"),
    ("scala", "scala", "Scala"),
    ("zig", "zig", "Zig"),
    // Markup / styles
    ("html", "html", "HTML"),
    ("htm", "html", "HTML"),
    ("css", "css", "CSS"),
    ("scss", "sass", "SCSS"),
    ("sass", "sass", "Sass"),
    ("less", "less", "Less"),
    ("vue", "vue", "Vue"),
    ("svelte", "svelte", "Svelte"),
    ("astro", "astro", "Astro"),
    ("svg", "svg", "SVG"),
    // Data / markup documents
    ("json", "json", "JSON"),
    ("jsonc", "json", "JSON"),
    ("json5", "json", "JSON"),
    ("yaml", "yaml", "YAML"),
    ("yml", "yaml", "YAML"),
    ("toml", "toml", "TOML"),
    ("xml", "xml", "XML"),
    ("md", "markdown", "Markdown"),
    ("mdx", "mdx", "MDX"),
    // Databases
    ("sql", "database", "SQL"),
    ("sqlite", "database", "SQLite"),
    // Shells / scripts
    ("sh", "console", "Shell"),
    ("bash", "console", "Shell"),
    ("zsh", "console", "Shell"),
    ("fish", "console", "Shell"),
    ("bat", "console", "Batch"),
    ("cmd", "console", "Batch"),
    ("ps1", "powershell", "PowerShell"),
    ("psm1", "powershell", "PowerShell"),
    ("psd1", "powershell", "PowerShell"),
    // Plain text / generic
    ("txt", "document", "Text"),
    ("log", "log", "Log"),
    ("env", "tune", "Environment"),
    ("ini", "settings", "Config"),
    ("conf", "settings", "Config"),
    ("lock", "lock", "Lock file"),
];

/// Build the external 512x512 PNG URL for an icon name.
///
/// The source SVG is served from the pinned npm release by jsDelivr and
/// converted to PNG by the weserv image proxy. No mutable tags or branches.
fn icon_url(icon: &str) -> String {
    let source = format!(
        "https://cdn.jsdelivr.net/npm/material-icon-theme@{ICON_SET_VERSION}/icons/{icon}.svg"
    );
    format!(
        "https://wsrv.nl/?url={}&output=png&w=512&h=512&fit=contain",
        encode_url(&source)
    )
}

/// Percent-encode a URL for use as the weserv `url` query parameter.
///
/// Keeps RFC 3986 unreserved characters literal and hex-encodes everything
/// else (`%XX`), matching the encodings the weserv pipeline was verified with.
fn encode_url(url: &str) -> String {
    let mut out = String::with_capacity(url.len() * 3);
    for &byte in url.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// Returns the base file name (including extension) for a path.
///
/// Returns `None` for a path with no final component (e.g. a trailing
/// separator or the filesystem root).
pub fn file_name_from_path(path: &str) -> Option<String> {
    let name = Path::new(path).file_name()?.to_string_lossy().to_string();
    if name.is_empty() {
        None
    } else {
        Some(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolve(name: &str) -> IconResolution {
        FileIconResolver::new().resolve(name)
    }

    /// Assert a recognized resolution and return its URL.
    fn assert_icon(name: &str, expected_icon: &str) -> String {
        let resolution = resolve(name);
        let url = resolution
            .image_url
            .unwrap_or_else(|| panic!("{name} should resolve to an icon"));
        assert!(
            url.contains(&format!("icons%2F{expected_icon}.svg")),
            "{name}: expected icon {expected_icon}, got {url}"
        );
        assert!(resolution.label.is_some(), "{name}: label missing");
        url
    }

    #[test]
    fn resolves_required_file_types() {
        assert_icon("lib.rs", "rust");
        assert_icon("main.rs", "rust");
        assert_icon("script.py", "python");
        assert_icon("main.ts", "typescript");
        assert_icon("component.tsx", "react_ts");
        assert_icon("index.html", "html");
        assert_icon("style.css", "css");
        assert_icon("image.svg", "svg");
        assert_icon("data.json", "json");
        assert_icon("config.toml", "toml");
        assert_icon("Cargo.toml", "rust");
        assert_icon("config.yaml", "yaml");
        assert_icon("config.yml", "yaml");
        assert_icon("README.md", "readme");
        assert_icon("query.sql", "database");
        assert_icon("Dockerfile", "docker");
        assert_icon("Makefile", "makefile");
    }

    #[test]
    fn exact_file_names_win_over_extension() {
        // package.json is a Node.js icon, not a generic JSON icon.
        assert_icon("package.json", "nodejs");
        // tsconfig.json is its own icon, not a generic JSON or TS icon.
        assert_icon("tsconfig.json", "tsconfig");
        // vite.config.ts is a Vite icon, not a TypeScript icon.
        assert_icon("vite.config.ts", "vite");
        // Cargo.toml is a Rust icon, while plain config.toml is TOML.
        assert_icon("Cargo.toml", "rust");
        assert_icon("config.toml", "toml");
        // composer.json is a PHP icon, distinct from data.json.
        assert_icon("composer.json", "php");
    }

    #[test]
    fn exact_file_names_without_extension() {
        assert_icon("Dockerfile", "docker");
        assert_icon("Makefile", "makefile");
        assert_icon(".gitignore", "git");
        assert_icon(".editorconfig", "editorconfig");
        assert_icon(".env", "tune");
    }

    #[test]
    fn extension_match_is_case_insensitive() {
        assert_icon("MAIN.RS", "rust");
        assert_icon("App.PY", "python");
        assert_icon("readme.MD", "readme");
        assert_icon("DOCKERFILE", "docker");
    }

    #[test]
    fn unknown_files_resolve_to_no_icon() {
        for name in ["blob.xyz", "file", "Makefile2", ""] {
            let resolution = resolve(name);
            assert!(
                resolution.image_url.is_none() && resolution.label.is_none(),
                "{name:?} should have no icon"
            );
        }
    }

    #[test]
    fn image_and_label_are_paired() {
        let resolver = FileIconResolver::new();
        for name in [
            "main.rs",
            "index.html",
            "package.json",
            "Dockerfile",
            "Makefile",
            "query.sql",
            "README.md",
            "Cargo.toml",
            "unknown.zzz",
        ] {
            let resolution = resolver.resolve(name);
            assert_eq!(
                resolution.image_url.is_some(),
                resolution.label.is_some(),
                "{name}: image and label must be set together"
            );
        }
    }

    #[test]
    fn urls_are_pinned_and_well_formed() {
        let url = icon_url("rust");
        assert!(url.contains("wsrv.nl/?url="), "weserv proxy: {url}");
        assert!(
            url.contains("material-icon-theme%405.29.0%2Ficons%2Frust.svg"),
            "pinned npm release: {url}"
        );
        assert!(url.contains("output=png&w=512&h=512&fit=contain"), "{url}");
        assert!(!url.contains("@latest"), "no mutable tag: {url}");
        assert!(!url.contains("/dev/"), "no moving branch: {url}");
        assert!(
            url.contains(
                "url=https%3A%2F%2Fcdn.jsdelivr.net%2Fnpm%2Fmaterial-icon-theme%405.29.0%2Ficons%2Frust.svg"
            ),
            "encoded source URL: {url}"
        );
    }

    #[test]
    fn encode_url_leaves_unreserved_characters_literal() {
        assert_eq!(
            encode_url("https://cdn.jsdelivr.net/npm/material-icon-theme@5.29.0/icons/rust.svg"),
            "https%3A%2F%2Fcdn.jsdelivr.net%2Fnpm%2Fmaterial-icon-theme%405.29.0%2Ficons%2Frust.svg"
        );
    }

    #[test]
    fn file_name_from_full_path() {
        assert_eq!(
            file_name_from_path("C:\\Users\\HP\\Desktop\\PresenceHUB\\src\\main.rs"),
            Some("main.rs".to_string())
        );
    }

    #[test]
    fn file_name_from_plain_name() {
        assert_eq!(file_name_from_path("main.rs"), Some("main.rs".to_string()));
    }

    #[test]
    fn file_name_from_root() {
        assert_eq!(file_name_from_path("C:\\"), None);
    }
}
