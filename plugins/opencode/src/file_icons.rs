//! OpenCode file-icon resolution.
//!
//! Maps a viewed file to the icon and label shown in the presence, using the
//! same method as `brkpoint/VSCode-Discord-RPC`: `large_image` is a short
//! asset key uploaded to the Discord application (the vsc-presence app
//! `1273940066603106328`), never an external URL.
//!
//! Resolution order:
//!
//! 1. **Exact file name** — special and compound file names (e.g.
//!    `Cargo.toml`, `package.json`, `vite.config.ts`, `.gitignore`) win over
//!    any extension rule.
//! 2. **Extension** — the file extension maps to its language asset key
//!    (e.g. `.rs` → `rust`, `.tsx` → `tsx`).
//! 3. **Fallback** — an unrecognized file resolves to no icon, so the
//!    presence carries no `large_image` and a stale icon from the previous
//!    file is never left behind.
//!
//! # Asset keys
//!
//! Only keys hosted by the Discord application may be used:
//! `javascript`, `tsx`, `typescript`, `jsx`, `vue`, `scss`, `json`,
//! `ignore`, `markdown`, `html`, `css`, `c`, `cpp`, `csharp`, `rust`,
//! `swift`, `lua`, `python`, `java`, `asm`, `bin`, `vscode`. Anything else
//! renders blank in Discord, so files without a hosted key resolve to no
//! icon instead of a guess.

use std::path::Path;

/// A resolved icon for a file.
///
/// [`image_key`](Self::image_key) and [`label`](Self::label) are always set
/// together: either the file is recognized (both are present) or it is not
/// (both are `None`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IconResolution {
    /// Discord application asset key for the large image, if the file is
    /// recognized.
    pub image_key: Option<&'static str>,
    /// Hover text (the file type name) shown over the large image.
    pub label: Option<&'static str>,
}

impl IconResolution {
    /// Build a recognized resolution for an asset key and its display label.
    fn recognized(image_key: &'static str, label: &'static str) -> Self {
        Self {
            image_key: Some(image_key),
            label: Some(label),
        }
    }
}

/// Resolves the icon for a file name.
///
/// Resolution is deterministic and side-effect free: no caching, no network.
/// The result is a Discord application asset key, never a URL.
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
        // Exact file name (case-insensitive) first: `Cargo.toml`,
        // `package.json`, `vite.config.ts`, ...
        if let Some((key, label)) = FILE_STEMS
            .iter()
            .find(|(stem, _, _)| stem.eq_ignore_ascii_case(file_name))
            .map(|(_, key, label)| (*key, *label))
        {
            return IconResolution::recognized(key, label);
        }

        // Extension (case-insensitive): `.rs`, `.tsx`, `.py`, ...
        let Some(ext) = Path::new(file_name).extension().and_then(|e| e.to_str()) else {
            return IconResolution {
                image_key: None,
                label: None,
            };
        };

        if let Some((key, label)) = FILE_SUFFIXES
            .iter()
            .find(|(suffix, _, _)| suffix.eq_ignore_ascii_case(ext))
            .map(|(_, key, label)| (*key, *label))
        {
            return IconResolution::recognized(key, label);
        }

        // Unknown: no icon. Unmapped files (Dockerfiles, lockfiles, config
        // formats without a hosted key) carry no large image.
        IconResolution {
            image_key: None,
            label: None,
        }
    }
}

/// Exact file-name associations: `(file name, asset key, display label)`.
///
/// Matched case-insensitively against the base file name. Every key must be
/// hosted by the Discord application; names without a hosted key are left
/// out so they fall through to the extension rule or to no icon.
const FILE_STEMS: &[(&str, &str, &str)] = &[
    // Rust
    ("cargo.toml", "rust", "Cargo.toml"),
    // Node.js / TypeScript config (JSON payloads, JSON-family key)
    ("package.json", "json", "Node.js"),
    ("package-lock.json", "json", "package-lock.json"),
    ("tsconfig.json", "json", "TypeScript Config"),
    ("tsconfig.base.json", "json", "TypeScript Config"),
    ("jsconfig.json", "json", "JavaScript Config"),
    ("composer.json", "json", "Composer"),
    // Bundlers / tooling (real source language keys)
    ("vite.config.ts", "typescript", "Vite"),
    ("vite.config.js", "javascript", "Vite"),
    ("vitest.config.ts", "typescript", "Vitest"),
    ("vitest.config.js", "javascript", "Vitest"),
    ("webpack.config.js", "javascript", "Webpack"),
    ("webpack.config.ts", "typescript", "Webpack"),
    // Lint / format
    ("prettier.config.js", "javascript", "Prettier"),
    (".prettierrc", "json", "Prettier"),
    (".prettierrc.json", "json", "Prettier"),
    (".eslintrc", "json", "ESLint"),
    (".eslintrc.json", "json", "ESLint"),
    (".eslintrc.js", "javascript", "ESLint"),
    // Git
    (".gitignore", "ignore", "Git"),
    (".dockerignore", "ignore", "Docker"),
    // Documentation
    ("readme.md", "markdown", "Markdown"),
    ("changelog.md", "markdown", "Changelog"),
    ("contributing.md", "markdown", "Contributing"),
];

/// Extension associations: `(extension, asset key, display label)`.
///
/// Matched case-insensitively. Keys mirror the reference language→icon map;
/// extensions without a hosted key (Go, PHP, TOML, YAML, Dockerfiles, shell
/// scripts, plain text, ...) resolve to no icon.
const FILE_SUFFIXES: &[(&str, &str, &str)] = &[
    // Languages with hosted keys
    ("rs", "rust", "Rust"),
    ("py", "python", "Python"),
    ("pyw", "python", "Python"),
    ("js", "javascript", "JavaScript"),
    ("mjs", "javascript", "JavaScript"),
    ("cjs", "javascript", "JavaScript"),
    ("jsx", "jsx", "React"),
    ("ts", "typescript", "TypeScript"),
    ("mts", "typescript", "TypeScript"),
    ("cts", "typescript", "TypeScript"),
    ("tsx", "tsx", "React (TypeScript)"),
    ("java", "java", "Java"),
    ("c", "c", "C"),
    ("h", "c", "C"),
    ("cpp", "cpp", "C++"),
    ("cc", "cpp", "C++"),
    ("cxx", "cpp", "C++"),
    ("hpp", "cpp", "C++"),
    ("hh", "cpp", "C++"),
    ("cs", "csharp", "C#"),
    ("swift", "swift", "Swift"),
    ("lua", "lua", "Lua"),
    ("asm", "asm", "Assembly"),
    ("s", "asm", "Assembly"),
    ("bin", "bin", "Binary"),
    // Markup / styles with hosted keys
    ("html", "html", "HTML"),
    ("htm", "html", "HTML"),
    ("css", "css", "CSS"),
    ("scss", "scss", "SCSS"),
    ("sass", "scss", "Sass"),
    ("vue", "vue", "Vue"),
    // Data documents with hosted keys
    ("json", "json", "JSON"),
    ("jsonc", "json", "JSON"),
    ("json5", "json", "JSON"),
    ("md", "markdown", "Markdown"),
];

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

    /// Assert a recognized resolution and return its asset key.
    fn assert_key(name: &str, expected_key: &str) -> &'static str {
        let resolution = resolve(name);
        let key = resolution
            .image_key
            .unwrap_or_else(|| panic!("{name} should resolve to an icon"));
        assert_eq!(
            key, expected_key,
            "{name}: expected asset key {expected_key}, got {key}"
        );
        assert!(resolution.label.is_some(), "{name}: label missing");
        key
    }

    /// Assert an unrecognized file resolves to no icon.
    fn assert_no_icon(name: &str) {
        let resolution = resolve(name);
        assert!(
            resolution.image_key.is_none() && resolution.label.is_none(),
            "{name:?} should have no icon"
        );
    }

    #[test]
    fn resolves_supported_file_types_to_asset_keys() {
        assert_key("lib.rs", "rust");
        assert_key("main.rs", "rust");
        assert_key("script.py", "python");
        assert_key("main.ts", "typescript");
        assert_key("component.tsx", "tsx");
        assert_key("app.jsx", "jsx");
        assert_key("app.js", "javascript");
        assert_key("index.html", "html");
        assert_key("style.css", "css");
        assert_key("style.scss", "scss");
        assert_key("app.vue", "vue");
        assert_key("data.json", "json");
        assert_key("README.md", "markdown");
        assert_key("Cargo.toml", "rust");
        assert_key("package.json", "json");
        assert_key("Main.java", "java");
        assert_key("main.c", "c");
        assert_key("main.cpp", "cpp");
        assert_key("app.cs", "csharp");
        assert_key("app.swift", "swift");
        assert_key("app.lua", "lua");
    }

    #[test]
    fn exact_file_names_win_over_extension() {
        // Cargo.toml carries the Rust key while plain config.toml has none.
        assert_key("Cargo.toml", "rust");
        assert_no_icon("config.toml");
        // Tool configs resolve to their real language keys.
        assert_key("vite.config.ts", "typescript");
        assert_key("vite.config.js", "javascript");
        // package.json is JSON-family, distinct from a source file.
        assert_key("package.json", "json");
        assert_key("composer.json", "json");
    }

    #[test]
    fn files_without_a_hosted_key_resolve_to_no_icon() {
        for name in [
            "Dockerfile",
            "Makefile",
            "config.toml",
            "config.yaml",
            "config.yml",
            "query.sql",
            "image.svg",
            "notes.txt",
            "deploy.sh",
            "main.go",
            ".env",
            "LICENSE",
            "blob.xyz",
            "file",
            "Makefile2",
            "",
        ] {
            assert_no_icon(name);
        }
    }

    #[test]
    fn extension_match_is_case_insensitive() {
        assert_key("MAIN.RS", "rust");
        assert_key("App.PY", "python");
        assert_key("readme.MD", "markdown");
        assert_key("App.TSX", "tsx");
    }

    #[test]
    fn image_key_and_label_are_paired() {
        let resolver = FileIconResolver::new();
        for name in [
            "main.rs",
            "index.html",
            "package.json",
            "Dockerfile",
            "query.sql",
            "README.md",
            "Cargo.toml",
            "unknown.zzz",
        ] {
            let resolution = resolver.resolve(name);
            assert_eq!(
                resolution.image_key.is_some(),
                resolution.label.is_some(),
                "{name}: key and label must be set together"
            );
        }
    }

    #[test]
    fn keys_are_short_asset_keys_never_urls() {
        // The reference method uses uploaded asset keys; a URL here would
        // render blank. Every hosted key must be a bare key.
        for name in [
            "main.rs",
            "script.py",
            "main.ts",
            "component.tsx",
            "index.html",
            "data.json",
            "README.md",
            "Cargo.toml",
            "package.json",
        ] {
            let key = assert_key(name, resolve(name).image_key.unwrap());
            assert!(
                !key.contains("http") && !key.contains("://") && !key.contains('/'),
                "{name}: asset key must not be a URL, got {key}"
            );
        }
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
