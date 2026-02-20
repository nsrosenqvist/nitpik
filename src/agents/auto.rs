//! Auto-profile selection based on file heuristics.
//!
//! When `--profile auto` is used, we select built-in reviewer profiles
//! without an LLM call by analyzing three layers of signals from the diff
//! and the repository root.
//!
//! # Decision layers
//!
//! ## 1. Extension & path classification (per file)
//!
//! Every changed file is classified by its extension and path:
//!
//! | Signal | Examples | Profile |
//! |---|---|---|
//! | Always-frontend extensions | `.vue`, `.svelte`, `.css`, `.html`, `.astro` | `frontend` |
//! | Always-backend extensions | `.rs`, `.go`, `.py`, `.java`, `.php`, … | `backend` |
//! | Ambiguous JS/TS extensions | `.js`, `.ts`, `.tsx`, `.mjs`, … | *see below* |
//!
//! For JS/TS files the extension alone is not enough — a `.ts` file could
//! be an Express controller or a React component. We disambiguate with:
//!
//! - **Filename suffixes**: NestJS patterns like `*.controller.ts`,
//!   `*.service.ts`, `*.module.ts` → backend.
//! - **Path segments**: `controllers/`, `middleware/`, `routes/` → backend;
//!   `components/`, `pages/`, `hooks/` → frontend.
//! - **Bare `.tsx`/`.jsx`** without a backend signal → leans frontend.
//!
//! ## 2. Project root inspection (repo-level tiebreaker)
//!
//! When JS/TS path signals are **absent or one-sided** (e.g. only frontend
//! paths matched, but unclassified `.ts` files also exist), we inspect the
//! repository root for stronger indicators:
//!
//! - **Config files**: `nest-cli.json`, `wrangler.toml`, `nodemon.json`, …
//!   → backend.
//! - **`package.json` dependencies**: `express`, `fastify`, `@nestjs/core`,
//!   `prisma`, … → backend; `react`, `vue`, `next`, `svelte`, … → frontend.
//!
//! The root signal is **merged** with path signals (not replaced), so a
//! monorepo with `wrangler.toml` + frontend components will get both
//! profiles.
//!
//! If neither paths nor the root produce any signal, JS/TS defaults to
//! `frontend`.
//!
//! ## 3. Architect triggers (cross-cutting)
//!
//! The `architect` profile is added when the diff suggests a structural or
//! cross-cutting change:
//!
//! - **Architectural files**: CI configs, Dockerfiles, IaC (Terraform,
//!   Pulumi, CDK), build configs, dependency manifests, database
//!   migrations, API definitions (`.proto`, `.graphql`, OpenAPI).
//! - **Large diffs**: ≥ `LARGE_DIFF_FILE_THRESHOLD` files changed.
//! - **Broad diffs**: ≥ `BROAD_DIFF_DIR_THRESHOLD` distinct directories
//!   touched.
//!
//! # Final profile assembly
//!
//! ```text
//! if has_frontend           → add "frontend"
//! if has_backend OR
//!    !has_frontend           → add "backend" (default profile)
//! if has_architect           → add "architect"
//! always                     → add "security"
//! ```
//!
//! The `security` profile is unconditional — every review gets it.
//! `backend` also serves as the catch-all default when nothing matched
//! frontend (e.g. a `README.md`-only diff).

use std::path::Path;

use crate::models::DEFAULT_PROFILE;
use crate::models::diff::FileDiff;

/// Backend-indicating path segments for JS/TS files.
const JS_BACKEND_PATH_SEGMENTS: &[&str] = &[
    "controllers/",
    "middleware/",
    "routes/",
    "handlers/",
    "resolvers/",
    "services/",
    "repositories/",
    "migrations/",
    "seeds/",
    "prisma/",
    "graphql/",
    "db/",
    "schemas/",
    "trpc/",
    "lambdas/",
    "functions/",
];

/// Backend-indicating filename suffixes for JS/TS files (NestJS, etc.).
const JS_BACKEND_FILE_SUFFIXES: &[&str] = &[
    ".controller.ts",
    ".controller.js",
    ".service.ts",
    ".service.js",
    ".middleware.ts",
    ".middleware.js",
    ".resolver.ts",
    ".resolver.js",
    ".module.ts",
    ".guard.ts",
    ".guard.js",
    ".interceptor.ts",
    ".interceptor.js",
    ".pipe.ts",
    ".pipe.js",
    ".gateway.ts",
    ".gateway.js",
    ".entity.ts",
    ".entity.js",
    ".dto.ts",
    ".dto.js",
    ".repository.ts",
    ".repository.js",
];

/// Backend-indicating root filenames (exact matches) for JS/TS files.
const JS_BACKEND_ROOT_FILES: &[&str] = &[
    "server.ts",
    "server.js",
    "server.mts",
    "server.mjs",
    "app.ts",
    "app.js",
    "index.ts", // root index in a non-frontend project often is a server
    "index.js",
];

/// Frontend-indicating path segments for JS/TS files.
const JS_FRONTEND_PATH_SEGMENTS: &[&str] = &[
    "components/",
    "pages/",
    "views/",
    "public/",
    "static/",
    "styles/",
    "hooks/",
    "stores/",
    "layouts/",
    "composables/",
    "assets/",
    "app/", // Next.js app router
];

/// Backend dependency names in `package.json` that strongly indicate a Node backend.
const BACKEND_DEPS: &[&str] = &[
    "express",
    "fastify",
    "@nestjs/core",
    "@nestjs/common",
    "koa",
    "@hapi/hapi",
    "hono",
    "elysia",
    "drizzle-orm",
    "prisma",
    "@prisma/client",
    "typeorm",
    "sequelize",
    "knex",
    "mongoose",
    "pg",
    "mysql2",
    "mongodb",
    "redis",
    "ioredis",
    "bull",
    "bullmq",
    "socket.io",
    "trpc",
    "@trpc/server",
    "graphql-yoga",
    "apollo-server",
    "@apollo/server",
    "mercurius",
    "aws-lambda",
    "serverless",
    "aws-cdk",
    "aws-cdk-lib",
    "grpc",
    "@grpc/grpc-js",
];

/// Config files in the project root that strongly indicate a Node backend.
const BACKEND_ROOT_CONFIG_FILES: &[&str] = &[
    "nest-cli.json",
    "nodemon.json",
    ".sequelizerc",
    "drizzle.config.ts",
    "drizzle.config.js",
    "knexfile.js",
    "knexfile.ts",
    "ormconfig.json",
    "ormconfig.ts",
    "ormconfig.js",
    "pm2.config.js",
    "pm2.ecosystem.config.js",
    "serverless.yml",
    "serverless.ts",
    "wrangler.toml",
];

/// Files and path patterns that indicate cross-cutting / architectural changes.
/// A diff touching these warrants architectural review.
const ARCHITECTURE_FILE_PATTERNS: &[&str] = &[
    // CI / CD
    ".github/workflows/",
    ".gitlab-ci.yml",
    ".circleci/",
    "Jenkinsfile",
    ".buildkite/",
    ".travis.yml",
    "azure-pipelines.yml",
    "bitbucket-pipelines.yml",
    // Containerization / orchestration
    "Dockerfile",
    "docker-compose",
    "compose.yml",
    "compose.yaml",
    "kubernetes/",
    "k8s/",
    "helm/",
    ".dockerignore",
    // Infrastructure as Code
    "terraform/",
    ".tf",
    "pulumi/",
    "cdk.json",
    "serverless.yml",
    "serverless.ts",
    "wrangler.toml",
    "cloudformation/",
    "sam.yml",
    "sam.yaml",
    // Build system / project config
    "Makefile",
    "CMakeLists.txt",
    "build.gradle",
    "pom.xml",
    "build.rs",
    "build.zig",
    ".cargo/config",
    "nx.json",
    "turbo.json",
    "lerna.json",
    "pnpm-workspace.yaml",
    // Dependency manifests (changing deps can signal architectural shifts)
    "Cargo.toml",
    "package.json",
    "go.mod",
    "requirements.txt",
    "pyproject.toml",
    "Gemfile",
    "build.sbt",
    "deps.edn",
    // Database migrations
    "migrations/",
    "alembic/",
    // API definitions
    "openapi",
    "swagger",
    ".proto",
    ".graphql",
    ".gql",
];

/// Minimum number of changed files to consider a diff "large" enough for
/// architectural review, even without explicit architectural file signals.
const LARGE_DIFF_FILE_THRESHOLD: usize = 15;

/// Minimum number of distinct directories touched to consider a diff
/// structurally broad enough for architectural review.
const BROAD_DIFF_DIR_THRESHOLD: usize = 8;

/// Select built-in reviewer profiles based on the changed files and the
/// repository root.
///
/// See the [module-level documentation](self) for the full decision tree.
///
/// # Arguments
///
/// * `diffs` — The set of file diffs to classify.
/// * `repo_root` — Path to the repository root, used to read `package.json`
///   and check for backend config files when JS/TS path signals are
///   ambiguous.
///
/// # Returns
///
/// A non-empty list of profile name strings (e.g. `["frontend", "security"]`)
/// suitable for passing to [`crate::agents::resolve_profiles`].
pub fn auto_select_profiles(diffs: &[FileDiff], repo_root: &Path) -> Vec<String> {
    let mut profiles = Vec::new();
    let mut has_frontend = false;
    let mut has_backend = false;
    let mut has_architect = false;

    // Track JS/TS files separately — their classification is ambiguous.
    let mut has_js_ts = false;
    let mut js_ts_backend_signals = 0u32;
    let mut js_ts_frontend_signals = 0u32;

    for diff in diffs {
        let path = diff.path();
        let ext = path.rsplit('.').next().unwrap_or("");

        // ── Always-frontend extensions ────────────────────────────────
        match ext {
            "vue" | "svelte" | "css" | "scss" | "less" | "html" | "astro" => {
                has_frontend = true;
                continue;
            }
            _ => {}
        }

        // ── Always-backend extensions ─────────────────────────────────
        match ext {
            "rs" | "go" | "py" | "rb" | "java" | "kt" | "cs" | "php" | "ex" | "exs" | "c"
            | "cpp" | "h" | "hpp" | "scala" | "clj" | "zig" | "nim" | "erl" | "gleam" => {
                has_backend = true;
                continue;
            }
            _ => {}
        }

        // ── Ambiguous JS/TS extensions — classify by context ──────────
        if matches!(
            ext,
            "js" | "jsx" | "ts" | "tsx" | "mjs" | "mts" | "cjs" | "cts"
        ) {
            has_js_ts = true;

            // Filename-suffix heuristics (e.g. *.controller.ts → backend)
            if JS_BACKEND_FILE_SUFFIXES.iter().any(|s| path.ends_with(s)) {
                js_ts_backend_signals += 1;
                continue;
            }

            // Root-level server entrypoints (e.g. src/server.ts → backend)
            let filename = path.rsplit('/').next().unwrap_or(path);
            if JS_BACKEND_ROOT_FILES.contains(&filename) && !is_frontend_path(path) {
                js_ts_backend_signals += 1;
                continue;
            }

            // Path-segment heuristics
            if JS_BACKEND_PATH_SEGMENTS
                .iter()
                .any(|seg| path.contains(seg))
            {
                js_ts_backend_signals += 1;
                continue;
            }
            if JS_FRONTEND_PATH_SEGMENTS
                .iter()
                .any(|seg| path.contains(seg))
            {
                js_ts_frontend_signals += 1;
                continue;
            }

            // JSX/TSX files without explicit backend signals lean frontend
            if matches!(ext, "jsx" | "tsx") {
                js_ts_frontend_signals += 1;
            }

            continue;
        }

        // ── Generic path-based heuristics for other extensions ────────
        if path.contains("frontend/") || path.contains("client/") {
            has_frontend = true;
        }
        if path.contains("backend/") || path.contains("server/") || path.contains("api/") {
            has_backend = true;
        }
    }

    // ── Architect triggers ────────────────────────────────────────────
    // 1. Structural files: CI, IaC, build configs, dependency manifests, etc.
    let touches_architecture = diffs.iter().any(|d| {
        let p = d.path();
        ARCHITECTURE_FILE_PATTERNS.iter().any(|pat| p.contains(pat))
    });

    // 2. Large or broad diffs: many files or many distinct directories.
    let file_count = diffs.len();
    let dir_count = {
        let mut dirs: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for d in diffs {
            if let Some(parent) = d.path().rsplit_once('/').map(|(dir, _)| dir) {
                dirs.insert(parent);
            }
        }
        dirs.len()
    };
    let is_large_diff =
        file_count >= LARGE_DIFF_FILE_THRESHOLD || dir_count >= BROAD_DIFF_DIR_THRESHOLD;

    if touches_architecture || is_large_diff {
        has_architect = true;
    }

    // ── Resolve ambiguous JS/TS classification ────────────────────────
    if has_js_ts {
        // Start from path-level signals, then layer in project root context.
        // Path signals are authoritative for the files they match, but some
        // JS/TS files may not sit under a recognized directory (e.g.
        // `worker/src/index.ts`), so we always consult the project root to
        // catch what paths alone miss.
        if js_ts_backend_signals > 0 {
            has_backend = true;
        }
        if js_ts_frontend_signals > 0 {
            has_frontend = true;
        }

        // Consult the project root when paths left either side unresolved,
        // or when there were JS/TS files with no path signal at all.
        let has_unclassified_js_ts = has_js_ts
            && (js_ts_backend_signals + js_ts_frontend_signals)
                < diffs
                    .iter()
                    .filter(|d| {
                        matches!(
                            d.path().rsplit('.').next().unwrap_or(""),
                            "js" | "jsx" | "ts" | "tsx" | "mjs" | "mts" | "cjs" | "cts"
                        )
                    })
                    .count() as u32;

        if has_unclassified_js_ts
            || (has_js_ts && (js_ts_backend_signals == 0 || js_ts_frontend_signals == 0))
        {
            let root_signal = detect_js_ts_project_type(repo_root);
            match root_signal {
                JsTsSignal::Backend => has_backend = true,
                JsTsSignal::Frontend => has_frontend = true,
                JsTsSignal::Both => {
                    has_backend = true;
                    has_frontend = true;
                }
                JsTsSignal::Unknown => {
                    // Root gave no signal either — if paths didn't resolve
                    // anything, default JS/TS to frontend.
                    if !has_backend && !has_frontend {
                        has_frontend = true;
                    }
                }
            }
        }
    }

    // ── Build profile list ────────────────────────────────────────────
    if has_frontend {
        profiles.push("frontend".to_string());
    }
    if has_backend || !has_frontend {
        profiles.push(DEFAULT_PROFILE.to_string());
    }
    if has_architect {
        profiles.push("architect".to_string());
    }

    // Always include security for comprehensive reviews
    profiles.push("security".to_string());

    profiles
}

/// Returns `true` if the path looks like it sits inside a frontend directory.
fn is_frontend_path(path: &str) -> bool {
    JS_FRONTEND_PATH_SEGMENTS
        .iter()
        .any(|seg| path.contains(seg))
}

/// Coarse classification for a JS/TS project.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JsTsSignal {
    Backend,
    Frontend,
    Both,
    Unknown,
}

/// Inspect project root files to determine whether a JS/TS project is a
/// backend, a frontend, or both (fullstack).
fn detect_js_ts_project_type(repo_root: &Path) -> JsTsSignal {
    // 1. Config-file shortcuts — presence alone is a strong signal.
    let has_backend_config = BACKEND_ROOT_CONFIG_FILES
        .iter()
        .any(|f| repo_root.join(f).exists());

    // 2. Parse package.json dependencies.
    let (has_backend_dep, has_frontend_dep) = inspect_package_json(repo_root);

    let backend = has_backend_config || has_backend_dep;
    let frontend = has_frontend_dep;

    match (backend, frontend) {
        (true, true) => JsTsSignal::Both,
        (true, false) => JsTsSignal::Backend,
        (false, true) => JsTsSignal::Frontend,
        (false, false) => JsTsSignal::Unknown,
    }
}

/// Read `package.json` in `repo_root` and return `(has_backend_dep, has_frontend_dep)`.
fn inspect_package_json(repo_root: &Path) -> (bool, bool) {
    let pkg_path = repo_root.join("package.json");
    let Ok(content) = std::fs::read_to_string(&pkg_path) else {
        return (false, false);
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) else {
        return (false, false);
    };

    let dep_names = ["dependencies", "devDependencies", "peerDependencies"];
    let mut all_deps: Vec<&str> = Vec::new();

    for key in &dep_names {
        if let Some(obj) = json.get(key).and_then(|v| v.as_object()) {
            all_deps.extend(obj.keys().map(|k| k.as_str()));
        }
    }

    let has_backend = all_deps.iter().any(|d| BACKEND_DEPS.contains(d));

    // Frontend frameworks / libraries
    let frontend_deps = [
        "react",
        "react-dom",
        "vue",
        "svelte",
        "@sveltejs/kit",
        "@angular/core",
        "next",
        "nuxt",
        "gatsby",
        "vite",
        "@vitejs/plugin-react",
        "webpack",
        "parcel",
        "astro",
        "solid-js",
        "preact",
        "lit",
        "ember-source",
        "@remix-run/react",
    ];
    let has_frontend = all_deps.iter().any(|d| frontend_deps.contains(d));

    (has_backend, has_frontend)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::diff::FileDiff;
    use std::path::PathBuf;

    fn make_diff(path: &str) -> FileDiff {
        FileDiff {
            old_path: path.to_string(),
            new_path: path.to_string(),
            is_new: false,
            is_deleted: false,
            is_rename: false,
            is_binary: false,
            hunks: vec![],
        }
    }

    /// Returns a temp dir with no project root files (bare repo root).
    fn bare_root() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_path_buf();
        (dir, path)
    }

    /// Returns a temp dir with a package.json containing the given deps.
    fn root_with_package_json(deps: &[&str]) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_path_buf();
        let dep_entries: Vec<String> = deps.iter().map(|d| format!("\"{d}\": \"*\"")).collect();
        let pkg = format!(r#"{{"dependencies": {{{}}}}}"#, dep_entries.join(", "));
        std::fs::write(path.join("package.json"), pkg).unwrap();
        (dir, path)
    }

    // ── Basic extension classification ────────────────────────────────

    #[test]
    fn selects_frontend_for_vue_file() {
        let (_dir, root) = bare_root();
        let diffs = vec![make_diff("src/App.vue")];
        let profiles = auto_select_profiles(&diffs, &root);
        assert!(profiles.contains(&"frontend".to_string()));
        assert!(!profiles.contains(&"backend".to_string()));
    }

    #[test]
    fn selects_backend_for_rust_file() {
        let (_dir, root) = bare_root();
        let diffs = vec![make_diff("src/handler.rs")];
        let profiles = auto_select_profiles(&diffs, &root);
        assert!(profiles.contains(&"backend".to_string()));
    }

    #[test]
    fn defaults_to_backend_for_unknown() {
        let (_dir, root) = bare_root();
        let diffs = vec![make_diff("README.md")];
        let profiles = auto_select_profiles(&diffs, &root);
        assert!(profiles.contains(&"backend".to_string()));
    }

    #[test]
    fn always_includes_security() {
        let (_dir, root) = bare_root();
        let diffs = vec![make_diff("anything.txt")];
        let profiles = auto_select_profiles(&diffs, &root);
        assert!(profiles.contains(&"security".to_string()));
    }

    // ── JS/TS path-based heuristics ──────────────────────────────────

    #[test]
    fn js_in_controllers_dir_selects_backend() {
        let (_dir, root) = bare_root();
        let diffs = vec![make_diff("src/controllers/user.controller.ts")];
        let profiles = auto_select_profiles(&diffs, &root);
        assert!(profiles.contains(&"backend".to_string()));
        assert!(!profiles.contains(&"frontend".to_string()));
    }

    #[test]
    fn js_in_middleware_dir_selects_backend() {
        let (_dir, root) = bare_root();
        let diffs = vec![make_diff("src/middleware/auth.ts")];
        let profiles = auto_select_profiles(&diffs, &root);
        assert!(profiles.contains(&"backend".to_string()));
    }

    #[test]
    fn js_in_routes_dir_selects_backend() {
        let (_dir, root) = bare_root();
        let diffs = vec![make_diff("src/routes/api.ts")];
        let profiles = auto_select_profiles(&diffs, &root);
        assert!(profiles.contains(&"backend".to_string()));
    }

    #[test]
    fn tsx_in_components_dir_selects_frontend() {
        let (_dir, root) = bare_root();
        let diffs = vec![make_diff("src/components/Button.tsx")];
        let profiles = auto_select_profiles(&diffs, &root);
        assert!(profiles.contains(&"frontend".to_string()));
    }

    #[test]
    fn nestjs_filename_pattern_selects_backend() {
        let (_dir, root) = bare_root();
        let diffs = vec![
            make_diff("src/users/users.controller.ts"),
            make_diff("src/users/users.service.ts"),
            make_diff("src/users/users.module.ts"),
        ];
        let profiles = auto_select_profiles(&diffs, &root);
        assert!(profiles.contains(&"backend".to_string()));
        assert!(!profiles.contains(&"frontend".to_string()));
    }

    #[test]
    fn tsx_without_path_signal_leans_frontend() {
        let (_dir, root) = bare_root();
        let diffs = vec![make_diff("src/Widget.tsx")];
        let profiles = auto_select_profiles(&diffs, &root);
        assert!(profiles.contains(&"frontend".to_string()));
    }

    // ── package.json inspection ──────────────────────────────────────

    #[test]
    fn express_in_package_json_selects_backend() {
        let (_dir, root) = root_with_package_json(&["express", "cors"]);
        // Plain .ts file, no path signal
        let diffs = vec![make_diff("src/index.ts")];
        let profiles = auto_select_profiles(&diffs, &root);
        assert!(profiles.contains(&"backend".to_string()));
    }

    #[test]
    fn react_in_package_json_selects_frontend() {
        let (_dir, root) = root_with_package_json(&["react", "react-dom"]);
        let diffs = vec![make_diff("src/App.ts")];
        let profiles = auto_select_profiles(&diffs, &root);
        assert!(profiles.contains(&"frontend".to_string()));
    }

    #[test]
    fn fullstack_package_json_selects_both() {
        let (_dir, root) = root_with_package_json(&["express", "react", "react-dom"]);
        let diffs = vec![make_diff("src/utils.ts")];
        let profiles = auto_select_profiles(&diffs, &root);
        assert!(profiles.contains(&"backend".to_string()));
        assert!(profiles.contains(&"frontend".to_string()));
    }

    #[test]
    fn nestjs_config_file_selects_backend() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        std::fs::write(root.join("nest-cli.json"), "{}").unwrap();
        let diffs = vec![make_diff("src/app.module.ts")];
        let profiles = auto_select_profiles(&diffs, &root);
        assert!(profiles.contains(&"backend".to_string()));
    }

    // ── Mixed diffs ──────────────────────────────────────────────────

    #[test]
    fn mixed_rust_and_frontend_selects_both() {
        let (_dir, root) = bare_root();
        let diffs = vec![
            make_diff("backend/src/main.rs"),
            make_diff("frontend/src/App.vue"),
        ];
        let profiles = auto_select_profiles(&diffs, &root);
        assert!(profiles.contains(&"backend".to_string()));
        assert!(profiles.contains(&"frontend".to_string()));
    }

    #[test]
    fn mixed_backend_ts_and_frontend_tsx() {
        let (_dir, root) = bare_root();
        let diffs = vec![
            make_diff("server/routes/api.ts"),
            make_diff("client/components/Header.tsx"),
        ];
        let profiles = auto_select_profiles(&diffs, &root);
        assert!(profiles.contains(&"backend".to_string()));
        assert!(profiles.contains(&"frontend".to_string()));
    }

    // ── Monorepo / mixed project root scenarios ──────────────────────

    #[test]
    fn cloudflare_worker_with_frontend_paths_selects_both() {
        // wrangler.toml at root, but the diff has both a worker file
        // (no recognizable path segment) and a frontend component.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        std::fs::write(root.join("wrangler.toml"), "").unwrap();
        let diffs = vec![
            make_diff("worker/src/index.ts"),       // no path signal
            make_diff("web/components/Header.tsx"), // frontend path signal
        ];
        let profiles = auto_select_profiles(&diffs, &root);
        assert!(
            profiles.contains(&"backend".to_string()),
            "wrangler.toml should cause backend to be selected"
        );
        assert!(
            profiles.contains(&"frontend".to_string()),
            "components/ path should cause frontend to be selected"
        );
    }

    #[test]
    fn wrangler_toml_with_ambiguous_ts_selects_backend() {
        // Pure worker repo: wrangler.toml at root, only ambiguous .ts files.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        std::fs::write(root.join("wrangler.toml"), "").unwrap();
        let diffs = vec![make_diff("src/index.ts")];
        let profiles = auto_select_profiles(&diffs, &root);
        assert!(profiles.contains(&"backend".to_string()));
    }

    #[test]
    fn frontend_path_signal_plus_backend_package_json_selects_both() {
        // Monorepo: package.json has express, but the changed files are
        // in a frontend directory plus an ambiguous shared util.
        let (_dir, root) = root_with_package_json(&["express", "react", "react-dom"]);
        let diffs = vec![
            make_diff("packages/web/components/Button.tsx"),
            make_diff("packages/shared/utils.ts"),
        ];
        let profiles = auto_select_profiles(&diffs, &root);
        assert!(profiles.contains(&"backend".to_string()));
        assert!(profiles.contains(&"frontend".to_string()));
    }

    #[test]
    fn only_frontend_paths_with_backend_root_still_includes_backend() {
        // Even if all changed JS files are in frontend paths, a backend
        // root signal should still add the backend profile because
        // unclassified files or one-sided signals trigger root inspection.
        let (_dir, root) = root_with_package_json(&["express"]);
        let diffs = vec![make_diff("src/components/Widget.tsx")];
        let profiles = auto_select_profiles(&diffs, &root);
        assert!(profiles.contains(&"frontend".to_string()));
        assert!(
            profiles.contains(&"backend".to_string()),
            "express in package.json should add backend even with only frontend paths"
        );
    }

    // ── Architect triggers ───────────────────────────────────────────

    #[test]
    fn dockerfile_triggers_architect() {
        let (_dir, root) = bare_root();
        let diffs = vec![make_diff("Dockerfile"), make_diff("src/main.rs")];
        let profiles = auto_select_profiles(&diffs, &root);
        assert!(
            profiles.contains(&"architect".to_string()),
            "Dockerfile should trigger architect profile"
        );
    }

    #[test]
    fn ci_workflow_triggers_architect() {
        let (_dir, root) = bare_root();
        let diffs = vec![make_diff(".github/workflows/ci.yml")];
        let profiles = auto_select_profiles(&diffs, &root);
        assert!(profiles.contains(&"architect".to_string()));
    }

    #[test]
    fn terraform_triggers_architect() {
        let (_dir, root) = bare_root();
        let diffs = vec![make_diff("infra/main.tf")];
        let profiles = auto_select_profiles(&diffs, &root);
        assert!(profiles.contains(&"architect".to_string()));
    }

    #[test]
    fn cargo_toml_triggers_architect() {
        let (_dir, root) = bare_root();
        let diffs = vec![make_diff("Cargo.toml"), make_diff("src/lib.rs")];
        let profiles = auto_select_profiles(&diffs, &root);
        assert!(profiles.contains(&"architect".to_string()));
    }

    #[test]
    fn package_json_triggers_architect() {
        let (_dir, root) = bare_root();
        let diffs = vec![make_diff("package.json"), make_diff("src/index.ts")];
        let profiles = auto_select_profiles(&diffs, &root);
        assert!(profiles.contains(&"architect".to_string()));
    }

    #[test]
    fn proto_file_triggers_architect() {
        let (_dir, root) = bare_root();
        let diffs = vec![make_diff("api/service.proto")];
        let profiles = auto_select_profiles(&diffs, &root);
        assert!(profiles.contains(&"architect".to_string()));
    }

    #[test]
    fn migration_triggers_architect() {
        let (_dir, root) = bare_root();
        let diffs = vec![make_diff("migrations/20260101_create_users.sql")];
        let profiles = auto_select_profiles(&diffs, &root);
        assert!(profiles.contains(&"architect".to_string()));
    }

    #[test]
    fn large_diff_triggers_architect() {
        let (_dir, root) = bare_root();
        // 15 files across a few dirs — meets the file count threshold.
        let diffs: Vec<FileDiff> = (0..15)
            .map(|i| make_diff(&format!("src/module_{i}.rs")))
            .collect();
        let profiles = auto_select_profiles(&diffs, &root);
        assert!(
            profiles.contains(&"architect".to_string()),
            "15+ files should trigger architect profile"
        );
    }

    #[test]
    fn broad_diff_triggers_architect() {
        let (_dir, root) = bare_root();
        // 8 files in 8 distinct directories — meets the directory breadth threshold.
        let diffs: Vec<FileDiff> = (0..8)
            .map(|i| make_diff(&format!("src/module_{i}/lib.rs")))
            .collect();
        let profiles = auto_select_profiles(&diffs, &root);
        assert!(
            profiles.contains(&"architect".to_string()),
            "8+ distinct directories should trigger architect profile"
        );
    }

    #[test]
    fn small_isolated_change_does_not_trigger_architect() {
        let (_dir, root) = bare_root();
        let diffs = vec![make_diff("src/handler.rs")];
        let profiles = auto_select_profiles(&diffs, &root);
        assert!(
            !profiles.contains(&"architect".to_string()),
            "single file change should not trigger architect"
        );
    }

    #[test]
    fn few_files_in_same_dir_does_not_trigger_architect() {
        let (_dir, root) = bare_root();
        let diffs: Vec<FileDiff> = (0..5)
            .map(|i| make_diff(&format!("src/handlers/handler_{i}.rs")))
            .collect();
        let profiles = auto_select_profiles(&diffs, &root);
        assert!(
            !profiles.contains(&"architect".to_string()),
            "5 files in one directory should not trigger architect"
        );
    }
}
